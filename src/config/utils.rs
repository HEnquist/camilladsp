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
use crate::filters;
use crate::mixer;
use crate::processors::compressor;
use crate::processors::noisegate;
use crate::processors::race;
use crate::wavtools::find_data_in_wav_stream;
use parking_lot::RwLock;
use serde::{Deserialize, de};
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};

// Keep same result type used by config module utility functions.
type Res<T> = Result<T, Box<dyn error::Error>>;

#[derive(Clone)]
pub struct OverridesState {
    pub samplerate: Option<usize>,
    pub sample_format: Option<BinarySampleFormat>,
    pub extra_samples: Option<usize>,
    pub channels: Option<usize>,
}

lazy_static! {
    pub static ref OVERRIDES: RwLock<OverridesState> = RwLock::new(OverridesState {
        samplerate: None,
        sample_format: None,
        extra_samples: None,
        channels: None,
    });
}

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
    pub fn new(desc: &str) -> Self {
        ConfigErrorType {
            desc: desc.to_owned(),
        }
    }
}

pub(crate) fn validate_nonzero_usize<'de, D>(d: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = usize::deserialize(d)?;
    if value < 1 {
        return Err(de::Error::invalid_value(
            de::Unexpected::Unsigned(value as u64),
            &"a value > 0",
        ));
    }
    Ok(value)
}

