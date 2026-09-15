// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

use std::{
    sync::{Arc, Barrier},
    thread,
};

use crate::filters::fftconv::{ConvCoeffCache, ImpulseCache};
use crate::{
    CommandMessage, ProcessingState, Res, StatusMessage, StatusStructs, audiodevice, config,
    processing,
};

/// Supervisory handles for the running capture/processing/playback threads.
pub struct EnginePipeline {
    /// commands (set speed, exit) to the capture thread.
    tx_command_cap: crossbeam_channel::Sender<CommandMessage>,
    /// config updates to the processing thread, with the coefficients they need.
    tx_pipeconf: crossbeam_channel::Sender<processing::PipelineConfig>,
    /// 4-way startup barrier (capture, playback, processing, supervisor).
    barrier: Arc<Barrier>,
    pb_handle: Box<thread::JoinHandle<()>>,
    cap_handle: Box<thread::JoinHandle<()>>,
    pb_ready: bool,
    cap_ready: bool,
}

impl EnginePipeline {
    /// Both capture and playback are ready: release the startup barrier so the
    /// threads begin processing. Returns `true` once startup is complete.
    pub fn release_barrier_if_ready(&self) -> bool {
        if self.pb_ready && self.cap_ready {
            debug!("Both capture and playback ready, release barrier");
            self.barrier.wait();
            debug!("Supervisor loop starts now!");
            true
        } else {
            false
        }
    }

    /// Tell the capture thread to exit, release the startup barrier if we are
    /// still starting (so the device/processing threads unblock), then join the
    /// capture and playback threads.
    pub fn stop(self, is_starting: bool) {
        if self.tx_command_cap.send(CommandMessage::Exit).is_err() {
            debug!("Capture thread has already exited");
        }
        if is_starting {
            debug!("Stopping while still starting, release barrier");
            self.barrier.wait();
        }
        trace!("Wait for playback thread to exit..");
        self.pb_handle.join().unwrap();
        trace!("Wait for capture thread to exit..");
        self.cap_handle.join().unwrap();
    }

    /// Transform the convolution coefficients the change needs, then send it to
    /// the processing thread.
    ///
    /// The transforms happen here, on the supervisor thread, rather than on the
    /// processing thread as it applies the change. That thread has been
    /// promoted to real time, so the work it used to do here stalled the audio
    /// for as long as it took, which for a multichannel config of long FIR
    /// filters is a sizeable fraction of the playback buffer. What it does now
    /// is assemble filters from coefficients already in memory. See
    /// `benches/pipeline_build.rs` for what the two cost on a given machine.
    ///
    /// `impulses` is what validating `configuration` read, so nothing is read
    /// from the file system here either. An `Err` means the change was not
    /// sent and the pipeline keeps running the config it has.
    pub fn update_processing_config(
        &self,
        change: config::ConfigChange,
        configuration: config::Configuration,
        impulses: &ImpulseCache,
    ) -> Res<()> {
        let names = filters_to_build(&change, &configuration);
        let coeff_cache = ConvCoeffCache::transformed(&configuration, &names, impulses)?;
        self.tx_pipeconf
            .send((change, configuration, coeff_cache))
            .unwrap();
        Ok(())
    }

    /// Set playback readiness state
    pub fn set_playback_ready(&mut self) {
        self.pb_ready = true;
    }

    /// Set capture readiness state
    pub fn set_capture_ready(&mut self) {
        self.cap_ready = true;
    }

    pub fn send_capture_command(&self, command: CommandMessage) {
        if self.tx_command_cap.send(command).is_err() {
            debug!("Capture thread has already exited");
        }
    }
}

