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

use crate::config::*;
use crate::fader;
use crate::filters;
use crate::filters::fftconv::ImpulseCache;
use crate::mixer;
use crate::processors::compressor;
use crate::processors::lookahead_limiter;
use crate::processors::noisegate;
use crate::processors::race;
use crate::utils::wavtools::find_data_in_wav_stream;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// Keep same result type used by config module utility functions.
type Res<T> = Result<T, Box<dyn error::Error>>;

/// Runtime overrides that replace specific fields in a loaded configuration.
#[derive(Clone)]
pub struct OverridesState {
    /// Override the capture/playback sample rate.
    pub samplerate: Option<usize>,
    /// Override the sample format for binary backends.
    pub sample_format: Option<BinarySampleFormat>,
    /// Override the number of extra (silent) samples the capture device prepends.
    pub extra_samples: Option<usize>,
    /// Override the number of capture/playback channels.
    pub channels: Option<usize>,
}

pub static OVERRIDES: LazyLock<RwLock<OverridesState>> = LazyLock::new(|| {
    RwLock::new(OverridesState {
        samplerate: None,
        sample_format: None,
        extra_samples: None,
        channels: None,
    })
});

/// Error type for configuration parsing and validation failures.
#[derive(Debug)]
pub struct ConfigErrorType {
    desc: String,
}

impl fmt::Display for ConfigErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl error::Error for ConfigErrorType {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl ConfigErrorType {
    /// Create a new config error with the given description message.
    pub fn new(desc: &str) -> Self {
        ConfigErrorType {
            desc: desc.to_owned(),
        }
    }
}

/// Reject a non-finite value that did not come from the config file.
///
/// Coefficients read from a raw or wav file cannot be checked while deserializing, since they
/// never pass through serde. This is the equivalent check for them.
pub fn check_all_finite<T: Into<f64> + Copy>(name: &str, values: &[T]) -> Res<()> {
    for (index, value) in values.iter().enumerate() {
        let value: f64 = (*value).into();
        if !value.is_finite() {
            let msg = format!(
                "Value for '{name}' at index {index} must be a finite number, got {value}."
            );
            return Err(ConfigError::new(&msg).into());
        }
    }
    Ok(())
}

/// Validate the resampler parameters that rubato does not check itself.
///
/// Only the free `AsyncSinc` parameters can be wrong, the profiles are fixed and the other
/// resamplers take no parameters of their own.
///
/// Issue paths are relative to the resampler.
fn validate_resampler(resampler: &Option<Resampler>) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let Some(Resampler::AsyncSinc(AsyncSincParameters::Free {
        sinc_len,
        interpolation,
        f_cutoff,
        oversampling_factor,
        ..
    })) = resampler
    else {
        return Ok(());
    };
    // Checked here rather than with a `NonZeroUsize` field, since `AsyncSincParameters` is
    // untagged and a rejected field there only reports that no variant matched.
    if *sinc_len == 0 {
        let msg = "sinc_len must be larger than zero, 64 to 256 are typical values.";
        issues.invalid(issue_path!["sinc_len"], msg);
    }
    // Rubato fits a polynomial through a number of neighbouring sincs, and wraps an index that
    // runs past the end of the table only once. Fitting n points therefore needs a table of at
    // least n - 1, and anything smaller indexes out of bounds and panics.
    let min_oversampling = match interpolation {
        AsyncSincInterpolation::Nearest | AsyncSincInterpolation::Linear => 1,
        AsyncSincInterpolation::Quadratic => 2,
        AsyncSincInterpolation::Cubic => 3,
    };
    if *oversampling_factor < min_oversampling {
        let msg = format!(
            "oversampling_factor must be at least {min_oversampling} for {interpolation:?} interpolation, got {oversampling_factor}. \
             Values in the hundreds are normal, see the profiles for typical settings."
        );
        issues.invalid(issue_path!["oversampling_factor"], msg);
    }
    if let Some(cutoff) = f_cutoff
        && !(0.0 < *cutoff && *cutoff <= 1.0)
    {
        let msg = format!(
            "f_cutoff must be larger than 0 and no larger than 1.0, got {cutoff}. \
             It is relative to the Nyquist limit, useful values are in the range 0.9 - 0.99."
        );
        issues.invalid(issue_path!["f_cutoff"], msg);
    }
    issues.into_result(())
}

/// Deserialize a configuration, reporting a structural error with its path.
///
/// Works with any serde format, so a config can be read from JSON as well as
/// YAML. Deserialization stops at the first error. Inside a filter, mixer or
/// processor the path only reaches the item itself, since serde reads tagged
/// enums ahead into a buffer and loses track of where it is in them.
pub fn deserialize_config<'de, D>(deserializer: D) -> Result<Configuration, Issue>
where
    D: serde::Deserializer<'de>,
{
    serde_path_to_error::deserialize(deserializer).map_err(|err| {
        let path: Vec<PathElement> = err
            .path()
            .iter()
            .filter_map(|segment| match segment {
                serde_path_to_error::Segment::Map { key } => Some(PathElement::from(key)),
                serde_path_to_error::Segment::Seq { index } => Some(PathElement::from(*index)),
                serde_path_to_error::Segment::Enum { .. }
                | serde_path_to_error::Segment::Unknown => None,
            })
            .collect();
        let message = strip_path_prefix(&err.inner().to_string(), &path);
        Issue::invalid(path, message)
    })
}

/// The YAML parser starts its messages with the path where it is, which may be
/// shorter than the one `serde_path_to_error` found. Drop it, the issue has its own.
fn strip_path_prefix(message: &str, path: &[PathElement]) -> String {
    for len in (1..=path.len()).rev() {
        let prefix = format!("{}: ", format_path(&path[..len]));
        if let Some(rest) = message.strip_prefix(&prefix) {
            return rest.to_string();
        }
    }
    message.to_string()
}

/// Parse a configuration from a YAML string.
pub fn parse_config(yaml: &str) -> Result<Configuration, Issue> {
    deserialize_config(yaml_serde::Deserializer::from_str(yaml))
}

/// Read and parse a YAML configuration file.
pub fn load_config(filename: &str) -> Result<Configuration, Issues> {
    let file = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("Could not open config file '{filename}'. Reason: {err}");
            return Err(Issue::invalid(issue_path![], msg).into());
        }
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(err) => {
            let msg = format!("Could not read config file '{filename}'. Reason: {err}");
            return Err(Issue::invalid(issue_path![], msg).into());
        }
    };
    Ok(parse_config(&contents)?)
}

