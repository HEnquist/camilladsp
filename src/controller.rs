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

//! Built-in controller: source format following and error recovery.
//!
//! This module holds the settings and the config selection, which are plain functions
//! of the entry config, the reported source format and the settings. The state machine
//! that runs sessions and decides what happens when one ends is in `engine.rs`.
//!
//! A config comes from one of two sources. From the files source, the controller loads the
//! entry config, the active config file, through a provider. Single runs it as is. When the
//! capture source changes format, following runs the config through the Adapt provider,
//! which adapts the entry config to the new format. A config from the websocket source runs
//! exactly as sent.

use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;

use crate::config::{self, Configuration};
use crate::filters::fftconv::ImpulseCache;
pub use camilladsp_schema::protocol::{
    AdaptCheck, ConfigProvider, ControllerFileChecks, ControllerSettings, ControllerSource,
    SourceFormat,
};

/// How close a reported rate has to be to a standard rate to be snapped to it, relative.
///
/// A rate measured over a short window can be more than 1% off, 44581 for 44100 at a
/// 0.2 s window. The closest standard rates, 44100 and 48000, are 8.8% apart, so 3% still
/// can't snap to the wrong one.
const SNAP_TOLERANCE: f64 = 0.03;

/// What a capture source is doing right now, see `audiodevice::query_capture_source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// The source is active at this format.
    Format(SourceFormat),
    /// The backend can tell that nothing feeds the capture.
    Inactive,
    /// The backend can't tell.
    Unknown,
}

/// A config as it was loaded, before overrides, token expansion and path resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSource {
    pub raw: Configuration,
    /// The file it was read from, used to resolve relative paths.
    pub filename: Option<String>,
}

/// What a loaded config is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    /// A new entry config, which switches to the files source.
    Entry,
    /// A config sent or patched over the websocket. It switches to the websocket source,
    /// where it runs exactly as sent, until an entry is loaded from a file. The entry stays
    /// as it is.
    Websocket,
}

/// A config sent to the controller, along with the result of validating it as is.
///
/// The sender validates before sending, so that a client gets its error directly. The
/// result is reused when the config runs as it is, so that the coefficient files are
/// not read twice.
pub struct LoadedConfig {
    pub source: ConfigSource,
    pub kind: LoadKind,
    pub validated: Configuration,
    pub impulses: ImpulseCache,
}

impl LoadedConfig {
    /// Validate `source` as is, and bundle it for sending to the controller.
    pub fn validate(source: ConfigSource, kind: LoadKind) -> crate::Res<Self> {
        let mut validated = source.raw.clone();
        let impulses = config::validate_config(&mut validated, source.filename.as_deref())?;
        Ok(LoadedConfig {
            source,
            kind,
            validated,
            impulses,
        })
    }

    /// Load a config file and validate it as is.
    pub fn from_file(filename: &str) -> crate::Res<Self> {
        let raw = config::load_config(filename)?;
        Self::validate(
            ConfigSource {
                raw,
                filename: Some(filename.to_string()),
            },
            LoadKind::Entry,
        )
    }

    /// Validate a patched copy of the running config, for the websocket source.
    ///
    /// The running config already has the CLI overrides applied, and the adaptation
    /// following made, so applying `-r` again would put it back at the override rate. With
    /// `-r` set, it is validated at its own capture rate instead, which leaves the rate as it
    /// is. The other overrides give the same result when applied again. Its tokens are
    /// already expanded and its relative paths resolved, so it needs no file name.
    pub fn patched(raw: Configuration) -> crate::Res<Self> {
        let rate = config::OVERRIDES
            .read()
            .samplerate
            .and_then(|_| NonZeroUsize::new(capture_rate_of(&raw)));
        let mut validated = raw.clone();
        let impulses = config::validate_config_at_rate(&mut validated, None, rate)?;
        Ok(LoadedConfig {
            source: ConfigSource {
                raw,
                filename: None,
            },
            kind: LoadKind::Websocket,
            validated,
            impulses,
        })
    }
}

/// Which provider supplied a config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    /// The entry config as is.
    Single,
    /// The Adapt provider.
    Adapt,
    /// A config from the websocket source, run as sent.
    Websocket,
}

