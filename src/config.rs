use filters;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;

//type SmpFmt = i16;
use PrcFmt;
type Res<T> = Result<T, Box<dyn error::Error>>;

#[derive(Debug)]
pub struct ConfigError {
    desc: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl error::Error for ConfigError {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl ConfigError {
    pub fn new(desc: &str) -> Self {
        ConfigError {
            desc: desc.to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S32LE,
    FLOAT32LE,
    FLOAT64LE,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Device {
    #[cfg(feature = "alsa-backend")]
    Alsa {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[cfg(feature = "pulse-backend")]
    Pulse {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    File {
        channels: usize,
        filename: String,
        format: SampleFormat,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Devices {
    pub samplerate: usize,
    // alias to allow old name buffersize
    #[serde(alias = "buffersize")]
    pub chunksize: usize,
    #[serde(default = "default_queuelimit")]
    pub queuelimit: usize,
    #[serde(default)]
    pub silence_threshold: PrcFmt,
    #[serde(default)]
    pub silence_timeout: PrcFmt,
    pub capture: Device,
    pub playback: Device,
    #[serde(default)]
    pub target_level: usize,
    #[serde(default = "default_period")]
    pub adjust_period: f32,
}

fn default_period() -> f32 {
    10.0
}

fn default_queuelimit() -> usize {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Filter {
    Conv {
        #[serde(default)]
        parameters: ConvParameters,
    },
    Biquad {
        parameters: BiquadParameters,
    },
    Delay {
        parameters: DelayParameters,
    },
    Gain {
        parameters: GainParameters,
    },
    Dither {
        parameters: DitherParameters,
    },
    DiffEq {
        parameters: DiffEqParameters,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum FileFormat {
    TEXT,
    S16LE,
    S24LE,
    S32LE,
    FLOAT32LE,
    FLOAT64LE,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ConvParameters {
    File {
        filename: String,
        #[serde(default)]
        format: FileFormat,
    },
    Values {
        values: Vec<PrcFmt>,
    },
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat::TEXT
    }
}

impl Default for ConvParameters {
    fn default() -> Self {
        ConvParameters::Values { values: vec![1.0] }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum BiquadParameters {
    Free {
        a1: PrcFmt,
        a2: PrcFmt,
        b0: PrcFmt,
        b1: PrcFmt,
        b2: PrcFmt,
    },
    Highpass {
        freq: PrcFmt,
        q: PrcFmt,
    },
    Lowpass {
        freq: PrcFmt,
        q: PrcFmt,
    },
    Peaking {
        freq: PrcFmt,
        gain: PrcFmt,
        q: PrcFmt,
    },
    Highshelf {
        freq: PrcFmt,
        slope: PrcFmt,
        gain: PrcFmt,
    },
    HighshelfFO {
        freq: PrcFmt,
        gain: PrcFmt,
    },
    Lowshelf {
        freq: PrcFmt,
        slope: PrcFmt,
        gain: PrcFmt,
    },
    LowshelfFO {
        freq: PrcFmt,
        gain: PrcFmt,
    },
    HighpassFO {
        freq: PrcFmt,
    },
    LowpassFO {
        freq: PrcFmt,
    },
    Allpass {
        freq: PrcFmt,
        q: PrcFmt,
    },
    AllpassFO { 
        freq: PrcFmt, 
    },
    Bandpass {
        freq: PrcFmt,
        q: PrcFmt,
    },
    Notch {
        freq: PrcFmt,
        q: PrcFmt,
    },
    LinkwitzTransform {
        freq_act: PrcFmt,
        q_act: PrcFmt,
        freq_target: PrcFmt,
        q_target: PrcFmt,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GainParameters {
    pub gain: PrcFmt,
    #[serde(default)]
    pub inverted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DelayParameters {
    pub delay: PrcFmt,
    #[serde(default)]
    pub unit: TimeUnit,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TimeUnit {
    #[serde(rename = "ms")]
    Milliseconds,
    #[serde(rename = "samples")]
    Samples,
}
impl Default for TimeUnit {
    fn default() -> Self {
        TimeUnit::Milliseconds
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum DitherParameters {
    Simple { bits: usize },
    Lipshitz { bits: usize },
    Uniform { bits: usize, amplitude: PrcFmt },
    None { bits: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DiffEqParameters {
    #[serde(default)]
    pub a: Vec<PrcFmt>,
    #[serde(default)]
    pub b: Vec<PrcFmt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MixerChannels {
    pub r#in: usize,
    pub out: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
    pub inverted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mixer {
    pub channels: MixerChannels,
    pub mapping: Vec<MixerMapping>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Configuration {
    pub devices: Devices,
    #[serde(default)]
    pub mixers: HashMap<String, Mixer>,
    #[serde(default)]
    pub filters: HashMap<String, Filter>,
    #[serde(default)]
    pub pipeline: Vec<PipelineStep>,
}

pub fn load_config(filename: &str) -> Res<Configuration> {
    let file = match File::open(filename) {
        Ok(f) => f,
        Err(_) => {
            return Err(Box::new(ConfigError::new("Could not open config file!")));
        }
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(_err) => {
            return Err(Box::new(ConfigError::new("Could not read config file!")));
        }
    };
    let configuration: Configuration = match serde_yaml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            return Err(Box::new(ConfigError::new(&format!(
                "Invalid config file!\n{}",
                err
            ))));
        }
    };
    Ok(configuration)
}

#[derive(Debug)]
pub enum ConfigChange {
    FilterParameters {
        filters: Vec<String>,
        mixers: Vec<String>,
    },
    Pipeline,
    Devices,
    None,
}

pub fn load_validate_config(configname: &str) -> Res<Configuration> {
    let configuration = load_config(configname)?;
    validate_config(configuration.clone())?;
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
    let mut filters = Vec::<String>::new();
    let mut mixers = Vec::<String>::new();
    for (filter, params) in &newconf.filters {
        match (params, currentconf.filters.get(filter).unwrap()) {
            (Filter::Biquad { .. }, Filter::Biquad { .. })
            | (Filter::Conv { .. }, Filter::Conv { .. })
            | (Filter::Delay { .. }, Filter::Delay { .. })
            | (Filter::Gain { .. }, Filter::Gain { .. })
            | (Filter::Dither { .. }, Filter::Dither { .. })
            | (Filter::DiffEq { .. }, Filter::DiffEq { .. }) => {}
            _ => {
                return ConfigChange::Pipeline;
            }
        };
        if params != currentconf.filters.get(filter).unwrap() {
            filters.push(filter.to_string());
        }
    }
    for (mixer, params) in &newconf.mixers {
        if params != currentconf.mixers.get(mixer).unwrap() {
            mixers.push(mixer.to_string());
        }
    }
    ConfigChange::FilterParameters { filters, mixers }
}

/// Validate the loaded configuration, stop on errors and print a helpful message.
pub fn validate_config(conf: Configuration) -> Res<()> {
    if conf.devices.target_level >= 2 * conf.devices.chunksize {
        return Err(Box::new(ConfigError::new("target_level is too large.")));
    }
    if conf.devices.adjust_period <= 0.0 {
        return Err(Box::new(ConfigError::new(
            "adjust_period must be positive and > 0",
        )));
    }
    let mut num_channels = match conf.devices.capture {
        #[cfg(feature = "alsa-backend")]
        Device::Alsa { channels, .. } => channels,
        #[cfg(feature = "pulse-backend")]
        Device::Pulse { channels, .. } => channels,
        Device::File { channels, .. } => channels,
    };
    let fs = conf.devices.samplerate;
    for step in conf.pipeline {
        match step {
            PipelineStep::Mixer { name } => {
                if !conf.mixers.contains_key(&name) {
                    return Err(Box::new(ConfigError::new(&format!(
                        "Use of missing mixer '{}'",
                        name
                    ))));
                } else {
                    let chan_in = conf.mixers.get(&name).unwrap().channels.r#in;
                    if chan_in != num_channels {
                        return Err(Box::new(ConfigError::new(&format!(
                            "Mixer '{}' has wrong number of input channels. Expected {}, found {}.",
                            name, num_channels, chan_in
                        ))));
                    }
                    num_channels = conf.mixers.get(&name).unwrap().channels.out;
                }
            }
            PipelineStep::Filter { channel, names } => {
                if channel > num_channels {
                    return Err(Box::new(ConfigError::new(&format!(
                        "Use of non existing channel {}",
                        channel
                    ))));
                }
                for name in names {
                    if !conf.filters.contains_key(&name) {
                        return Err(Box::new(ConfigError::new(&format!(
                            "Use of missing filter '{}'",
                            name
                        ))));
                    }
                    filters::validate_filter(fs, &conf.filters.get(&name).unwrap())?;
                }
            }
        }
    }
    let num_channels_out = match conf.devices.playback {
        #[cfg(feature = "alsa-backend")]
        Device::Alsa { channels, .. } => channels,
        #[cfg(feature = "pulse-backend")]
        Device::Pulse { channels, .. } => channels,
        Device::File { channels, .. } => channels,
    };
    if num_channels != num_channels_out {
        return Err(Box::new(ConfigError::new(&format!(
            "Pipeline outputs {} channels, playback device has {}.",
            num_channels, num_channels_out
        ))));
    }
    Ok(())
}