fn apply_overrides(configuration: &mut Configuration) -> Res<()> {
    let mut overrides = OVERRIDES.read().clone();
    // Only one match arm for now, might be more later.
    #[allow(clippy::single_match)]
    match &configuration.devices.capture {
        CaptureDevice::WavFile(dev) => {
            if let Ok(wav_info) = dev.wav_info() {
                overrides.channels = Some(wav_info.channels);
                overrides.sample_format = Some(wav_info.sample_format);
                overrides.samplerate = Some(wav_info.sample_rate);
                debug!(
                    "Updating overrides with values from wav input file, rate {}, format: {}, channels: {}",
                    wav_info.sample_rate, wav_info.sample_format, wav_info.channels
                );
            }
        }
        _ => {}
    }
    if let Some(rate) = overrides.samplerate {
        let Some(rate_nonzero) = NonZeroUsize::new(rate) else {
            return Err(
                ConfigError::new("The samplerate override must be larger than zero").into(),
            );
        };
        let cfg_rate = configuration.devices.samplerate();
        let cfg_chunksize = configuration.devices.chunksize();

        if configuration.devices.resampler.is_none() {
            debug!("Apply override for samplerate: {rate}");
            configuration.devices.samplerate = rate_nonzero;
            let scaled_chunksize = if rate > cfg_rate {
                cfg_chunksize * (rate as f32 / cfg_rate as f32).round() as usize
            } else {
                cfg_chunksize / (cfg_rate as f32 / rate as f32).round() as usize
            };
            // Scaling down is an integer division, so a small enough chunksize
            // divides away entirely. Zero would hang the capture loop, and a
            // configuration this odd should still run, so keep one frame.
            let scaled_chunksize = NonZeroUsize::new(scaled_chunksize).unwrap_or_else(|| {
                warn!(
                    "Overriding the samplerate to {rate} scales chunksize {cfg_chunksize} below one frame, using 1"
                );
                NonZeroUsize::MIN
            });
            debug!(
                "Samplerate changed, adjusting chunksize: {cfg_chunksize} -> {scaled_chunksize}"
            );
            configuration.devices.chunksize = scaled_chunksize;
            #[allow(unreachable_patterns)]
            match &mut configuration.devices.capture {
                CaptureDevice::RawFile(dev) => {
                    let new_extra = dev.extra_samples() * rate / cfg_rate;
                    debug!(
                        "Scale extra samples: {} -> {}",
                        dev.extra_samples(),
                        new_extra
                    );
                    dev.extra_samples = Some(new_extra);
                }
                CaptureDevice::Stdin(dev) => {
                    let new_extra = dev.extra_samples() * rate / cfg_rate;
                    debug!(
                        "Scale extra samples: {} -> {}",
                        dev.extra_samples(),
                        new_extra
                    );
                    dev.extra_samples = Some(new_extra);
                }
                _ => {}
            }
        } else {
            debug!("Apply override for capture_samplerate: {rate}");
            configuration.devices.capture_samplerate = Some(rate_nonzero);
            if rate == cfg_rate && !configuration.devices.rate_adjust() {
                debug!("Disabling unneccesary 1:1 resampling");
                configuration.devices.resampler = None;
            }
        }
    }
    if let Some(extra) = overrides.extra_samples {
        debug!("Apply override for extra_samples: {extra}");
        #[allow(unreachable_patterns)]
        match &mut configuration.devices.capture {
            CaptureDevice::RawFile(dev) => {
                dev.extra_samples = Some(extra);
            }
            CaptureDevice::Stdin(dev) => {
                dev.extra_samples = Some(extra);
            }
            _ => {}
        }
    }
    if let Some(chans) = overrides.channels {
        debug!("Apply override for capture channels: {chans}");
        let Some(chans) = NonZeroUsize::new(chans) else {
            return Err(ConfigError::new("The channels override must be larger than zero").into());
        };
        match &mut configuration.devices.capture {
            CaptureDevice::RawFile(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::WavFile(_dev) => {}
            CaptureDevice::Stdin(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::Alsa { channels, .. } => {
                *channels = chans;
            }
            CaptureDevice::PipeWire { channels, .. } => {
                *channels = chans;
            }
            CaptureDevice::CoreAudio(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::Wasapi(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::Asio(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::SignalGenerator { channels, .. } => {
                *channels = chans;
            }
            CaptureDevice::Dummy { channels, .. } => {
                *channels = chans;
            }
        }
    }
    if let Some(fmt) = overrides.sample_format {
        debug!("Apply override for capture sample format: {fmt}");
        match &mut configuration.devices.capture {
            CaptureDevice::RawFile(dev) => {
                dev.format = fmt;
            }
            CaptureDevice::WavFile(_dev) => {}
            CaptureDevice::Stdin(dev) => {
                dev.format = fmt;
            }
            CaptureDevice::Alsa { format, .. } => {
                let mapped_format = AlsaSampleFormat::from_binary_format(&fmt);
                *format = Some(mapped_format);
            }
            CaptureDevice::PipeWire { .. } => {
                error!("Not possible to override capture format for PipeWire, ignoring");
            }
            CaptureDevice::CoreAudio(dev) => {
                let mapped_format = CoreAudioSampleFormat::from_binary_format(&fmt);
                if let Some(mapped) = mapped_format {
                    dev.format = Some(mapped);
                } else {
                    let msg =
                        format!("CoreAudio does not have a sample format corresponding to {fmt}");
                    return Err(ConfigError::new(&msg).into());
                }
            }
            CaptureDevice::Wasapi(dev) => {
                let mapped_format = WasapiSampleFormat::from_binary_format(&fmt);
                if let Some(mapped) = mapped_format {
                    dev.format = Some(mapped);
                } else {
                    let msg =
                        format!("Wasapi does not have a sample format corresponding to {fmt}");
                    return Err(ConfigError::new(&msg).into());
                }
            }
            CaptureDevice::Asio(dev) => {
                let mapped_format = AsioSampleFormat::from_binary_format(&fmt);
                if let Some(mapped) = mapped_format {
                    dev.format = Some(mapped);
                } else {
                    let msg = format!("ASIO does not have a sample format corresponding to {fmt}");
                    return Err(ConfigError::new(&msg).into());
                }
            }
            CaptureDevice::SignalGenerator { .. } => {}
            CaptureDevice::Dummy { .. } => {}
        }
    }
    Ok(())
}

fn replace_tokens(string: &str, samplerate: usize, channels: usize) -> String {
    let srate = format!("{samplerate}");
    let ch = format!("{channels}");
    string
        .replace("$samplerate$", &srate)
        .replace("$channels$", &ch)
}

fn replace_tokens_in_config(config: &mut Configuration) {
    let samplerate = config.devices.samplerate();
    let num_channels = config.devices.capture.channels();
    if let Some(filters) = &mut config.filters {
        for filter in filters.values_mut() {
            match filter {
                Filter::Conv {
                    parameters: ConvParameters::Raw(params),
                    ..
                } => {
                    params.filename = replace_tokens(&params.filename, samplerate, num_channels);
                }
                Filter::Conv {
                    parameters: ConvParameters::Wav(params),
                    ..
                } => {
                    params.filename = replace_tokens(&params.filename, samplerate, num_channels);
                }
                _ => {}
            }
        }
    }
    if let Some(pipeline) = &mut config.pipeline {
        for mut step in pipeline.iter_mut() {
            match &mut step {
                PipelineStep::Filter(step) => {
                    for name in step.names.iter_mut() {
                        *name = replace_tokens(name, samplerate, num_channels);
                    }
                }
                PipelineStep::Mixer(step) => {
                    step.name = replace_tokens(&step.name, samplerate, num_channels);
                }
                PipelineStep::Processor(step) => {
                    step.name = replace_tokens(&step.name, samplerate, num_channels);
                }
            }
        }
    }
}

// Check if coefficent files with relative paths are relative to the config file path, replace path if they are
fn replace_relative_paths_in_config(config: &mut Configuration, configname: &str) {
    if let Ok(config_file) = PathBuf::from(configname.to_owned()).canonicalize() {
        if let Some(config_dir) = config_file.parent() {
            if let Some(filters) = &mut config.filters {
                for filter in filters.values_mut() {
                    if let Filter::Conv {
                        parameters: ConvParameters::Raw(params),
                        ..
                    } = filter
                    {
                        check_and_replace_relative_path(&mut params.filename, config_dir);
                    } else if let Filter::Conv {
                        parameters: ConvParameters::Wav(params),
                        ..
                    } = filter
                    {
                        check_and_replace_relative_path(&mut params.filename, config_dir);
                    }
                }
            }
        } else {
            warn!("Can't find parent directory of config file");
        }
    } else {
        warn!("Can't find absolute path of config file");
    }
}

fn check_and_replace_relative_path(path_str: &mut String, config_path: &Path) {
    let path = PathBuf::from(path_str.to_owned());
    if path.is_absolute() {
        trace!("{path_str} is absolute, no change");
    } else {
        debug!("{path_str} is relative");
        let mut in_config_dir = config_path.to_path_buf();
        in_config_dir.push(&path_str);
        if in_config_dir.exists() {
            debug!("Using {path_str} found relative to config file dir");
            *path_str = in_config_dir.to_string_lossy().into();
        } else {
            trace!("{path_str} not found relative to config file dir, not changing path");
        }
    }
}

/// Parse, apply overrides, and fully validate a configuration file.
pub fn load_validate_config(configname: &str) -> Result<(Configuration, ImpulseCache), Issues> {
    let mut configuration = load_config(configname)?;
    let impulses = validate_config(&mut configuration, Some(configname))?;
    Ok((configuration, impulses))
}

/// Compare two configurations and return the most significant [`ConfigChange`] between them.
pub fn config_diff(currentconf: &Configuration, newconf: &Configuration) -> ConfigChange {
    if currentconf == newconf {
        return ConfigChange::None;
    }
    if currentconf.devices != newconf.devices {
        return ConfigChange::Devices;
    }
    if currentconf.pipeline != newconf.pipeline {
        return ConfigChange::Pipeline;
    }
    if currentconf.mixers != newconf.mixers {
        return ConfigChange::MixerParameters;
    }
    let mut filters = Vec::<String>::new();
    let mut processors = Vec::<String>::new();
    if let (Some(newfilters), Some(oldfilters)) = (&newconf.filters, &currentconf.filters) {
        for (filter, params) in newfilters {
            // The pipeline didn't change, any added filter isn't included and can be skipped
            if let Some(current_filter) = oldfilters.get(filter) {
                // Did the filter change type?
                match (params, current_filter) {
                    (Filter::Biquad { .. }, Filter::Biquad { .. })
                    | (Filter::BiquadCombo { .. }, Filter::BiquadCombo { .. })
                    | (Filter::Conv { .. }, Filter::Conv { .. })
                    | (Filter::Delay { .. }, Filter::Delay { .. })
                    | (Filter::Gain { .. }, Filter::Gain { .. })
                    | (Filter::Dither { .. }, Filter::Dither { .. })
                    | (Filter::DiffEq { .. }, Filter::DiffEq { .. })
                    | (Filter::Volume { .. }, Filter::Volume { .. })
                    | (Filter::Loudness { .. }, Filter::Loudness { .. })
                    | (Filter::Clipper { .. }, Filter::Clipper { .. }) => {}
                    _ => {
                        // A filter changed type, need to rebuild the pipeline
                        return ConfigChange::Pipeline;
                    }
                };
                // Only parameters changed, ok to update
                if params != current_filter {
                    filters.push(filter.to_string());
                }
            }
        }
    }
    if let (Some(newprocs), Some(oldprocs)) = (&newconf.processors, &currentconf.processors) {
        for (proc, params) in newprocs {
            // The pipeline didn't change, any added processor isn't included and can be skipped
            if let Some(current_proc) = oldprocs.get(proc) {
                // Did the processor change type?
                match (params, current_proc) {
                    (Processor::Compressor { .. }, Processor::Compressor { .. })
                    | (Processor::NoiseGate { .. }, Processor::NoiseGate { .. })
                    | (Processor::LookaheadLimiter { .. }, Processor::LookaheadLimiter { .. })
                    | (Processor::RACE { .. }, Processor::RACE { .. }) => {}
                    _ => {
                        // A processor changed type, need to rebuild the pipeline
                        return ConfigChange::Pipeline;
                    }
                };
                // Only parameters changed, ok to update
                if params != current_proc {
                    processors.push(proc.to_string());
                }
            }
        }
    }
    ConfigChange::FilterParameters {
        filters,
        processors,
    }
}

/// Validate the loaded configuration, collecting every issue found.
///
/// Checking carries on after an issue wherever the rest still makes sense, so
/// the result lists all of them, each with its location. Where it does not, for
/// example the channel counts after a missing mixer, those checks are skipped.
/// Any issue at all makes the config invalid for CamillaDSP.
///
/// Returns the impulse responses of the convolution filters the pipeline uses,
/// read as part of validating them. Pass it along with the configuration to
/// whatever applies it and nothing has to read a coefficient file again; drop
/// it if the configuration is only being checked. See
/// [`ImpulseCache`](crate::filters::fftconv::ImpulseCache).
pub fn validate_config(
    conf: &mut Configuration,
    filename: Option<&str>,
) -> Result<ImpulseCache, Issues> {
    let mut issues = Issues::new();
    let mut impulses = ImpulseCache::new();
    // pre-process by applying overrides and replacing tokens
    if let Err(err) = apply_overrides(conf) {
        // The overrides come from the command line, and without them the rest
        // of the config cannot be checked as it will run.
        return Err(Issue::invalid(issue_path![], err.to_string()).into());
    }
    replace_tokens_in_config(conf);
    if let Some(fname) = filename {
        replace_relative_paths_in_config(conf, fname);
    }
    validate_devices(conf, &mut issues);
    validate_pipeline(conf, &mut impulses, &mut issues);
    issues.nest_result(Vec::new(), fader::validate_fader_settings(conf));
    issues.into_result(impulses)
}

/// An issue for a file that could not be opened. One that does not exist is a
/// [`IssueKind::MissingFile`], anything else means the config is invalid.
fn file_issue(path: Vec<PathElement>, filename: &str, message: String) -> Issue {
    if matches!(Path::new(filename).try_exists(), Ok(false)) {
        Issue::missing_file(path, message)
    } else {
        Issue::invalid(path, message)
    }
}

fn validate_devices(conf: &Configuration, issues: &mut Issues) {
    if !conf.devices.capture.is_supported() {
        let msg = format!(
            "The {} capture device type is not supported by this build",
            conf.devices.capture.type_name()
        );
        issues.push(Issue::unsupported(
            issue_path!["devices", "capture", "type"],
            msg,
        ));
    }
    if !conf.devices.playback.is_supported() {
        let msg = format!(
            "The {} playback device type is not supported by this build",
            conf.devices.playback.type_name()
        );
        issues.push(Issue::unsupported(
            issue_path!["devices", "playback", "type"],
            msg,
        ));
    }
    issues.nest_result(
        issue_path!["devices", "resampler"],
        validate_resampler(&conf.devices.resampler),
    );
    let target_level_limit = if matches!(conf.devices.playback, PlaybackDevice::Alsa { .. }) {
        (4 + conf.devices.queuelimit()) * conf.devices.chunksize()
    } else {
        (2 + conf.devices.queuelimit()) * conf.devices.chunksize()
    };

    if conf.devices.target_level() > target_level_limit {
        let msg = format!("target_level cannot be larger than {target_level_limit}");
        issues.invalid(issue_path!["devices", "target_level"], msg);
    }
    if let Some(interval) = conf.devices.adjust_interval_s
        && interval <= 0.0
    {
        issues.invalid(
            issue_path!["devices", "adjust_interval_s"],
            "adjust_interval_s must be positive and > 0",
        );
    }
    if let Some(interval) = conf.devices.rate_measure_interval_s
        && interval <= 0.0
    {
        issues.invalid(
            issue_path!["devices", "rate_measure_interval_s"],
            "rate_measure_interval_s must be positive and > 0",
        );
    }
    if let Some(threshold) = conf.devices.silence_threshold
        && threshold > 0.0
    {
        issues.invalid(
            issue_path!["devices", "silence_threshold"],
            "silence_threshold must be less than or equal to 0",
        );
    }
    if let Some(timeout) = conf.devices.silence_timeout_s
        && timeout < 0.0
    {
        issues.invalid(
            issue_path!["devices", "silence_timeout_s"],
            "silence_timeout_s cannot be negative",
        );
    }
    if conf.devices.volume_ramp_time_ms() < 0.0 {
        issues.invalid(
            issue_path!["devices", "volume_ramp_time_ms"],
            "Volume ramp time cannot be negative",
        );
    }
    if conf.devices.volume_limit() > 50.0 {
        issues.invalid(
            issue_path!["devices", "volume_limit"],
            "Volume limit cannot be above +50 dB",
        );
    }
    if conf.devices.volume_limit() < -150.0 {
        issues.invalid(
            issue_path!["devices", "volume_limit"],
            "Volume limit cannot be less than -150 dB",
        );
    }
    if matches!(conf.devices.resampler, Some(Resampler::Slip))
        && conf.devices.capture_samplerate() != conf.devices.samplerate()
    {
        issues.invalid(
            issue_path!["devices", "resampler"],
            "The Slip resampler requires matching samplerate and capture_samplerate",
        );
    }
    if let CaptureDevice::Wasapi(dev) = &conf.devices.capture
        && let Some(format) = dev.format
        && format != WasapiSampleFormat::F32
        && !dev.is_exclusive()
    {
        issues.invalid(
            issue_path!["devices", "capture", "format"],
            "Wasapi shared mode capture must use F32 sample format",
        );
    }
    if let CaptureDevice::Wasapi(dev) = &conf.devices.capture
        && dev.is_loopback()
        && dev.is_exclusive()
    {
        issues.invalid(
            issue_path!["devices", "capture"],
            "Wasapi loopback capture is only supported in shared mode",
        );
    }
    if let PlaybackDevice::Wasapi(dev) = &conf.devices.playback
        && let Some(format) = dev.format
        && format != WasapiSampleFormat::F32
        && !dev.is_exclusive()
    {
        issues.invalid(
            issue_path!["devices", "playback", "format"],
            "Wasapi shared mode playback must use F32 sample format",
        );
    }
    if let (CaptureDevice::Asio(cap_dev), PlaybackDevice::Asio(pb_dev)) =
        (&conf.devices.capture, &conf.devices.playback)
    {
        // Capture and playback on the same device share a single driver instance, and
        // therefore a single clock and sample rate, so there is nothing to resample
        // between. Different devices are independent and resample like any other pair.
        if cap_dev.device == pb_dev.device && conf.devices.resampler.is_some() {
            issues.invalid(
                issue_path!["devices", "resampler"],
                "Resampling is not supported in full-duplex ASIO mode. \
                 Both capture and playback share the same driver and sample rate",
            );
        }
    }
    if let PlaybackDevice::File {
        format, wav_header, ..
    } = &conf.devices.playback
        && *format == BinarySampleFormat::S24_4_RJ_LE
        && *wav_header == Some(true)
    {
        issues.invalid(
            issue_path!["devices", "playback", "format"],
            "Wav files do not support the S24_4_RJ_LE sample format",
        );
    }
    validate_device_names(conf, issues);
    // An empty file name is reported by `validate_device_names`.
    if let CaptureDevice::RawFile(dev) = &conf.devices.capture
        && !dev.filename.is_empty()
    {
        let fname = &dev.filename;
        if let Err(err) = File::open(fname) {
            let msg = format!("Could not open input file '{fname}'. Reason: {err}");
            issues.push(file_issue(
                issue_path!["devices", "capture", "filename"],
                fname,
                msg,
            ));
        }
    }
    if let CaptureDevice::WavFile(dev) = &conf.devices.capture
        && !dev.filename.is_empty()
    {
        let fname = &dev.filename;
        match File::open(fname) {
            Ok(f) => {
                let file = BufReader::new(&f);
                if let Err(err) = find_data_in_wav_stream(file) {
                    let msg = format!("Error reading wav file '{fname}'. Reason: {err}");
                    issues.invalid(issue_path!["devices", "capture", "filename"], msg);
                }
            }
            Err(err) => {
                let msg = format!("Could not open input file '{fname}'. Reason: {err}");
                issues.push(file_issue(
                    issue_path!["devices", "capture", "filename"],
                    fname,
                    msg,
                ));
            }
        }
    }
}

/// Check that the device and file names the backends look up are not empty.
///
/// An empty name would otherwise only fail when the device is opened. The
/// optional ones are only checked when given.
fn validate_device_names(conf: &Configuration, issues: &mut Issues) {
    let mut check = |side: &str, field: &str, value: Option<&String>| {
        if value.is_some_and(|value| value.is_empty()) {
            issues.invalid(issue_path!["devices", side, field], "Must not be empty");
        }
    };
    match &conf.devices.capture {
        CaptureDevice::Alsa {
            device,
            link_volume_control,
            link_mute_control,
            ..
        } => {
            check("capture", "device", Some(device));
            check(
                "capture",
                "link_volume_control",
                link_volume_control.as_ref(),
            );
            check("capture", "link_mute_control", link_mute_control.as_ref());
        }
        CaptureDevice::PipeWire {
            node_name,
            node_description,
            node_group_name,
            ..
        } => {
            check("capture", "node_name", node_name.as_ref());
            check("capture", "node_description", node_description.as_ref());
            check("capture", "node_group_name", node_group_name.as_ref());
        }
        CaptureDevice::RawFile(dev) => check("capture", "filename", Some(&dev.filename)),
        CaptureDevice::WavFile(dev) => check("capture", "filename", Some(&dev.filename)),
        CaptureDevice::CoreAudio(dev) => check("capture", "device", dev.device.as_ref()),
        CaptureDevice::Wasapi(dev) => check("capture", "device", dev.device.as_ref()),
        CaptureDevice::Asio(dev) => check("capture", "device", Some(&dev.device)),
        CaptureDevice::Stdin(_)
        | CaptureDevice::SignalGenerator { .. }
        | CaptureDevice::Dummy { .. } => {}
    }
    match &conf.devices.playback {
        PlaybackDevice::Alsa { device, .. } => check("playback", "device", Some(device)),
        PlaybackDevice::PipeWire {
            node_name,
            node_description,
            node_group_name,
            ..
        } => {
            check("playback", "node_name", node_name.as_ref());
            check("playback", "node_description", node_description.as_ref());
            check("playback", "node_group_name", node_group_name.as_ref());
        }
        PlaybackDevice::File { filename, .. } => check("playback", "filename", Some(filename)),
        PlaybackDevice::CoreAudio(dev) => check("playback", "device", dev.device.as_ref()),
        PlaybackDevice::Wasapi(dev) => check("playback", "device", dev.device.as_ref()),
        PlaybackDevice::Asio(dev) => check("playback", "device", Some(&dev.device)),
        PlaybackDevice::Stdout { .. } | PlaybackDevice::Dummy { .. } => {}
    }
}

/// Walk the pipeline, checking that every step refers to something that exists,
/// that the channel counts line up, and that what each step uses is valid.
///
/// Each filter, mixer and processor is checked once, however many steps use it,
/// and its issues are placed under its own definition. The channel counts are no
/// longer known after a missing mixer, so they are not checked from there on.
fn validate_pipeline(conf: &Configuration, impulses: &mut ImpulseCache, issues: &mut Issues) {
    let mut num_channels = Some(conf.devices.capture.channels());
    let fs = conf.devices.samplerate();
    let mut checked_mixers = HashSet::new();
    let mut checked_filters = HashSet::new();
    let mut checked_processors = HashSet::new();
    if let Some(pipeline) = &conf.pipeline {
        for (idx, step) in pipeline.iter().enumerate() {
            match step {
                PipelineStep::Mixer(step) => {
                    if step.is_bypassed() {
                        continue;
                    }
                    let Some(mixer) = conf.mixers.as_ref().and_then(|m| m.get(&step.name)) else {
                        let msg = format!("Use of missing mixer '{}'", step.name);
                        issues.invalid(issue_path!["pipeline", idx, "name"], msg);
                        num_channels = None;
                        continue;
                    };
                    let chan_in = mixer.channels.input();
                    if let Some(expected) = num_channels
                        && chan_in != expected
                    {
                        let msg = format!(
                            "Mixer '{}' has wrong number of input channels. Expected {}, found {}.",
                            step.name, expected, chan_in
                        );
                        issues.invalid(issue_path!["pipeline", idx], msg);
                    }
                    num_channels = Some(mixer.channels.output());
                    if checked_mixers.insert(&step.name) {
                        issues.nest_result(
                            issue_path!["mixers", &step.name],
                            mixer::validate_mixer(mixer),
                        );
                    }
                }
                PipelineStep::Filter(step) => {
                    if step.is_bypassed() {
                        continue;
                    }
                    if let Some(channels) = &step.channels {
                        if let Some(available) = num_channels {
                            for channel in channels {
                                if *channel >= available {
                                    let msg = format!("Use of non existing channel {channel}");
                                    issues.invalid(issue_path!["pipeline", idx, "channels"], msg);
                                }
                            }
                        }
                        let mut duplicated = Vec::new();
                        for n in 1..channels.len() {
                            let channel = channels[n - 1];
                            if channels[n..].contains(&channel) && !duplicated.contains(&channel) {
                                duplicated.push(channel);
                                let msg = format!("Use of duplicated channel {channel}");
                                issues.invalid(issue_path!["pipeline", idx, "channels"], msg);
                            }
                        }
                    }
                    for (n, name) in step.names.iter().enumerate() {
                        let Some(filter) = conf.filters.as_ref().and_then(|f| f.get(name)) else {
                            let msg = format!("Use of missing filter '{name}'");
                            issues.invalid(issue_path!["pipeline", idx, "names", n], msg);
                            continue;
                        };
                        if checked_filters.insert(name) {
                            issues.nest_result(
                                issue_path!["filters", name],
                                filters::validate_filter(fs, name, filter, impulses),
                            );
                        }
                    }
                }
                PipelineStep::Processor(step) => {
                    if step.is_bypassed() {
                        continue;
                    }
                    let Some(procconf) = conf.processors.as_ref().and_then(|p| p.get(&step.name))
                    else {
                        let msg = format!("Use of missing processor '{}'", step.name);
                        issues.invalid(issue_path!["pipeline", idx, "name"], msg);
                        continue;
                    };
                    let (kind, channels) = match procconf {
                        Processor::Compressor { parameters, .. } => {
                            ("Compressor", parameters.channels)
                        }
                        Processor::NoiseGate { parameters, .. } => {
                            ("NoiseGate", parameters.channels)
                        }
                        Processor::LookaheadLimiter { parameters, .. } => {
                            ("LookaheadLimiter", parameters.channels)
                        }
                        Processor::RACE { parameters, .. } => {
                            ("RACE processor", parameters.channels)
                        }
                    };
                    if let Some(expected) = num_channels
                        && channels != expected
                    {
                        let msg = format!(
                            "{kind} '{}' has wrong number of channels. Expected {}, found {}.",
                            step.name, expected, channels
                        );
                        issues.invalid(issue_path!["pipeline", idx], msg);
                    }
                    if checked_processors.insert(&step.name) {
                        issues.nest_result(
                            issue_path!["processors", &step.name],
                            validate_processor(fs, procconf),
                        );
                    }
                }
            }
        }
    }
    let num_channels_out = conf.devices.playback.channels();
    if let Some(num_channels) = num_channels
        && num_channels != num_channels_out
    {
        let msg = format!(
            "Pipeline outputs {num_channels} channels, playback device has {num_channels_out}."
        );
        issues.invalid(issue_path!["pipeline"], msg);
    }
}

/// Validate the parameters of a processor. Issue paths are relative to the processor.
pub fn validate_processor(fs: usize, procconf: &Processor) -> Result<(), Issues> {
    let result = match procconf {
        Processor::Compressor { parameters, .. } => compressor::validate_compressor(parameters),
        Processor::NoiseGate { parameters, .. } => noisegate::validate_noise_gate(parameters),
        Processor::LookaheadLimiter { parameters, .. } => {
            lookahead_limiter::validate_lookahead_limiter(parameters, fs)
        }
        Processor::RACE { parameters, .. } => race::validate_race(parameters),
    };
    let mut issues = Issues::new();
    issues.nest_result(issue_path!["parameters"], result);
    issues.into_result(())
}

/// The names of the filters, mixers and processors that the pipeline uses.
///
/// These are exactly the ones [`validate_config`] checks: everything named by a
/// step that is not bypassed.
fn used_names(conf: &Configuration) -> [HashSet<&str>; 3] {
    let mut filters = HashSet::new();
    let mut mixers = HashSet::new();
    let mut processors = HashSet::new();
    for step in conf.pipeline.iter().flatten() {
        match step {
            PipelineStep::Mixer(step) if !step.is_bypassed() => {
                mixers.insert(step.name.as_str());
            }
            PipelineStep::Filter(step) if !step.is_bypassed() => {
                filters.extend(step.names.iter().map(String::as_str));
            }
            PipelineStep::Processor(step) if !step.is_bypassed() => {
                processors.insert(step.name.as_str());
            }
            _ => {}
        }
    }
    [filters, mixers, processors]
}

/// Validate the filters, mixers and processors that the pipeline does not use.
///
/// [`validate_config`] only checks what the pipeline uses, so a broken
/// definition that nothing refers to, or that only a bypassed step refers to,
/// does not stop CamillaDSP. A config editor still wants to hear about it, and
/// can call this as well. Call it after `validate_config`, on the same
/// configuration, so that tokens, relative paths and overrides are already
/// applied. The issue paths are the same as `validate_config` would give, and
/// checking a convolution filter reads its coefficient file.
pub fn validate_unused(conf: &Configuration) -> Result<(), Issues> {
    let [used_filters, used_mixers, used_processors] = used_names(conf);
    let fs = conf.devices.samplerate();
    let mut issues = Issues::new();
    // Sorted by name, since the definitions are in hash maps.
    if let Some(filters) = &conf.filters {
        let mut impulses = ImpulseCache::new();
        let mut unused: Vec<_> = filters
            .iter()
            .filter(|(name, _)| !used_filters.contains(name.as_str()))
            .collect();
        unused.sort_by_key(|(name, _)| *name);
        for (name, filter) in unused {
            issues.nest_result(
                issue_path!["filters", name],
                filters::validate_filter(fs, name, filter, &mut impulses),
            );
        }
    }
    if let Some(mixers) = &conf.mixers {
        let mut unused: Vec<_> = mixers
            .iter()
            .filter(|(name, _)| !used_mixers.contains(name.as_str()))
            .collect();
        unused.sort_by_key(|(name, _)| *name);
        for (name, mixer) in unused {
            issues.nest_result(issue_path!["mixers", name], mixer::validate_mixer(mixer));
        }
    }
    if let Some(processors) = &conf.processors {
        let mut unused: Vec<_> = processors
            .iter()
            .filter(|(name, _)| !used_processors.contains(name.as_str()))
            .collect();
        unused.sort_by_key(|(name, _)| *name);
        for (name, procconf) in unused {
            issues.nest_result(
                issue_path!["processors", name],
                validate_processor(fs, procconf),
            );
        }
    }
    issues.into_result(())
}

/// The largest number of channels anywhere in the pipeline: the devices and
/// both sides of every mixer that is not bypassed.
pub fn max_channels(conf: &Configuration) -> usize {
    let mut max_channels = conf
        .devices
        .capture
        .channels()
        .max(conf.devices.playback.channels());
    if let (Some(pipeline), Some(mixers)) = (&conf.pipeline, &conf.mixers) {
        for step in pipeline {
            if let PipelineStep::Mixer(step) = step
                && !step.is_bypassed()
                && let Some(mixer) = mixers.get(&step.name)
            {
                max_channels = max_channels
                    .max(mixer.channels.input())
                    .max(mixer.channels.output());
            }
        }
    }
    max_channels
}

/// Get a vector telling which channels are actually used in the pipeline
pub fn used_capture_channels(conf: &Configuration) -> Vec<bool> {
    if let Some(pipeline) = &conf.pipeline {
        for step in pipeline.iter() {
            if let PipelineStep::Mixer(mix) = step
                && !mix.is_bypassed()
            {
                // Safe to unwrap here since we have already verified that the mixer exists
                let mixerconf = conf.mixers.as_ref().unwrap().get(&mix.name).unwrap();
                return mixer::used_input_channels(mixerconf);
            }
        }
    }
    let capture_channels = conf.devices.capture.channels();
    vec![true; capture_channels]
}

/// Return the capture channel labels from `config`, or `None` if no config is active.
pub fn capture_channel_labels(config: &Option<Configuration>) -> Option<Vec<Option<String>>> {
    if let Some(conf) = config {
        conf.devices.capture.labels()
    } else {
        None
    }
}

/// Return the playback channel labels from `config`, or `None` if no config is active.
pub fn playback_channel_labels(config: &Option<Configuration>) -> Option<Vec<Option<String>>> {
    if let Some(conf) = config {
        if let Some(pipeline) = &conf.pipeline {
            for step in pipeline.iter().rev() {
                if let PipelineStep::Mixer(mixerstep) = step
                    && let Some(mixers) = &conf.mixers
                    && let Some(mixer) = mixers.get(&mixerstep.name)
                {
                    return mixer.labels.clone();
                }
            }
        }
        conf.devices.capture.labels()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_all_finite, deserialize_config, max_channels, parse_config, validate_config,
        validate_resampler, validate_unused,
    };
    use crate::config::{AsyncSincInterpolation, AsyncSincParameters, AsyncSincWindow, Resampler};
    use crate::config::{IssueKind, Issues, format_path};

    fn free_sinc(
        sinc_len: usize,
        interpolation: AsyncSincInterpolation,
        oversampling_factor: usize,
        f_cutoff: Option<f32>,
    ) -> Option<Resampler> {
        Some(Resampler::AsyncSinc(AsyncSincParameters::Free {
            sinc_len,
            interpolation,
            window: AsyncSincWindow::Blackman2,
            f_cutoff: f_cutoff.map(crate::config::FiniteF32::expect_finite),
            oversampling_factor,
        }))
    }

    fn parse(yaml: &str) -> Result<crate::config::Configuration, yaml_serde::Error> {
        yaml_serde::from_str(yaml)
    }

    const BASE: &str = r#"
devices:
  samplerate: 44100
  chunksize: 1024
  capture: {type: Stdin, channels: 2, format: S16_LE}
  playback: {type: Stdout, channels: 2, format: S16_LE}
"#;

    fn with_filter(params: &str) -> String {
        format!(
            "{BASE}filters:\n  f:\n    {params}\npipeline:\n  - type: Filter\n    channels: [0]\n    names: [f]\n"
        )
    }

    #[test]
    fn non_finite_rejected_while_parsing() {
        // A plain f64 field.
        assert!(parse(&with_filter("type: Gain\n    parameters: {gain: 3.0}")).is_ok());
        assert!(parse(&with_filter("type: Gain\n    parameters: {gain: .nan}")).is_err());
        assert!(parse(&with_filter("type: Gain\n    parameters: {gain: .inf}")).is_err());
        assert!(parse(&with_filter("type: Gain\n    parameters: {gain: -.inf}")).is_err());
        // An optional field, where an explicit null must still be accepted.
        assert!(
            parse(&with_filter(
                "type: Volume\n    parameters: {fader: Aux1, limit: null}"
            ))
            .is_ok()
        );
        assert!(
            parse(&with_filter(
                "type: Volume\n    parameters: {fader: Aux1, limit: .nan}"
            ))
            .is_err()
        );
        // A list field.
        assert!(
            parse(&with_filter(
                "type: Conv\n    parameters: {type: Values, values: [0.5, 0.5]}"
            ))
            .is_ok()
        );
        assert!(
            parse(&with_filter(
                "type: Conv\n    parameters: {type: Values, values: [0.5, .nan]}"
            ))
            .is_err()
        );
        // An optional list field.
        assert!(
            parse(&with_filter(
                "type: DiffEq\n    parameters: {a: [1.0], b: [1.0]}"
            ))
            .is_ok()
        );
        assert!(
            parse(&with_filter(
                "type: DiffEq\n    parameters: {a: [1.0], b: [1.0, .inf]}"
            ))
            .is_err()
        );
        // A devices field.
        assert!(
            parse(&BASE.replace("chunksize: 1024", "chunksize: 1024\n  volume_limit: .nan"))
                .is_err()
        );
    }

    /// The websocket `GetConfig` hands the config back as YAML, so the finite wrappers must
    /// serialize as plain numbers. A newtype that serialized as a map would change the wire
    /// format for every client.
    #[test]
    fn config_round_trips_as_plain_numbers() {
        let yaml = format!(
            "{BASE}filters:\n  g:\n    type: Gain\n    parameters: {{gain: -6.5}}\n\
             pipeline:\n  - type: Filter\n    channels: [0]\n    names: [g]\n"
        );
        let parsed = parse(&yaml).unwrap();
        let written = yaml_serde::to_string(&parsed).unwrap();
        assert!(written.contains("gain: -6.5"), "{written}");
        assert!(written.contains("samplerate: 44100"), "{written}");
        assert!(!written.contains("FiniteF"), "{written}");
        // And it parses back to the same thing.
        let reparsed: crate::config::Configuration = yaml_serde::from_str(&written).unwrap();
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn check_all_finite_covers_file_coefficients() {
        assert!(check_all_finite("x", &[1.0, 2.0, 3.0]).is_ok());
        assert!(check_all_finite::<f64>("x", &[]).is_ok());
        assert!(check_all_finite("x", &[1.0, f64::NAN]).is_err());
        assert!(check_all_finite("x", &[1.0f32, f32::INFINITY]).is_err());
        let err = check_all_finite("x", &[1.0, 2.0, f64::INFINITY])
            .unwrap_err()
            .to_string();
        assert!(err.contains("index 2"), "{err}");
    }

    #[test]
    fn resampler_profile_and_none_are_accepted() {
        assert!(validate_resampler(&None).is_ok());
        assert!(validate_resampler(&Some(Resampler::Synchronous)).is_ok());
        assert!(validate_resampler(&Some(Resampler::Slip)).is_ok());
        assert!(
            validate_resampler(&Some(Resampler::AsyncSinc(AsyncSincParameters::Profile {
                profile: crate::config::AsyncSincProfile::Balanced,
            })))
            .is_ok()
        );
    }

    #[test]
    fn resampler_sinc_len_must_be_nonzero() {
        assert!(
            validate_resampler(&free_sinc(0, AsyncSincInterpolation::Cubic, 256, None)).is_err()
        );
        assert!(
            validate_resampler(&free_sinc(1, AsyncSincInterpolation::Cubic, 256, None)).is_ok()
        );
    }

    /// Rubato fits the interpolation polynomial through neighbouring sincs and wraps a
    /// running index only once, so a table smaller than the number of fitted points panics.
    #[test]
    fn resampler_oversampling_minimum_follows_interpolation() {
        for (interpolation, minimum) in [
            (AsyncSincInterpolation::Nearest, 1),
            (AsyncSincInterpolation::Linear, 1),
            (AsyncSincInterpolation::Quadratic, 2),
            (AsyncSincInterpolation::Cubic, 3),
        ] {
            assert!(
                validate_resampler(&free_sinc(64, interpolation, minimum, None)).is_ok(),
                "{interpolation:?} should accept {minimum}"
            );
            assert!(
                validate_resampler(&free_sinc(64, interpolation, minimum - 1, None)).is_err(),
                "{interpolation:?} should reject {}",
                minimum - 1
            );
        }
    }

    #[test]
    fn resampler_cutoff_range() {
        let cubic = AsyncSincInterpolation::Cubic;
        assert!(validate_resampler(&free_sinc(64, cubic, 256, None)).is_ok());
        assert!(validate_resampler(&free_sinc(64, cubic, 256, Some(0.95))).is_ok());
        assert!(validate_resampler(&free_sinc(64, cubic, 256, Some(1.0))).is_ok());
        assert!(validate_resampler(&free_sinc(64, cubic, 256, Some(0.0))).is_err());
        assert!(validate_resampler(&free_sinc(64, cubic, 256, Some(-0.5))).is_err());
        assert!(validate_resampler(&free_sinc(64, cubic, 256, Some(1.5))).is_err());
        // A non-finite cutoff cannot reach here at all, `FiniteF32` cannot hold one. The
        // parsing side of that is covered by `non_finite_rejected_while_parsing`.
    }

    fn with_mixers(bypassed: bool) -> String {
        format!(
            "{BASE}mixers:
  wide:
    channels: {{in: 2, out: 6}}
    mapping:
      - dest: 0
        sources: [{{channel: 0}}]
  narrow:
    channels: {{in: 6, out: 2}}
    mapping:
      - dest: 0
        sources: [{{channel: 0}}]
pipeline:
  - type: Mixer
    name: wide
    bypassed: {bypassed}
  - type: Mixer
    name: narrow
    bypassed: {bypassed}
"
        )
    }

    #[test]
    fn max_channels_includes_mixers() {
        assert_eq!(max_channels(&parse(BASE).unwrap()), 2);
        assert_eq!(max_channels(&parse(&with_mixers(false)).unwrap()), 6);
        // Bypassed mixers do not widen anything.
        assert_eq!(max_channels(&parse(&with_mixers(true)).unwrap()), 2);
    }

    /// The path and kind of every issue, in order.
    fn located(issues: &Issues) -> Vec<(String, IssueKind)> {
        issues
            .iter()
            .map(|issue| (format_path(&issue.path), issue.kind))
            .collect()
    }

    /// The issues found in a config that is expected to be invalid.
    fn issues_in(yaml: &str) -> Issues {
        let mut conf = parse(yaml).unwrap();
        validate_config(&mut conf, None)
            .err()
            .expect("the config should be invalid")
    }

    #[test]
    fn all_issues_are_reported_with_their_paths() {
        let yaml = r#"
devices:
  samplerate: 44100
  chunksize: 1024
  volume_limit: 60
  capture: {type: Stdin, channels: 2, format: S16_LE}
  playback: {type: Stdout, channels: 2, format: S16_LE}
mixers:
  mono:
    channels: {in: 2, out: 1}
    mapping:
      - dest: 3
        sources: [{channel: 0}]
filters:
  lp:
    type: Biquad
    parameters: {type: Lowpass, freq: 30000, q: 0}
  fir:
    type: Conv
    parameters: {type: Raw, filename: /no/such/dir/fir.raw, format: F32_LE}
pipeline:
  - type: Filter
    channels: [0, 5]
    names: [lp, missing, fir]
  - type: Mixer
    name: mono
  - type: Filter
    names: [lp]
"#;
        let issues = issues_in(yaml);
        let invalid = IssueKind::Invalid;
        assert_eq!(
            located(&issues),
            vec![
                ("devices.volume_limit".to_string(), invalid),
                ("pipeline[0].channels".to_string(), invalid),
                ("filters.lp.parameters.freq".to_string(), invalid),
                ("filters.lp.parameters.q".to_string(), invalid),
                ("pipeline[0].names[1]".to_string(), invalid),
                (
                    "filters.fir.parameters.filename".to_string(),
                    IssueKind::MissingFile
                ),
                ("mixers.mono.mapping[0].dest".to_string(), invalid),
                ("pipeline".to_string(), invalid),
            ],
            "{issues}"
        );
        // The issues come out one per line, each with its path.
        let text = issues.to_string();
        assert_eq!(text.lines().count(), 8, "{text}");
        assert!(
            text.contains("filters.lp.parameters.freq: Frequency must be < samplerate/2"),
            "{text}"
        );
    }

    #[test]
    fn a_filter_used_twice_is_reported_once() {
        let yaml = format!(
            "{BASE}filters:\n  g:\n    type: Gain\n    parameters: {{gain: 200}}\n\
             pipeline:\n  - type: Filter\n    names: [g, g]\n  - type: Filter\n    names: [g]\n"
        );
        let issues = issues_in(&yaml);
        assert_eq!(
            located(&issues),
            vec![("filters.g.parameters.gain".to_string(), IssueKind::Invalid)]
        );
    }

    #[test]
    fn channel_counts_are_not_checked_after_a_missing_mixer() {
        let yaml = format!(
            "{BASE}processors:\n  race:\n    type: RACE\n    parameters:\n      \
             {{channels: 6, channel_a: 0, channel_b: 1, delay: 1, delay_unit: ms, attenuation: 3}}\n\
             pipeline:\n  - type: Mixer\n    name: missing\n  - type: Processor\n    name: race\n"
        );
        // Only the missing mixer, the count of 6 cannot be judged without it.
        let issues = issues_in(&yaml);
        assert_eq!(
            located(&issues),
            vec![("pipeline[0].name".to_string(), IssueKind::Invalid)]
        );
    }

    #[test]
    fn missing_capture_file_is_its_own_kind() {
        let yaml = r#"
devices:
  samplerate: 44100
  chunksize: 1024
  capture: {type: RawFile, filename: /no/such/dir/input.raw, channels: 2, format: S16_LE}
  playback: {type: Stdout, channels: 2, format: S16_LE}
"#;
        let issues = issues_in(yaml);
        assert_eq!(
            located(&issues),
            vec![(
                "devices.capture.filename".to_string(),
                IssueKind::MissingFile
            )]
        );
    }

    #[test]
    fn structural_errors_have_a_path() {
        let yaml = BASE.replace("type: Stdin", "type: NoSuchDevice");
        let issue = parse_config(&yaml).unwrap_err();
        assert_eq!(format_path(&issue.path), "devices.capture.type");
        assert_eq!(issue.kind, IssueKind::Invalid);
        // The parser's own copy of the path is not repeated in the message.
        assert!(
            issue.message.starts_with("unknown variant `NoSuchDevice`"),
            "{}",
            issue.message
        );
        assert!(
            issue
                .to_string()
                .starts_with("devices.capture.type: unknown variant")
        );

        // Inside a tagged enum the path reaches the item.
        let yaml = with_filter("type: Gain\n    parameters: {gain: loud}");
        let issue = parse_config(&yaml).unwrap_err();
        assert_eq!(format_path(&issue.path), "filters.f");
    }

    #[test]
    fn config_can_be_deserialized_from_json() {
        let json = r#"{"devices": {"samplerate": 44100, "chunksize": "big",
            "capture": {"type": "Stdin", "channels": 2, "format": "S16_LE"},
            "playback": {"type": "Stdout", "channels": 2, "format": "S16_LE"}}}"#;
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let issue = deserialize_config(&mut deserializer).unwrap_err();
        assert_eq!(format_path(&issue.path), "devices.chunksize");
    }

    /// A device type that this build has no backend for.
    #[cfg(not(target_os = "macos"))]
    const FOREIGN_CAPTURE: &str = "{type: CoreAudio, channels: 2}";
    #[cfg(target_os = "macos")]
    const FOREIGN_CAPTURE: &str = "{type: Wasapi, channels: 2}";

    #[test]
    fn unsupported_device_type_does_not_hide_other_issues() {
        let yaml = BASE
            .replace(
                "{type: Stdin, channels: 2, format: S16_LE}",
                FOREIGN_CAPTURE,
            )
            .replace("chunksize: 1024", "chunksize: 1024\n  volume_limit: 60");
        // Parses, rather than failing on an unknown variant.
        let issues = issues_in(&yaml);
        assert_eq!(
            located(&issues),
            vec![
                ("devices.capture.type".to_string(), IssueKind::Unsupported),
                ("devices.volume_limit".to_string(), IssueKind::Invalid),
            ],
            "{issues}"
        );
    }

    #[test]
    fn unused_definitions_are_checked_separately() {
        let yaml = format!(
            "{BASE}filters:
  used:
    type: Gain
    parameters: {{gain: 3}}
  loud:
    type: Gain
    parameters: {{gain: 200}}
  fir:
    type: Conv
    parameters: {{type: Raw, filename: /no/such/dir/fir.raw, format: F32_LE}}
  skipped:
    type: Delay
    parameters: {{delay: -1, delay_unit: ms}}
mixers:
  bad:
    channels: {{in: 2, out: 2}}
    mapping:
      - dest: 5
        sources: [{{channel: 0}}]
processors:
  gate:
    type: NoiseGate
    parameters:
      {{channels: 2, attack: 1, attack_unit: ms, release: 1, release_unit: ms,
       threshold: -50, attenuation: -20}}
pipeline:
  - type: Filter
    names: [used]
  - type: Filter
    bypassed: true
    names: [skipped]
"
        );
        // CamillaDSP itself only checks what the pipeline uses.
        let mut conf = parse(&yaml).unwrap();
        assert!(validate_config(&mut conf, None).is_ok());
        let issues = validate_unused(&conf).unwrap_err();
        assert_eq!(
            located(&issues),
            vec![
                (
                    "filters.fir.parameters.filename".to_string(),
                    IssueKind::MissingFile
                ),
                (
                    "filters.loud.parameters.gain".to_string(),
                    IssueKind::Invalid
                ),
                (
                    "filters.skipped.parameters.delay".to_string(),
                    IssueKind::Invalid
                ),
                ("mixers.bad.mapping[0].dest".to_string(), IssueKind::Invalid),
                (
                    "processors.gate.parameters.attenuation".to_string(),
                    IssueKind::Invalid
                ),
            ],
            "{issues}"
        );
    }

    #[test]
    fn rules_taken_over_from_the_gui_schemas() {
        let yaml = r#"
devices:
  samplerate: 44100
  chunksize: 1024
  capture: {type: RawFile, filename: "", channels: 2, format: S16_LE}
  playback: {type: File, filename: "", channels: 2, format: S16_LE}
mixers:
  mix:
    channels: {in: 2, out: 2}
    mapping:
      - dest: 0
        sources: [{channel: 0, gain: 200}]
      - dest: 1
        sources: [{channel: 1, gain: 20, scale: linear}]
filters:
  vol:
    type: Volume
    parameters: {fader: Aux1, limit: 60}
processors:
  gate:
    type: NoiseGate
    parameters:
      {channels: 2, attack: 1, attack_unit: ms, release: 1, release_unit: ms,
       threshold: -50, attenuation: -20}
pipeline:
  - type: Mixer
    name: mix
  - type: Filter
    names: [vol]
  - type: Processor
    name: gate
"#;
        let issues = issues_in(yaml);
        let paths: Vec<String> = located(&issues).into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            vec![
                "devices.capture.filename",
                "devices.playback.filename",
                "mixers.mix.mapping[0].sources[0].gain",
                "mixers.mix.mapping[1].sources[0].gain",
                "filters.vol.parameters.limit",
                "processors.gate.parameters.attenuation",
            ],
            "{issues}"
        );
        // An empty file name is invalid, not a missing file.
        assert!(issues.iter().all(|issue| issue.kind == IssueKind::Invalid));
    }
}
