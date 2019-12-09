#[macro_use]
extern crate serde_derive;

type SampleFormat = i16;
type ProcessingFormat = f64;


#[derive(Debug, Deserialize)]
enum DeviceType {
    alsa,
}


#[derive(Debug, Deserialize)]
struct Device {
    type: DeviceType
    channels: usize,
    device: String,
}

#[derive(Debug, Deserialize)]
struct Devices {
    samplerate: usize,
    capture: Device,
    playback: Device,
}

#[derive(Debug, Deserialize)]
struct Filter {
    name: String,
    type: FilterType,
    coefficients: FilterCoefficients,
}


#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum FilterCoefficients {
    File { values: String },
    Values { values: Vec<ProcessingFormat>},
}


#[derive(Debug, Deserialize)]
struct MixerChannels {
    in: usize,
    out: usize,
}

#[derive(Debug, Deserialize)]
struct MixerSource {
    channel: usize,
    gain: ProcessingFormat,
    inverted: bool,
}

#[derive(Debug, Deserialize)]
struct MixerMapping {
    dest: usize,
    sources: Vec<MixerSource>,
}

#[derive(Debug, Deserialize)]
struct Mixer {
    name: String,
    channels: MixerChannels,
    mapping: Vec<MixerMapping>,
}


#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String>}
}

struct Configuration {
    devices: Devices,
    mixers: Vec<Mixer>,
    filters: Vec<Filter>,
    pipeline: Vec<PipelineStep>,
}