/// A config ready to run.
pub struct Selected {
    pub config: Configuration,
    pub impulses: ImpulseCache,
    pub provider: Provider,
    /// The source format following selected it for, if any.
    pub format: Option<SourceFormat>,
}

/// The outcome of [`select_config`].
pub enum Selection {
    /// A config to run.
    Run(Box<Selected>),
    /// The entry config doesn't validate.
    Invalid(String),
    /// Following found no config for this format.
    NoConfig(SourceFormat),
}

/// Live state of the controller, for `GetControllerStatus`.
#[derive(Clone, Debug, Default)]
pub struct ControllerStatus {
    /// Where the config comes from, kept until the next switch.
    pub source: ControllerSource,
    /// The provider of the running config, `None` for the websocket source and while
    /// nothing runs.
    pub provider: Option<ConfigProvider>,
    /// The file that runs, `None` when `provider` is.
    pub config_file: Option<String>,
    /// Waiting to retry after an error.
    pub recovering: bool,
    /// Retry attempts since the last session that ran long enough.
    pub attempts: usize,
    /// When the next retry is due, while waiting for it.
    pub next_retry: Option<Instant>,
    /// The format Adapt had no config for, while waiting for the source to change.
    pub waiting_for_source: Option<SourceFormat>,
}

/// Controller state shared with the websocket server.
#[derive(Clone, Debug, Default)]
pub struct ControllerShared {
    /// The settings, `None` when the statefile has no controller section.
    pub settings: Arc<Mutex<Option<ControllerSettings>>>,
    pub status: Arc<Mutex<ControllerStatus>>,
    /// The entry config, for `CheckControllerFiles`.
    pub entry: Arc<Mutex<Option<ConfigSource>>>,
    /// Whether the process runs in wait mode, without which the controller does nothing.
    pub wait: bool,
}

impl ControllerShared {
    /// The current settings, with defaults if there are none.
    pub fn settings(&self) -> ControllerSettings {
        self.settings.lock().clone().unwrap_or_default()
    }
}

/// Snap a measured rate to a standard rate within [`SNAP_TOLERANCE`] of it.
///
/// Measured rates come out like 44097, and a config for that doesn't exist.
pub fn snap_rate(rate: usize) -> usize {
    crate::STANDARD_RATES
        .iter()
        .map(|r| *r as usize)
        .find(|r| ((rate as f64 - *r as f64) / *r as f64).abs() <= SNAP_TOLERANCE)
        .unwrap_or(rate)
}

/// The same format, with the rate snapped to a standard rate. See [`snap_rate`].
pub fn snap_format(format: &SourceFormat) -> SourceFormat {
    SourceFormat {
        samplerate: format.samplerate.map(snap_rate),
        channels: format.channels,
        format: format.format.clone(),
    }
}

/// The rate the capture device of a config opens at.
fn capture_rate_of(conf: &Configuration) -> usize {
    if conf.devices.resampler.is_some() {
        conf.devices.capture_samplerate()
    } else {
        conf.devices.samplerate()
    }
}

/// The Adapt provider: change the rate of the entry config to the reported one.
fn adapt_config(entry: &ConfigSource, format: &SourceFormat) -> Option<Box<Selected>> {
    let Some(rate) = format.known_rate() else {
        info!("Adapt: the source rate is unknown");
        return None;
    };
    let entry_channels = entry.raw.devices.capture.channels();
    if let Some(channels) = format.channels
        && channels != entry_channels
    {
        info!(
            "Adapt: the source has {channels} channels, the entry config captures {entry_channels}"
        );
        return None;
    }
    let mut conf = entry.raw.clone();
    if let Some(fmt) = &format.format
        && conf.devices.capture.format_matches(fmt) == Some(false)
        && let Err(err) = config::set_capture_format_by_name(&mut conf.devices.capture, fmt)
    {
        info!("Adapt: can't change the capture format to {fmt}: {err}");
        return None;
    }
    let impulses =
        match config::validate_config_at_rate(&mut conf, entry.filename.as_deref(), Some(rate)) {
            Ok(impulses) => impulses,
            Err(err) => {
                error!("Adapt: the entry config adapted to {format} is not valid: {err}");
                return None;
            }
        };
    if conf.devices.samplerate() != entry.raw.devices.samplerate()
        && let Some(problem) = rate_change_problem(&entry.raw)
    {
        warn!(
            "Adapt: can't change the samplerate to {}, {problem}",
            conf.devices.samplerate()
        );
        return None;
    }
    Some(Box::new(Selected {
        config: conf,
        impulses,
        provider: Provider::Adapt,
        format: Some(format.clone()),
    }))
}

