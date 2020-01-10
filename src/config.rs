use serde::Deserialize;
use std::collections::HashMap;

//type SmpFmt = i16;
type PrcFmt = f64;


#[derive(Clone, Debug, Deserialize)]
pub enum DeviceType {
    Alsa,
}

#[derive(Clone, Debug, Deserialize)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S32LE,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Device {
    pub r#type: DeviceType,
    pub channels: usize,
    pub device: String,
    pub format: SampleFormat,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Devices {
    pub samplerate: usize,
    pub buffersize: usize,
    pub capture: Device,
    pub playback: Device,
}

#[derive(Clone, Debug, Deserialize)]
pub enum FilterType {
    Biquad,
    Conv,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Filter {
    pub r#type: FilterType,
    pub coefficients: FilterCoefficients,
}


#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
pub enum FilterCoefficients {
    File { values: String },
    Values { values: Vec<PrcFmt>},
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
    pub mixers: HashMap<String, Mixer>,
    pub filters: HashMap<String, Filter>,
    pub pipeline: Vec<PipelineStep>,
}