use serde::{Serialize, Deserialize};
use std::collections::HashMap;

type SmpFmt = i16;
type PrcFmt = f64;


#[derive(Debug, Deserialize)]
pub enum DeviceType {
    Alsa,
}


#[derive(Debug, Deserialize)]
pub struct Device {
    r#type: DeviceType,
    channels: usize,
    device: String,
}

#[derive(Debug, Deserialize)]
pub struct Devices {
    samplerate: usize,
    capture: Device,
    playback: Device,
}

#[derive(Debug, Deserialize)]
pub enum FilterType {
    Biquad,
    Conv,
}

#[derive(Debug, Deserialize)]
pub struct Filter {
    r#type: FilterType,
    coefficients: FilterCoefficients,
}


#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum FilterCoefficients {
    File { values: String },
    Values { values: Vec<PrcFmt>},
}


#[derive(Debug, Deserialize)]
pub struct MixerChannels {
    r#in: usize,
    out: usize,
}

#[derive(Debug, Deserialize)]
pub struct MixerSource {
    channel: usize,
    gain: PrcFmt,
    inverted: bool,
}

#[derive(Debug, Deserialize)]
pub struct MixerMapping {
    dest: usize,
    sources: Vec<MixerSource>,
}

#[derive(Debug, Deserialize)]
pub struct Mixer {
    channels: MixerChannels,
    mapping: Vec<MixerMapping>,
}


#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String>}
}

#[derive(Debug, Deserialize)]
pub struct Configuration {
    devices: Devices,
    mixers: HashMap<String, Mixer>,
    filters: HashMap<String, Filter>,
    pipeline: Vec<PipelineStep>,
}