/// Why Adapt can't run a config at another samplerate than its own, `None` if it can.
/// Only matters where Adapt changes `samplerate`, a resampler keeps the config's rate.
fn rate_change_problem(raw: &Configuration) -> Option<String> {
    let settings = config::rate_dependent_settings(raw);
    if settings.is_empty() {
        return None;
    }
    let rate = raw.devices.samplerate();
    Some(format!(
        "these settings are only right at {rate} Hz: {}. Give delays, attack and release times in a time unit rather than in samples, add $samplerate$ to the coefficient file names, or set samplerate to [{rate}] to resample every other rate to {rate} Hz",
        settings.join(", ")
    ))
}

/// Select the config to run, for an entry config and the last known source format.
///
/// `format` is `None` when following is off or no format is known, and then the entry
/// runs as is. `validated` is the entry already validated as is, reused if given.
pub fn select_config(
    entry: &ConfigSource,
    format: Option<&SourceFormat>,
    validated: Option<(Configuration, ImpulseCache)>,
) -> Selection {
    let Some(format) = format else {
        let (config, impulses) = match validated {
            Some(v) => v,
            None => {
                let mut conf = entry.raw.clone();
                match config::validate_config(&mut conf, entry.filename.as_deref()) {
                    Ok(impulses) => (conf, impulses),
                    Err(err) => return Selection::Invalid(err.to_string()),
                }
            }
        };
        return Selection::Run(Box::new(Selected {
            config,
            impulses,
            provider: Provider::Single,
            format: None,
        }));
    };
    let format = snap_format(format);
    match adapt_config(entry, &format) {
        Some(selected) => Selection::Run(selected),
        None => Selection::NoConfig(format),
    }
}

/// Run a config from the websocket source exactly as sent, validated as it is.
pub fn select_websocket(loaded: LoadedConfig) -> Box<Selected> {
    Box::new(Selected {
        config: loaded.validated,
        impulses: loaded.impulses,
        provider: Provider::Websocket,
        format: None,
    })
}

/// Check the entry config Adapt starts from, when following is on, see
/// [`ControllerFileChecks`].
pub fn check_files(
    settings: &ControllerSettings,
    entry: Option<&ConfigSource>,
) -> ControllerFileChecks {
    ControllerFileChecks {
        adapt: settings.following_enabled().then(|| check_adapt(entry)),
    }
}

