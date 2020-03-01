use filters;
use serde::Deserialize;
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S32LE,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Devices {
    pub samplerate: usize,
    pub buffersize: usize,
    #[serde(default)]
    pub silence_threshold: PrcFmt,
    #[serde(default)]
    pub silence_timeout: PrcFmt,
    pub capture: Device,
    pub playback: Device,
}

//#[derive(Clone, Debug, Deserialize, PartialEq)]
//pub enum FilterType {
//    Biquad,
//    Conv,
//    Gain,
//    Delay,
//}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Filter {
    Conv { parameters: ConvParameters },
    Biquad { parameters: BiquadParameters },
    Delay { parameters: DelayParameters },
    Gain { parameters: GainParameters },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ConvParameters {
    File { filename: String },
    Values { values: Vec<PrcFmt> },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
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
    Lowshelf {
        freq: PrcFmt,
        slope: PrcFmt,
        gain: PrcFmt,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct GainParameters {
    pub gain: PrcFmt,
    pub inverted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DelayParameters {
    pub delay: PrcFmt,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MixerChannels {
    pub r#in: usize,
    pub out: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
    pub inverted: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Mixer {
    pub channels: MixerChannels,
    pub mapping: Vec<MixerMapping>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String> },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
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
            | (Filter::Gain { .. }, Filter::Gain { .. }) => {}
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
    let mut num_channels = match conf.devices.capture {
        #[cfg(feature = "alsa-backend")]
        Device::Alsa { channels, .. } => channels,
        #[cfg(feature = "pulse-backend")]
        Device::Pulse { channels, .. } => channels,
        Device::File { channels, .. } => channels,
    };
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
                    filters::validate_filter(&conf.filters.get(&name).unwrap())?;
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