/// Open the devices and spawn the capture, processing, and playback threads for
/// `active_config`. Returns the supervisor handles plus the channel on which the
/// device threads report their status.
pub fn start_pipeline(
    active_config: &config::Configuration,
    status_structs: &StatusStructs,
) -> (EnginePipeline, crossbeam_channel::Receiver<StatusMessage>) {
    let (tx_pb, rx_pb) = crossbeam_channel::bounded(active_config.devices.queuelimit());
    let (tx_cap, rx_cap) = crossbeam_channel::bounded(active_config.devices.queuelimit());
    let (tx_status, rx_status) = crossbeam_channel::unbounded();
    let (tx_command_cap, rx_command_cap) = crossbeam_channel::unbounded();
    let (tx_pipeconf, rx_pipeconf) = crossbeam_channel::unbounded();
    let barrier = Arc::new(Barrier::new(4));

    // Processing thread
    processing::run_processing(
        active_config.clone(),
        barrier.clone(),
        tx_pb,
        rx_cap,
        rx_pipeconf,
        status_structs.processing.clone(),
    );

    // Playback thread
    let mut playback_dev = audiodevice::new_playback_device(active_config.devices.clone());
    let pb_handle = playback_dev
        .start(
            rx_pb,
            barrier.clone(),
            tx_status.clone(),
            status_structs.playback.clone(),
        )
        .unwrap();

    let used_channels = config::used_capture_channels(active_config);
    debug!("Using channels {used_channels:?}");
    {
        let mut capture_status = status_structs.capture.write();
        crate::update_capture_state(&mut capture_status, ProcessingState::Starting);
        capture_status.used_channels = used_channels;
    }

    // Capture thread
    let mut capture_dev = audiodevice::new_capture_device(active_config.devices.clone());
    let cap_handle = capture_dev
        .start(
            tx_cap,
            barrier.clone(),
            tx_status,
            rx_command_cap,
            status_structs.capture.clone(),
            status_structs.processing.clone(),
        )
        .unwrap();

    let pipeline = EnginePipeline {
        tx_command_cap,
        tx_pipeconf,
        barrier,
        pb_handle,
        cap_handle,
        pb_ready: false,
        cap_ready: false,
    };
    (pipeline, rx_status)
}

/// The filters the processing thread is going to construct when it applies
/// `change`, and therefore the ones whose coefficients have to be ready.
fn filters_to_build(change: &config::ConfigChange, conf: &config::Configuration) -> Vec<String> {
    match change {
        // A parameter update only touches the filters it names, so preparing
        // the rest would be work for nothing. `config_diff` compares every
        // filter in the config, not just the ones the pipeline uses, so the
        // names are narrowed to the pipeline here. An unused filter is never
        // built, and it was never validated either, so reading one could fail
        // over a file that nothing was ever going to open.
        config::ConfigChange::FilterParameters { filters, .. } => pipeline_filter_names(conf)
            .into_iter()
            .filter(|name| filters.contains(name))
            .collect(),
        // A rebuild constructs every filter the pipeline names.
        config::ConfigChange::Pipeline | config::ConfigChange::MixerParameters => {
            pipeline_filter_names(conf)
        }
        // A device change restarts everything, and nothing else reaches the
        // processing thread at all.
        config::ConfigChange::Devices | config::ConfigChange::None => Vec::new(),
    }
}