pub fn load_config(filename: &str) -> Res<Configuration> {
    let file = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("Could not open config file '{filename}'. Reason: {err}");
            return Err(ConfigError::new(&msg).into());
        }
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(err) => {
            let msg = format!("Could not read config file '{filename}'. Reason: {err}");
            return Err(ConfigError::new(&msg).into());
        }
    };
    let configuration: Configuration = match yaml_serde::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            let msg = format!("Invalid config file!\n{err}");
            return Err(ConfigError::new(&msg).into());
        }
    };
    Ok(configuration)
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
        let cfg_rate = configuration.devices.samplerate;
        let cfg_chunksize = configuration.devices.chunksize;

        if configuration.devices.resampler.is_none() {
            debug!("Apply override for samplerate: {rate}");
            configuration.devices.samplerate = rate;
            let scaled_chunksize = if rate > cfg_rate {
                cfg_chunksize * (rate as f32 / cfg_rate as f32).round() as usize
            } else {
                cfg_chunksize / (cfg_rate as f32 / rate as f32).round() as usize
            };
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
            configuration.devices.capture_samplerate = Some(rate);
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
        match &mut configuration.devices.capture {
            CaptureDevice::RawFile(dev) => {
                dev.channels = chans;
            }
            CaptureDevice::WavFile(_dev) => {}
            CaptureDevice::Stdin(dev) => {
                dev.channels = chans;
            }
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez(dev) => {
                dev.channels = chans;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
            CaptureDevice::PipeWire { channels, .. } => {
                *channels = chans;
            }
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => {
                dev.channels = chans;
            }
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => {
                dev.channels = chans;
            }
            #[cfg(all(target_os = "windows", feature = "asio-backend"))]
            CaptureDevice::Asio(dev) => {
                dev.channels = chans;
            }
            #[cfg(all(
                feature = "cpal-backend",
                feature = "jack-backend",
                any(
                    target_os = "linux",
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "netbsd"
                )
            ))]
            CaptureDevice::Jack { channels, .. } => {
                *channels = chans;
            }
            CaptureDevice::SignalGenerator { channels, .. } => {
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
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { format, .. } => {
                let mapped_format = AlsaSampleFormat::from_binary_format(&fmt);
                *format = Some(mapped_format);
            }
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez(dev) => {
                dev.format = fmt;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { .. } => {
                error!("Not possible to override capture format for Pulse, ignoring");
            }
            #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
            CaptureDevice::PipeWire { .. } => {
                error!("Not possible to override capture format for PipeWire, ignoring");
            }
            #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "windows")]
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
            #[cfg(all(target_os = "windows", feature = "asio-backend"))]
            CaptureDevice::Asio(dev) => {
                let mapped_format = AsioSampleFormat::from_binary_format(&fmt);
                if let Some(mapped) = mapped_format {
                    dev.format = Some(mapped);
                } else {
                    let msg = format!("ASIO does not have a sample format corresponding to {fmt}");
                    return Err(ConfigError::new(&msg).into());
                }
            }
            #[cfg(all(
                feature = "cpal-backend",
                feature = "jack-backend",
                any(
                    target_os = "linux",
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "netbsd"
                )
            ))]
            CaptureDevice::Jack { .. } => {
                error!("Not possible to override capture format for Jack, ignoring");
            }
            CaptureDevice::SignalGenerator { .. } => {}
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
    let samplerate = config.devices.samplerate;
    let num_channels = config.devices.capture.channels();
    if let Some(filters) = &mut config.filters {
        for (_name, filter) in filters.iter_mut() {
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
                for (_name, filter) in filters.iter_mut() {
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

pub fn load_validate_config(configname: &str) -> Res<Configuration> {
    let mut configuration = load_config(configname)?;
    validate_config(&mut configuration, Some(configname))?;
    Ok(configuration)
}

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
    let mut mixers = Vec::<String>::new();
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
                    | (Filter::Limiter { .. }, Filter::Limiter { .. }) => {}
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
    if let (Some(newmixers), Some(oldmixers)) = (&newconf.mixers, &currentconf.mixers) {
        for (mixer, params) in newmixers {
            // The pipeline didn't change, any added mixer isn't included and can be skipped
            if let Some(current_mixer) = oldmixers.get(mixer) {
                if params != current_mixer {
                    mixers.push(mixer.to_string());
                }
            }
        }
    }
    if let (Some(newprocs), Some(oldprocs)) = (&newconf.processors, &currentconf.processors) {
        for (proc, params) in newprocs {
            // The pipeline didn't change, any added processor isn't included and can be skipped
            if let Some(current_proc) = oldprocs.get(proc) {
                if params != current_proc {
                    processors.push(proc.to_string());
                }
            }
        }
    }
    ConfigChange::FilterParameters {
        filters,
        mixers,
        processors,
    }
}

/// Validate the loaded configuration, stop on errors and print a helpful message.
pub fn validate_config(conf: &mut Configuration, filename: Option<&str>) -> Res<()> {
    // pre-process by applying overrides and replacing tokens
    apply_overrides(conf)?;
    replace_tokens_in_config(conf);
    if let Some(fname) = filename {
        replace_relative_paths_in_config(conf, fname);
    }
    #[cfg(target_os = "linux")]
    let target_level_limit = if matches!(conf.devices.playback, PlaybackDevice::Alsa { .. }) {
        (4 + conf.devices.queuelimit()) * conf.devices.chunksize
    } else {
        (2 + conf.devices.queuelimit()) * conf.devices.chunksize
    };
    #[cfg(not(target_os = "linux"))]
    let target_level_limit = (2 + conf.devices.queuelimit()) * conf.devices.chunksize;

    if conf.devices.target_level() > target_level_limit {
        let msg = format!("target_level cannot be larger than {target_level_limit}");
        return Err(ConfigError::new(&msg).into());
    }
    if let Some(period) = conf.devices.adjust_period {
        if period <= 0.0 {
            return Err(ConfigError::new("adjust_period must be positive and > 0").into());
        }
    }
    if let Some(threshold) = conf.devices.silence_threshold {
        if threshold > 0.0 {
            return Err(
                ConfigError::new("silence_threshold must be less than or equal to 0").into(),
            );
        }
    }
    if let Some(timeout) = conf.devices.silence_timeout {
        if timeout < 0.0 {
            return Err(ConfigError::new("silence_timeout cannot be negative").into());
        }
    }
    if conf.devices.ramp_time() < 0.0 {
        return Err(ConfigError::new("Volume ramp time cannot be negative").into());
    }
    if conf.devices.volume_limit() > 50.0 {
        return Err(ConfigError::new("Volume limit cannot be above +50 dB").into());
    }
    if conf.devices.volume_limit() < -150.0 {
        return Err(ConfigError::new("Volume limit cannot be less than -150 dB").into());
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi(dev) = &conf.devices.capture {
        if let Some(format) = dev.format {
            if format != WasapiSampleFormat::F32 && !dev.is_exclusive() {
                return Err(ConfigError::new(
                    "Wasapi shared mode capture must use F32 sample format",
                )
                .into());
            }
        }
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi(dev) = &conf.devices.capture {
        if dev.is_loopback() && dev.is_exclusive() {
            return Err(ConfigError::new(
                "Wasapi loopback capture is only supported in shared mode",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let PlaybackDevice::Wasapi(dev) = &conf.devices.playback {
        if let Some(format) = dev.format {
            if format != WasapiSampleFormat::F32 && !dev.is_exclusive() {
                return Err(ConfigError::new(
                    "Wasapi shared mode playback must use F32 sample format",
                )
                .into());
            }
        }
    }
    #[cfg(all(target_os = "windows", feature = "asio-backend"))]
    if let (CaptureDevice::Asio(cap_dev), PlaybackDevice::Asio(pb_dev)) =
        (&conf.devices.capture, &conf.devices.playback)
    {
        if cap_dev.device != pb_dev.device {
            return Err(ConfigError::new(
                "ASIO only supports one driver at a time. \
                 Capture and playback must use the same ASIO device",
            )
            .into());
        }
        if conf.devices.resampler.is_some() {
            return Err(ConfigError::new(
                "Resampling is not supported in full-duplex ASIO mode. \
                 Both capture and playback share the same driver and sample rate",
            )
            .into());
        }
    }
    if let PlaybackDevice::File {
        format, wav_header, ..
    } = &conf.devices.playback
    {
        if *format == BinarySampleFormat::S24_4_RJ_LE && *wav_header == Some(true) {
            return Err(
                ConfigError::new("Wav files do not support the S24_4_RJ_LE sample format").into(),
            );
        }
    }
    if let CaptureDevice::RawFile(dev) = &conf.devices.capture {
        let fname = &dev.filename;
        match File::open(fname) {
            Ok(f) => f,
            Err(err) => {
                let msg = format!("Could not open input file '{fname}'. Reason: {err}");
                return Err(ConfigError::new(&msg).into());
            }
        };
    }
    if let CaptureDevice::WavFile(dev) = &conf.devices.capture {
        let fname = &dev.filename;
        let f = match File::open(fname) {
            Ok(f) => f,
            Err(err) => {
                let msg = format!("Could not open input file '{fname}'. Reason: {err}");
                return Err(ConfigError::new(&msg).into());
            }
        };
        let file = BufReader::new(&f);
        let _wav_info = find_data_in_wav_stream(file).map_err(|err| {
            let msg = format!("Error reading wav file '{fname}'. Reason: {err}");
            ConfigError::new(&msg)
        })?;
    }
    let mut num_channels = conf.devices.capture.channels();
    let fs = conf.devices.samplerate;
    if let Some(pipeline) = &conf.pipeline {
        for step in pipeline {
            match step {
                PipelineStep::Mixer(step) => {
                    if !step.is_bypassed() {
                        if let Some(mixers) = &conf.mixers {
                            if !mixers.contains_key(&step.name) {
                                let msg = format!("Use of missing mixer '{}'", &step.name);
                                return Err(ConfigError::new(&msg).into());
                            } else {
                                let chan_in = mixers.get(&step.name).unwrap().channels.r#in;
                                if chan_in != num_channels {
                                    let msg = format!(
                                        "Mixer '{}' has wrong number of input channels. Expected {}, found {}.",
                                        &step.name, num_channels, chan_in
                                    );
                                    return Err(ConfigError::new(&msg).into());
                                }
                                num_channels = mixers.get(&step.name).unwrap().channels.out;
                                match mixer::validate_mixer(mixers.get(&step.name).unwrap()) {
                                    Ok(_) => {}
                                    Err(err) => {
                                        let msg = format!(
                                            "Invalid mixer '{}'. Reason: {}",
                                            &step.name, err
                                        );
                                        return Err(ConfigError::new(&msg).into());
                                    }
                                }
                            }
                        } else {
                            let msg = format!("Use of missing mixer '{}'", &step.name);
                            return Err(ConfigError::new(&msg).into());
                        }
                    }
                }
                PipelineStep::Filter(step) => {
                    if !step.is_bypassed() {
                        if let Some(channels) = &step.channels {
                            for channel in channels {
                                if *channel >= num_channels {
                                    let msg = format!("Use of non existing channel {channel}");
                                    return Err(ConfigError::new(&msg).into());
                                }
                            }
                            for idx in 1..channels.len() {
                                if channels[idx..].contains(&channels[idx - 1]) {
                                    let msg =
                                        format!("Use of duplicated channel {}", &channels[idx - 1]);
                                    return Err(ConfigError::new(&msg).into());
                                }
                            }
                        }
                        for name in &step.names {
                            if let Some(filters) = &conf.filters {
                                if !filters.contains_key(name) {
                                    let msg = format!("Use of missing filter '{name}'");
                                    return Err(ConfigError::new(&msg).into());
                                }
                                match filters::validate_filter(fs, filters.get(name).unwrap()) {
                                    Ok(_) => {}
                                    Err(err) => {
                                        let msg = format!("Invalid filter '{name}'. Reason: {err}");
                                        return Err(ConfigError::new(&msg).into());
                                    }
                                }
                            } else {
                                let msg = format!("Use of missing filter '{name}'");
                                return Err(ConfigError::new(&msg).into());
                            }
                        }
                    }
                }
                PipelineStep::Processor(step) => {
                    if !step.is_bypassed() {
                        if let Some(processors) = &conf.processors {
                            if !processors.contains_key(&step.name) {
                                let msg = format!("Use of missing processor '{}'", step.name);
                                return Err(ConfigError::new(&msg).into());
                            } else {
                                let procconf = processors.get(&step.name).unwrap();
                                match procconf {
                                    Processor::Compressor { parameters, .. } => {
                                        let channels = parameters.channels;
                                        if channels != num_channels {
                                            let msg = format!(
                                                "Compressor '{}' has wrong number of channels. Expected {}, found {}.",
                                                step.name, num_channels, channels
                                            );
                                            return Err(ConfigError::new(&msg).into());
                                        }
                                        match compressor::validate_compressor(parameters) {
                                            Ok(_) => {}
                                            Err(err) => {
                                                let msg = format!(
                                                    "Invalid processor '{}'. Reason: {}",
                                                    step.name, err
                                                );
                                                return Err(ConfigError::new(&msg).into());
                                            }
                                        }
                                    }
                                    Processor::NoiseGate { parameters, .. } => {
                                        let channels = parameters.channels;
                                        if channels != num_channels {
                                            let msg = format!(
                                                "NoiseGate '{}' has wrong number of channels. Expected {}, found {}.",
                                                step.name, num_channels, channels
                                            );
                                            return Err(ConfigError::new(&msg).into());
                                        }
                                        match noisegate::validate_noise_gate(parameters) {
                                            Ok(_) => {}
                                            Err(err) => {
                                                let msg = format!(
                                                    "Invalid noise gate '{}'. Reason: {}",
                                                    step.name, err
                                                );
                                                return Err(ConfigError::new(&msg).into());
                                            }
                                        }
                                    }
                                    Processor::RACE { parameters, .. } => {
                                        let channels = parameters.channels;
                                        if channels != num_channels {
                                            let msg = format!(
                                                "RACE processor '{}' has wrong number of channels. Expected {}, found {}.",
                                                step.name, num_channels, channels
                                            );
                                            return Err(ConfigError::new(&msg).into());
                                        }
                                        match race::validate_race(parameters) {
                                            Ok(_) => {}
                                            Err(err) => {
                                                let msg = format!(
                                                    "Invalid RACE processor '{}'. Reason: {}",
                                                    step.name, err
                                                );
                                                return Err(ConfigError::new(&msg).into());
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            let msg = format!("Use of missing processor '{}'", step.name);
                            return Err(ConfigError::new(&msg).into());
                        }
                    }
                }
            }
        }
    }
    let num_channels_out = conf.devices.playback.channels();
    if num_channels != num_channels_out {
        let msg = format!(
            "Pipeline outputs {num_channels} channels, playback device has {num_channels_out}."
        );
        return Err(ConfigError::new(&msg).into());
    }
    Ok(())
}

/// Get a vector telling which channels are actually used in the pipeline
pub fn used_capture_channels(conf: &Configuration) -> Vec<bool> {
    if let Some(pipeline) = &conf.pipeline {
        for step in pipeline.iter() {
            if let PipelineStep::Mixer(mix) = step {
                if !mix.is_bypassed() {
                    // Safe to unwrap here since we have already verified that the mixer exists
                    let mixerconf = conf.mixers.as_ref().unwrap().get(&mix.name).unwrap();
                    return mixer::used_input_channels(mixerconf);
                }
            }
        }
    }
    let capture_channels = conf.devices.capture.channels();
    vec![true; capture_channels]
}

pub fn capture_channel_labels(config: &Option<Configuration>) -> Option<Vec<Option<String>>> {
    if let Some(conf) = config {
        conf.devices.capture.labels()
    } else {
        None
    }
}

pub fn playback_channel_labels(config: &Option<Configuration>) -> Option<Vec<Option<String>>> {
    if let Some(conf) = config {
        if let Some(pipeline) = &conf.pipeline {
            for step in pipeline.iter().rev() {
                if let PipelineStep::Mixer(mixerstep) = step {
                    if let Some(mixers) = &conf.mixers {
                        if let Some(mixer) = mixers.get(&mixerstep.name) {
                            return mixer.labels.clone();
                        }
                    }
                }
            }
        }
        conf.devices.capture.labels()
    } else {
        None
    }
}
