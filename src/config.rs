use serde::Deserialize;
use std::collections::HashMap;
use std::error;
use std::fmt;
use filters;

//type SmpFmt = i16;
type PrcFmt = f64;
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

#[derive(Clone, Debug, Deserialize)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S32LE,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Device {
    Alsa { channels: usize, device: String, format: SampleFormat },
    Pulse { channels: usize, device: String, format: SampleFormat },
    File { channels: usize, filename: String, format: SampleFormat },
}

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
pub enum FilterType {
    Biquad,
    Conv,
    Gain,
    Delay
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Filter {
    Conv { parameters: ConvParameters },
    Biquad { parameters: BiquadParameters },
    Delay { parameters: DelayParameters },
    Gain { parameters: GainParameters },
}


#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ConvParameters {
    File { filename: String },
    Values { values: Vec<PrcFmt>},
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum BiquadParameters {
    Free {a1: PrcFmt, a2: PrcFmt, b0: PrcFmt, b1: PrcFmt, b2: PrcFmt},
    Highpass { freq: PrcFmt, q: PrcFmt},
    Lowpass { freq: PrcFmt, q: PrcFmt},
    Peaking { freq: PrcFmt, gain: PrcFmt, q: PrcFmt},
    Highshelf { freq: PrcFmt, slope: PrcFmt, gain: PrcFmt},
    Lowshelf  { freq: PrcFmt, slope: PrcFmt, gain: PrcFmt},
}

#[derive(Clone, Debug, Deserialize)]
pub struct GainParameters {
    pub gain: PrcFmt, 
    pub inverted: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DelayParameters {
    pub delay: PrcFmt,
}




#[derive(Clone, Debug, Deserialize)]
pub struct MixerChannels {
    pub r#in: usize,
    pub out: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
    pub inverted: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Mixer {
    pub channels: MixerChannels,
    pub mapping: Vec<MixerMapping>,
}


#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String>}
}

#[derive(Clone, Debug, Deserialize)]
pub struct Configuration {
    pub devices: Devices,
    #[serde(default)]
    pub mixers: HashMap<String, Mixer>,
    #[serde(default)]
    pub filters: HashMap<String, Filter>,
    #[serde(default)]
    pub pipeline: Vec<PipelineStep>,
}


/// Validate the loaded configuration, stop on errors and print a helpful message.
pub fn validate_config(conf: Configuration) -> Res<()> {
    let mut num_channels = match conf.devices.capture {
        Device::Alsa {channels, device: _, format: _} => channels,
        Device::Pulse {channels, device: _, format: _} => channels,
        Device::File {channels, filename: _, format: _} => channels,
    };
    for step in conf.pipeline {
        match step {
            PipelineStep::Mixer { name } => {
                if !conf.mixers.contains_key(&name) {
                    return Err(Box::new(ConfigError::new(&format!("Use of missing mixer '{}'", name))));
                }
                else {
                    let chan_in = conf.mixers.get(&name).unwrap().channels.r#in;
                    if chan_in != num_channels {
                        return Err(Box::new(ConfigError::new(&format!("Mixer '{}' has wrong number of input channels. Expected {}, found {}.", name, num_channels, chan_in))));
                    }
                    num_channels = conf.mixers.get(&name).unwrap().channels.out;
                }
            },
            PipelineStep::Filter { channel, names } => {
                if channel > num_channels {
                    return Err(Box::new(ConfigError::new(&format!("Use of non existing channel {}", channel))));
                }
                for name in names {
                    if !conf.filters.contains_key(&name) {
                        return Err(Box::new(ConfigError::new(&format!("Use of missing filter '{}'", name))));
                    }
                    let _ = filters::validate_filter(&conf.filters.get(&name).unwrap())?;
                }
            },
        }
    }
    let num_channels_out = match conf.devices.playback {
        Device::Alsa {channels, device: _, format: _} => channels,
        Device::Pulse {channels, device: _, format: _} => channels,
        Device::File {channels, filename: _, format: _} => channels,
    };
    if num_channels != num_channels_out {
        return Err(Box::new(ConfigError::new(&format!("Pipeline outputs {} channels, playback device has {}.", num_channels, num_channels_out))));
    }
    Ok(())
}