/// Check the entry config that Adapt starts from, and read the format it is written for.
///
/// The format is read after validation, so that it includes the command line overrides.
fn check_adapt(entry: Option<&ConfigSource>) -> AdaptCheck {
    let Some(entry) = entry else {
        return AdaptCheck {
            file: None,
            samplerate: None,
            samplerates: None,
            channels: None,
            format: None,
            problem: Some("no config is loaded".to_string()),
        };
    };
    let mut conf = entry.raw.clone();
    // Adapt changes `samplerate` for a rate in a list other than the first, and for any
    // other rate when the config has no resampler.
    let changes_rate = match entry.raw.devices.samplerate_list() {
        Some(rates) => rates.len() > 1,
        None => entry.raw.devices.resampler.is_none(),
    };
    let problem = match config::validate_config(&mut conf, entry.filename.as_deref()) {
        Err(err) => Some(err.to_string()),
        Ok(_) if changes_rate => rate_change_problem(&entry.raw)
            .map(|problem| format!("Adapt can't change the samplerate, {problem}")),
        Ok(_) => None,
    };
    AdaptCheck {
        file: entry.filename.clone(),
        samplerate: Some(capture_rate_of(&conf)),
        // From the config as written, since a `-r` override turns the list into one rate.
        samplerates: entry
            .raw
            .devices
            .samplerate_list()
            .map(|rates| rates.iter().map(|r| r.get()).collect()),
        channels: Some(conf.devices.capture.channels()),
        format: conf.devices.capture.format_name(),
        problem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CaptureDevice;

    fn following() -> ControllerSettings {
        ControllerSettings {
            follow_capture: Some(true),
            error_recovery: None,
        }
    }

    fn base_config(samplerate: usize, channels: usize, extra: &str) -> Configuration {
        // Validation opens the capture file, so it has to exist on every platform.
        let capture_file = format!("{}/testdata/int32.raw", env!("CARGO_MANIFEST_DIR"));
        let yaml = format!(
            "devices:
  samplerate: {samplerate}
  chunksize: 1024
  capture:
    type: RawFile
    channels: {channels}
    filename: '{capture_file}'
    format: S32_LE
  playback:
    type: File
    channels: {channels}
    filename: /dev/null
    format: S32_LE
{extra}"
        );
        yaml_serde::from_str(&yaml).unwrap()
    }

    fn entry(conf: Configuration) -> ConfigSource {
        ConfigSource {
            raw: conf,
            filename: None,
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cdsp_controller_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn format(samplerate: usize, channels: Option<usize>) -> SourceFormat {
        SourceFormat {
            samplerate: Some(samplerate),
            channels,
            format: None,
        }
    }

    fn unknown_rate(channels: Option<usize>) -> SourceFormat {
        SourceFormat {
            samplerate: None,
            ..format(0, channels)
        }
    }

    fn run(selection: Selection) -> Box<Selected> {
        match selection {
            Selection::Run(selected) => selected,
            Selection::Invalid(err) => panic!("invalid: {err}"),
            Selection::NoConfig(f) => panic!("no config for {f}"),
        }
    }

    #[test]
    fn rates_snap_to_standard_rates() {
        assert_eq!(snap_rate(44097), 44100);
        assert_eq!(snap_rate(47900), 48000);
        assert_eq!(snap_rate(44581), 44100);
        assert_eq!(snap_rate(96000), 96000);
        // Far from any standard rate, kept as is.
        assert_eq!(snap_rate(60000), 60000);
    }

    #[test]
    fn without_a_format_the_entry_runs_as_is() {
        let selected = run(select_config(&entry(base_config(48000, 2, "")), None, None));
        assert_eq!(selected.provider, Provider::Single);
        assert_eq!(selected.config.devices.samplerate(), 48000);
    }

    #[test]
    fn an_invalid_entry_is_reported() {
        let conf = base_config(48000, 2, "  target_level: 100000\n");
        assert!(matches!(
            select_config(&entry(conf), None, None),
            Selection::Invalid(_)
        ));
    }

    #[test]
    fn a_sent_config_runs_as_sent() {
        let source = entry(base_config(48000, 2, ""));
        let loaded = LoadedConfig::validate(source, LoadKind::Websocket).unwrap();
        assert_eq!(loaded.kind, LoadKind::Websocket);
        let selected = select_websocket(loaded);
        assert_eq!(selected.provider, Provider::Websocket);
        assert_eq!(selected.format, None);
        assert_eq!(selected.config.devices.samplerate(), 48000);
    }

    #[test]
    fn a_patched_config_keeps_its_rate() {
        // An entry adapted to 96 kHz, then patched: it stays at 96 kHz, with the chunksize
        // Adapt scaled, rather than going back to the rate of the entry.
        let adapted = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(96000, Some(2))),
            None,
        ));
        let loaded = LoadedConfig::patched(adapted.config).unwrap();
        assert_eq!(loaded.kind, LoadKind::Websocket);
        assert_eq!(loaded.source.filename, None);
        let selected = select_websocket(loaded);
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.chunksize(), 2048);
    }

    #[test]
    fn adapt_without_resampler_changes_rate_and_chunksize() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(96000, Some(2))),
            None,
        ));
        assert_eq!(selected.provider, Provider::Adapt);
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.chunksize(), 2048);
    }

    #[test]
    fn adapt_with_resampler_changes_capture_rate() {
        let extra = "  resampler:\n    type: Synchronous\n  capture_samplerate: 44100\n";
        let selected = run(select_config(
            &entry(base_config(48000, 2, extra)),
            Some(&format(96000, None)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 48000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert!(selected.config.devices.resampler.is_some());
        // At 1:1 without rate adjust, the resampler is dropped.
        let selected = run(select_config(
            &entry(base_config(48000, 2, extra)),
            Some(&format(48000, None)),
            None,
        ));
        assert!(selected.config.devices.resampler.is_none());
    }

    #[test]
    fn adapt_snaps_a_measured_rate() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(44097, None)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 44100);
        assert_eq!(selected.format.unwrap().samplerate, Some(44100));
    }

    #[test]
    fn adapt_expands_tokens_at_the_new_rate() {
        let dir = temp_dir("adapt_tokens");
        for rate in [48000, 96000] {
            std::fs::write(dir.join(format!("filter_{rate}.txt")), "1.0\n0.0\n").unwrap();
        }
        let extra = format!(
            "filters:
  fir:
    type: Conv
    parameters:
      type: Raw
      format: TEXT
      filename: {}/filter_$samplerate$.txt
pipeline:
  - type: Filter
    channels: [0]
    names: [fir]
",
            dir.to_string_lossy()
        );
        let selected = run(select_config(
            &entry(base_config(48000, 2, &extra)),
            Some(&format(96000, None)),
            None,
        ));
        let filters = selected.config.filters.unwrap();
        let config::Filter::Conv {
            parameters: config::ConvParameters::Raw(params),
            ..
        } = &filters["fir"]
        else {
            panic!("not a raw conv");
        };
        assert!(params.filename.ends_with("filter_96000.txt"));
    }

    #[test]
    fn adapt_gives_nothing_for_unknown_rate_or_changed_channels() {
        let e = entry(base_config(48000, 2, ""));
        assert!(matches!(
            select_config(&e, Some(&unknown_rate(None)), None),
            Selection::NoConfig(_)
        ));
        assert!(matches!(
            select_config(&e, Some(&format(96000, Some(4))), None),
            Selection::NoConfig(_)
        ));
    }

    #[test]
    fn adapt_changes_an_explicit_format() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&SourceFormat {
                samplerate: Some(48000),
                channels: None,
                format: Some("S16_LE".to_string()),
            }),
            None,
        ));
        let CaptureDevice::RawFile(dev) = &selected.config.devices.capture else {
            panic!("not a raw file");
        };
        assert_eq!(dev.format, config::BinarySampleFormat::S16_LE);
    }

    #[test]
    fn adapt_refuses_a_format_the_backend_lacks() {
        let selected = select_config(
            &entry(base_config(48000, 2, "")),
            Some(&SourceFormat {
                samplerate: Some(48000),
                channels: None,
                format: Some("S24".to_string()),
            }),
            None,
        );
        assert!(matches!(selected, Selection::NoConfig(_)));
    }

    #[test]
    fn check_files_checks_the_entry_while_following() {
        let e = entry(base_config(44100, 2, ""));
        assert_eq!(
            check_files(&following(), Some(&e)).adapt,
            Some(AdaptCheck {
                file: None,
                samplerate: Some(44100),
                samplerates: None,
                channels: Some(2),
                format: Some("S32_LE".to_string()),
                problem: None,
            })
        );

        let no_entry = check_files(&following(), None).adapt.unwrap();
        assert_eq!(no_entry.samplerate, None);
        assert!(no_entry.problem.is_some());

        let off = check_files(&ControllerSettings::default(), Some(&e));
        assert_eq!(off.adapt, None);
    }

    fn with_rate_list(mut conf: Configuration, rates: &[usize]) -> Configuration {
        conf.devices.samplerate = config::SampleRates::List(
            rates
                .iter()
                .map(|r| NonZeroUsize::new(*r).unwrap())
                .collect(),
        );
        conf
    }

    #[test]
    fn a_listed_rate_runs_natively() {
        let extra = "  resampler:\n    type: Synchronous\n";
        let conf = with_rate_list(base_config(48000, 2, extra), &[48000, 44100, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(96000, None)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert_eq!(selected.config.devices.chunksize(), 2048);
        assert_eq!(selected.config.devices.resampler, None);
    }

    #[test]
    fn a_listed_rate_keeps_the_resampler_for_rate_adjust() {
        let extra = "  resampler:\n    type: Slip\n  enable_rate_adjust: true\n";
        let conf = with_rate_list(base_config(48000, 2, extra), &[48000, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(96000, None)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert_eq!(
            selected.config.devices.resampler,
            Some(config::Resampler::Slip)
        );
    }

    #[test]
    fn an_unlisted_rate_is_resampled_to_the_first() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[48000, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(44100, None)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 48000);
        assert_eq!(selected.config.devices.capture_samplerate(), 44100);
        assert_eq!(selected.config.devices.chunksize(), 1024);
        assert_eq!(
            selected.config.devices.resampler,
            Some(config::Resampler::Synchronous)
        );
    }

    #[test]
    fn a_rate_list_starts_at_the_first_rate() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[96000, 48000]);
        let selected = run(select_config(&entry(conf), None, None));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.resampler, None);
    }

    #[test]
    fn a_rate_list_parses_and_rejects_an_empty_one() {
        let parsed = |rates: &str| {
            let yaml = format!(
                "samplerate: {rates}\nchunksize: 1024\ncapture: {{type: Stdin, channels: 2, format: S16_LE}}\nplayback: {{type: Stdout, channels: 2, format: S16_LE}}\n"
            );
            yaml_serde::from_str::<config::Devices>(&yaml)
        };
        assert_eq!(parsed("44100").unwrap().samplerate(), 44100);
        let list = parsed("[96000, 44100]").unwrap();
        assert_eq!(list.samplerate(), 96000);
        assert_eq!(list.samplerate_list().unwrap().len(), 2);
        assert!(parsed("[]").is_err());
    }

    const DELAY_IN_SAMPLES: &str = "filters:
  delay:
    type: Delay
    parameters: {delay: 10, delay_unit: samples}
