use std::{
    sync::{Arc, Barrier},
    thread,
};

use crate::{
    CommandMessage, ProcessingState, StatusMessage, StatusStructs, audiodevice, config, processing,
};

/// Supervisory handles for the running capture/processing/playback threads.
pub struct EnginePipeline {
    /// commands (set speed, exit) to the capture thread.
    tx_command_cap: crossbeam_channel::Sender<CommandMessage>,
    /// config updates to the processing thread.
    tx_pipeconf: crossbeam_channel::Sender<(config::ConfigChange, config::Configuration)>,
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

    /// Send a config update to the processing thread
    pub fn update_processing_config(
        &self,
        change: config::ConfigChange,
        configuration: config::Configuration,
    ) {
        self.tx_pipeconf.send((change, configuration)).unwrap();
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