/// Every filter name the pipeline refers to, in the steps that are not
/// bypassed.
///
/// Duplicates are harmless: transforming is idempotent per name, so a filter
/// named by several steps is done once and hit from the cache after that.
fn pipeline_filter_names(conf: &config::Configuration) -> Vec<String> {
    let Some(pipeline) = conf.pipeline.as_ref() else {
        return Vec::new();
    };
    pipeline
        .iter()
        .filter_map(|step| match step {
            config::PipelineStep::Filter(step) if !step.is_bypassed() => Some(&step.names),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::fftconv::ImpulseCache;
    use std::io::Write;

    fn config_from_json(filters: &str, pipeline: &str) -> config::Configuration {
        let json = format!(
            r#"{{
                "devices": {{
                    "samplerate": 48000,
                    "chunksize": 1024,
                    "capture": {{"type": "Stdin", "channels": 2, "format": "F32_LE"}},
                    "playback": {{"type": "Stdout", "channels": 2, "format": "F32_LE"}}
                }},
                "filters": {filters},
                "pipeline": {pipeline}
            }}"#
        );
        serde_json::from_str(&json).expect("the test config is valid")
    }

    const TWO_DUMMIES: &str = r#"{
        "conv_a": {"type": "Conv", "parameters": {"type": "Dummy", "length": 16}},
        "conv_b": {"type": "Conv", "parameters": {"type": "Dummy", "length": 16}}
    }"#;

    fn filter_change(names: &[&str]) -> config::ConfigChange {
        config::ConfigChange::FilterParameters {
            filters: names.iter().map(|n| n.to_string()).collect(),
            processors: Vec::new(),
        }
    }

    #[test]
    fn bypassed_steps_are_not_built() {
        let conf = config_from_json(
            TWO_DUMMIES,
            r#"[
                {"type": "Filter", "names": ["conv_a"]},
                {"type": "Filter", "names": ["conv_b"], "bypassed": true}
            ]"#,
        );
        assert_eq!(pipeline_filter_names(&conf), vec!["conv_a".to_string()]);
    }

    #[test]
    fn a_filter_named_by_several_steps_is_transformed_once() {
        let conf = config_from_json(
            TWO_DUMMIES,
            r#"[
                {"type": "Filter", "channels": [0], "names": ["conv_a"]},
                {"type": "Filter", "channels": [1], "names": ["conv_a"]}
            ]"#,
        );
        let names = pipeline_filter_names(&conf);
        assert_eq!(names.len(), 2);
        let cache = ConvCoeffCache::transformed(&conf, &names, &ImpulseCache::new())
            .expect("dummy coefficients always load");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_parameter_change_builds_only_what_it_names() {
        let conf = config_from_json(
            TWO_DUMMIES,
            r#"[{"type": "Filter", "names": ["conv_a", "conv_b"]}]"#,
        );
        assert_eq!(
            filters_to_build(&filter_change(&["conv_b"]), &conf),
            vec!["conv_b".to_string()]
        );
    }

    /// A filter that is only defined, never used, is not validated and never
    /// built, so a change that names it must not send anyone to read its file.
    #[test]
    fn a_changed_filter_outside_the_pipeline_is_not_built() {
        let conf = config_from_json(TWO_DUMMIES, r#"[{"type": "Filter", "names": ["conv_a"]}]"#);
        assert!(filters_to_build(&filter_change(&["conv_b"]), &conf).is_empty());
    }

    #[test]
    fn a_device_change_builds_nothing() {
        let conf = config_from_json(TWO_DUMMIES, r#"[{"type": "Filter", "names": ["conv_a"]}]"#);
        assert!(filters_to_build(&config::ConfigChange::Devices, &conf).is_empty());
    }

    fn temp_coeff_file(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "camilladsp_test_coeffs_{}_{}.raw",
            name,
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path).expect("can create the fixture");
        for value in [1.0_f64, 0.5, 0.25, 0.125] {
            file.write_all(&value.to_le_bytes()).expect("can write");
        }
        path
    }

    /// The point of the whole arrangement: validation reads the coefficient
    /// files, and nothing reads them again. Proved by deleting the file after
    /// validating and preparing the change anyway.
    #[test]
    fn validated_coefficients_are_not_read_a_second_time() {
        let path = temp_coeff_file("once");
        let filters = format!(
            r#"{{"conv_file": {{"type": "Conv", "parameters": {{
                "type": "Raw", "filename": "{}", "format": "F64_LE"
            }}}}}}"#,
            path.to_str().unwrap()
        );
        let mut conf =
            config_from_json(&filters, r#"[{"type": "Filter", "names": ["conv_file"]}]"#);

        let impulses =
            config::validate_config(&mut conf, None).expect("the fixture config is valid");
        assert_eq!(impulses.len(), 1, "validation should keep what it read");

        std::fs::remove_file(&path).expect("can remove the fixture");

        let names = filters_to_build(&config::ConfigChange::Pipeline, &conf);
        let cache = ConvCoeffCache::transformed(&conf, &names, &impulses)
            .expect("the coefficients came from validation, not from the file");
        assert_eq!(cache.len(), 1);

        // And the same call without them has nowhere to go but the file.
        assert!(
            ConvCoeffCache::transformed(&conf, &names, &ImpulseCache::new()).is_err(),
            "an empty impulse cache should fall back to reading, and fail"
        );
    }
}