pipeline:
  - type: Filter
    channels: [0]
    names: [delay]
";

    fn adapt_runs(conf: Configuration, rate: usize) -> bool {
        match select_config(&entry(conf), Some(&format(rate, None)), None) {
            Selection::Run(_) => true,
            Selection::NoConfig(_) => false,
            Selection::Invalid(err) => panic!("invalid: {err}"),
        }
    }

    #[test]
    fn adapt_refuses_to_change_the_rate_of_rate_dependent_settings() {
        let conf = base_config(48000, 2, DELAY_IN_SAMPLES);
        assert!(!adapt_runs(conf.clone(), 96000));
        // At its own rate nothing changes.
        assert!(adapt_runs(conf, 48000));
        // A listed rate other than the first runs natively, at another rate.
        let listed = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000, 96000]);
        assert!(!adapt_runs(listed, 96000));
    }

    #[test]
    fn rate_dependent_settings_run_when_the_source_is_resampled() {
        let extra = format!("  resampler:\n    type: Synchronous\n{DELAY_IN_SAMPLES}");
        assert!(adapt_runs(base_config(48000, 2, &extra), 96000));
        let single = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000]);
        assert!(adapt_runs(single, 96000));
        let listed = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000, 96000]);
        assert!(adapt_runs(listed, 44100));
    }

    #[test]
    fn check_files_reports_rate_dependent_settings() {
        let problem = |conf| {
            check_files(&following(), Some(&entry(conf)))
                .adapt
                .unwrap()
                .problem
        };
        let reported = problem(base_config(48000, 2, DELAY_IN_SAMPLES)).unwrap();
        assert!(
            reported.contains("filter 'delay': delay in samples"),
            "{reported}"
        );
        let extra = format!("  resampler:\n    type: Synchronous\n{DELAY_IN_SAMPLES}");
        assert_eq!(problem(base_config(48000, 2, &extra)), None);
        let single = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000]);
        assert_eq!(problem(single), None);
    }

    #[test]
    fn check_files_lists_the_native_rates() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[96000, 48000]);
        let checks = check_files(&following(), Some(&entry(conf)));
        let adapt = checks.adapt.unwrap();
        assert_eq!(adapt.samplerate, Some(96000));
        assert_eq!(adapt.samplerates, Some(vec![96000, 48000]));
        assert_eq!(adapt.problem, None);
    }
}
