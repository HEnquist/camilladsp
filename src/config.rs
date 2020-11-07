use filters;
use mixer;
use serde::{Deserialize, Serialize};
use serde_with;
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::sync::RwLock;

//type SmpFmt = i16;
use PrcFmt;
type Res<T> = Result<T, Box<dyn error::Error>>;

pub struct Overrides {
    pub samplerate: Option<usize>,
    pub capture_samplerate: Option<usize>,
    pub sample_format: Option<SampleFormat>,
    pub extra_samples: Option<usize>,
    pub channels: Option<usize>,
}

lazy_static! {
    pub static ref OVERRIDES: RwLock<Overrides> = RwLock::new(Overrides {
        samplerate: None,
        capture_samplerate: None,
        sample_format: None,
        extra_samples: None,
        channels: None,
    });
}

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
#[serde(deny_unknown_fields)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S24LE3,
    S32LE,
    FLOAT32LE,
    FLOAT64LE,
}

#[derive(Clone, Debug)]
pub enum NumberFamily {
    Integer,
    Float,
}

impl SampleFormat {
    pub fn bits_per_sample(&self) -> usize {
        match self {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S24LE3 => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        }
    }

    pub fn bytes_per_sample(&self) -> usize {
        match self {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE => 4,
            SampleFormat::S24LE3 => 3,
            SampleFormat::S32LE => 4,
            SampleFormat::FLOAT32LE => 4,
            SampleFormat::FLOAT64LE => 8,
        }
    }

    pub fn number_family(&self) -> NumberFamily {
        match self {
            SampleFormat::S16LE
            | SampleFormat::S24LE
            | SampleFormat::S24LE3
            | SampleFormat::S32LE => NumberFamily::Integer,
            SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE => NumberFamily::Float,
        }
    }

    pub fn is_float(&self) -> bool {
        matches!(self, SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE)
    }

    pub fn from_name(label: &str) -> Option<SampleFormat> {
        match label {
            "FLOAT32LE" => Some(SampleFormat::FLOAT32LE),
            "FLOAT64LE" => Some(SampleFormat::FLOAT64LE),
            "S16LE" => Some(SampleFormat::S16LE),
            "S24LE" => Some(SampleFormat::S24LE),
            "S24LE3" => Some(SampleFormat::S24LE3),
            "S32LE" => Some(SampleFormat::S32LE),
            _ => None,
        }
    }
}

impl fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let formatstr = match self {
            SampleFormat::FLOAT32LE => "FLOAT32LE",
            SampleFormat::FLOAT64LE => "FLOAT64LE",
            SampleFormat::S16LE => "S16LE",
            SampleFormat::S24LE => "S24LE",
            SampleFormat::S24LE3 => "S24LE3",
            SampleFormat::S32LE => "S32LE",
        };
        write!(f, "{}", formatstr)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum CaptureDevice {
    #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
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
        #[serde(default)]
        extra_samples: usize,
        #[serde(default)]
        skip_bytes: usize,
        #[serde(default)]
        read_bytes: usize,
    },
    Stdin {
        channels: usize,
        format: SampleFormat,
        #[serde(default)]
        extra_samples: usize,
        #[serde(default)]
        skip_bytes: usize,
        #[serde(default)]
        read_bytes: usize,
    },
    #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
    CoreAudio {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
    Wasapi {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
}

impl CaptureDevice {
    pub fn channels(&self) -> usize {
        match self {
            #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
            CaptureDevice::Alsa { channels, .. } => *channels,
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => *channels,
            CaptureDevice::File { channels, .. } => *channels,
            CaptureDevice::Stdin { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
            CaptureDevice::CoreAudio { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
            CaptureDevice::Wasapi { channels, .. } => *channels,
        }
    }

    pub fn sampleformat(&self) -> SampleFormat {
        match self {
            #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
            CaptureDevice::Alsa { format, .. } => format.clone(),
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { format, .. } => format.clone(),
            CaptureDevice::File { format, .. } => format.clone(),
            CaptureDevice::Stdin { format, .. } => format.clone(),
            #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
            CaptureDevice::CoreAudio { format, .. } => format.clone(),
            #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
            CaptureDevice::Wasapi { format, .. } => format.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum PlaybackDevice {
    #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
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
    Stdout {
        channels: usize,
        format: SampleFormat,
    },
    #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
    CoreAudio {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
    Wasapi {
        channels: usize,
        device: String,
        format: SampleFormat,
    },
}

impl PlaybackDevice {
    pub fn channels(&self) -> usize {
        match self {
            #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
            PlaybackDevice::Alsa { channels, .. } => *channels,
            #[cfg(feature = "pulse-backend")]
            PlaybackDevice::Pulse { channels, .. } => *channels,
            PlaybackDevice::File { channels, .. } => *channels,
            PlaybackDevice::Stdout { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
            PlaybackDevice::CoreAudio { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
            PlaybackDevice::Wasapi { channels, .. } => *channels,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
    pub capture: CaptureDevice,
    pub playback: PlaybackDevice,
    #[serde(default)]
    pub enable_rate_adjust: bool,
    #[serde(default)]
    pub target_level: usize,
    #[serde(default = "default_period")]
    pub adjust_period: f32,
    #[serde(default)]
    pub enable_resampling: bool,
    #[serde(default)]
    pub resampler_type: Resampler,
    #[serde(default)]
    pub capture_samplerate: usize,
}

fn default_period() -> f32 {
    10.0
}

fn default_queuelimit() -> usize {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum Resampler {
    FastAsync,
    BalancedAsync,
    AccurateAsync,
    Synchronous,
    FreeAsync {
        sinc_len: usize,
        oversampling_ratio: usize,
        interpolation: InterpolationType,
        window: WindowFunction,
        f_cutoff: f32,
    },
}

impl Default for Resampler {
    fn default() -> Self {
        Resampler::BalancedAsync
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum WindowFunction {
    Hann,
    Hann2,
    Blackman,
    Blackman2,
    BlackmanHarris,
    BlackmanHarris2,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum InterpolationType {
    Cubic,
    Linear,
    Nearest,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum Filter {
    Conv {
        #[serde(default)]
        parameters: ConvParameters,
    },
    Biquad {
        parameters: BiquadParameters,
    },
    BiquadCombo {
        parameters: BiquadComboParameters,
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
#[serde(deny_unknown_fields)]
pub enum FileFormat {
    TEXT,
    S16LE,
    S24LE,
    S24LE3,
    S32LE,
    FLOAT32LE,
    FLOAT64LE,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum ConvParameters {
    File {
        filename: String,
        #[serde(default)]
        format: FileFormat,
        #[serde(default)]
        skip_bytes_lines: usize,
        #[serde(default)]
        read_bytes_lines: usize,
    },
    Values {
        values: Vec<PrcFmt>,
        #[serde(default)]
        length: usize,
    },
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat::TEXT
    }
}

impl Default for ConvParameters {
    fn default() -> Self {
        ConvParameters::Values {
            values: vec![1.0],
            length: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
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
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum BiquadComboParameters {
    LinkwitzRileyHighpass { freq: PrcFmt, order: usize },
    LinkwitzRileyLowpass { freq: PrcFmt, order: usize },
    ButterworthHighpass { freq: PrcFmt, order: usize },
    ButterworthLowpass { freq: PrcFmt, order: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GainParameters {
    pub gain: PrcFmt,
    #[serde(default)]
    pub inverted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DelayParameters {
    pub delay: PrcFmt,
    #[serde(default)]
    pub unit: TimeUnit,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub enum DitherParameters {
    Simple { bits: usize },
    Lipshitz441 { bits: usize },
    Fweighted441 { bits: usize },
    Shibata441 { bits: usize },
    Shibata48 { bits: usize },
    Uniform { bits: usize, amplitude: PrcFmt },
    None { bits: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiffEqParameters {
    #[serde(default)]
    pub a: Vec<PrcFmt>,
    #[serde(default)]
    pub b: Vec<PrcFmt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerChannels {
    pub r#in: usize,
    pub out: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
    pub inverted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mixer {
    pub channels: MixerChannels,
    pub mapping: Vec<MixerMapping>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum PipelineStep {
    Mixer { name: String },
    Filter { channel: usize, names: Vec<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub devices: Devices,
    #[serde(default)]
    pub mixers: HashMap<String, Mixer>,
    #[serde(default)]
    #[serde(deserialize_with = "serde_with::rust::maps_duplicate_key_is_error::deserialize")]
    pub filters: HashMap<String, Filter>,
    #[serde(default)]
    pub pipeline: Vec<PipelineStep>,
}

pub fn load_config(filename: &str) -> Res<Configuration> {
    let file = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("Could not open config file '{}'. Error: {}", filename, err);
            return Err(ConfigError::new(&msg).into());
        }
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(err) => {
            let msg = format!("Could not read config file '{}'. Error: {}", filename, err);
            return Err(ConfigError::new(&msg).into());
        }
    };
    let mut configuration: Configuration = match serde_yaml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            let msg = format!("Invalid config file!\n{}", err);
            return Err(ConfigError::new(&msg).into());
        }
    };
    //Ok(configuration)
    apply_overrides(&mut configuration);
    replace_tokens_in_config(&configuration)
}

fn apply_overrides(configuration: &mut Configuration) {
    if let Some(rate) = OVERRIDES.read().unwrap().samplerate {
        debug!("Apply override for samplerate: {}", rate);
        let cfg_rate = configuration.devices.samplerate;
        configuration.devices.samplerate = rate;
        match &mut configuration.devices.capture {
            CaptureDevice::File { extra_samples, .. } => {
                let new_extra = *extra_samples * rate / cfg_rate;
                debug!("Scale extra samples: {} -> {}", *extra_samples, new_extra);
                *extra_samples = new_extra;
            }
            CaptureDevice::Stdin { extra_samples, .. } => {
                let new_extra = *extra_samples * rate / cfg_rate;
                debug!("Scale extra samples: {} -> {}", *extra_samples, new_extra);
                *extra_samples = new_extra;
            }
            _ => {}
        }
    }
    if let Some(rate) = OVERRIDES.read().unwrap().capture_samplerate {
        debug!("Apply override for capture_samplerate: {}", rate);
        let cfg_rate = configuration.devices.capture_samplerate;
        configuration.devices.capture_samplerate = rate;
        if cfg_rate > 0 {
            match &mut configuration.devices.capture {
                CaptureDevice::File { extra_samples, .. } => {
                    let new_extra = *extra_samples * rate / cfg_rate;
                    debug!("Scale extra samples: {} -> {}", *extra_samples, new_extra);
                    *extra_samples = new_extra;
                }
                CaptureDevice::Stdin { extra_samples, .. } => {
                    let new_extra = *extra_samples * rate / cfg_rate;
                    debug!("Scale extra samples: {} -> {}", *extra_samples, new_extra);
                    *extra_samples = new_extra;
                }
                _ => {}
            }
        }
    }
    if let Some(extra) = OVERRIDES.read().unwrap().extra_samples {
        debug!("Apply override for extra_samples: {}", extra);
        match &mut configuration.devices.capture {
            CaptureDevice::File { extra_samples, .. } => {
                *extra_samples = extra;
            }
            CaptureDevice::Stdin { extra_samples, .. } => {
                *extra_samples = extra;
            }
            _ => {}
        }
    }
    if let Some(chans) = OVERRIDES.read().unwrap().channels {
        debug!("Apply override for capture channels: {}", chans);
        match &mut configuration.devices.capture {
            CaptureDevice::File { channels, .. } => {
                *channels = chans;
            }
            CaptureDevice::Stdin { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
            CaptureDevice::Alsa { channels, .. } => {
                *channels = chans;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
            CaptureDevice::CoreAudio { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
            CaptureDevice::Wasapi { channels, .. } => {
                *channels = chans;
            }
        }
    }
    if let Some(fmt) = OVERRIDES.read().unwrap().sample_format.clone() {
        debug!("Apply override for capture sample format: {}", fmt);
        match &mut configuration.devices.capture {
            CaptureDevice::File { format, .. } => {
                *format = fmt;
            }
            CaptureDevice::Stdin { format, .. } => {
                *format = fmt;
            }
            #[cfg(all(feature = "alsa-backend", target_os = "linux"))]
            CaptureDevice::Alsa { format, .. } => {
                *format = fmt;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { format, .. } => {
                *format = fmt;
            }
            #[cfg(all(feature = "cpal-backend", target_os = "macos"))]
            CaptureDevice::CoreAudio { format, .. } => {
                *format = fmt;
            }
            #[cfg(all(feature = "cpal-backend", target_os = "windows"))]
            CaptureDevice::Wasapi { format, .. } => {
                *format = fmt;
            }
        }
    }
}

fn replace_tokens(
    string: &str,
    samplerate: usize,
    channels: usize,
    format: SampleFormat,
) -> String {
    let srate = format!("{}", samplerate);
    let ch = format!("{}", channels);
    let fmt = format!("{}", format);
    string
        .replace("$samplerate$", &srate)
        .replace("$channels$", &ch)
        .replace("$format$", &fmt)
}

fn replace_tokens_in_config(config: &Configuration) -> Res<Configuration> {
    let samplerate = config.devices.samplerate;
    let num_channels = config.devices.capture.channels();
    let sformat = config.devices.capture.sampleformat();
    let mut new_filters = config.filters.clone();
    for (_name, filter) in new_filters.iter_mut() {
        let modified_filter = match filter {
            Filter::Conv { parameters } => {
                let params = match parameters {
                    ConvParameters::File {
                        filename,
                        skip_bytes_lines,
                        read_bytes_lines,
                        format,
                    } => ConvParameters::File {
                        filename: replace_tokens(
                            filename,
                            samplerate,
                            num_channels,
                            sformat.clone(),
                        ),
                        skip_bytes_lines: *skip_bytes_lines,
                        read_bytes_lines: *read_bytes_lines,
                        format: format.clone(),
                    },
                    _ => parameters.clone(),
                };
                Filter::Conv { parameters: params }
            }
            _ => filter.clone(),
        };
        *filter = modified_filter;
    }
    let new_pipeline = config.pipeline.clone();
    for mut step in new_pipeline {
        match &mut step {
            PipelineStep::Filter { names, .. } => {
                for name in names.iter_mut() {
                    *name = replace_tokens(name, samplerate, num_channels, sformat.clone());
                }
            }
            PipelineStep::Mixer { name } => {
                *name = replace_tokens(name, samplerate, num_channels, sformat.clone());
            }
        }
    }

    let mut new_config = config.clone();
    new_config.filters = new_filters;
    Ok(new_config)
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
            | (Filter::BiquadCombo { .. }, Filter::BiquadCombo { .. })
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
        let msg = format!(
            "target_level can't be larger than {}",
            2 * conf.devices.chunksize
        );
        return Err(ConfigError::new(&msg).into());
    }
    if conf.devices.adjust_period <= 0.0 {
        return Err(ConfigError::new("adjust_period must be positive and > 0").into());
    }
    let mut num_channels = conf.devices.capture.channels();
    let fs = conf.devices.samplerate;
    for step in conf.pipeline {
        match step {
            PipelineStep::Mixer { name } => {
                if !conf.mixers.contains_key(&name) {
                    let msg = format!("Use of missing mixer '{}'", name);
                    return Err(ConfigError::new(&msg).into());
                } else {
                    let chan_in = conf.mixers.get(&name).unwrap().channels.r#in;
                    if chan_in != num_channels {
                        let msg = format!(
                            "Mixer '{}' has wrong number of input channels. Expected {}, found {}.",
                            name, num_channels, chan_in
                        );
                        return Err(ConfigError::new(&msg).into());
                    }
                    num_channels = conf.mixers.get(&name).unwrap().channels.out;
                    match mixer::validate_mixer(&conf.mixers.get(&name).unwrap()) {
                        Ok(_) => {}
                        Err(err) => {
                            let msg = format!("Invalid mixer '{}'. Reason: {}", name, err);
                            return Err(ConfigError::new(&msg).into());
                        }
                    }
                }
            }
            PipelineStep::Filter { channel, names } => {
                if channel >= num_channels {
                    let msg = format!("Use of non existing channel {}", channel);
                    return Err(ConfigError::new(&msg).into());
                }
                for name in names {
                    if !conf.filters.contains_key(&name) {
                        let msg = format!("Use of missing filter '{}'", name);
                        return Err(ConfigError::new(&msg).into());
                    }
                    match filters::validate_filter(fs, &conf.filters.get(&name).unwrap()) {
                        Ok(_) => {}
                        Err(err) => {
                            let msg = format!("Invalid filter '{}'. Reason: {}", name, err);
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
            "Pipeline outputs {} channels, playback device has {}.",
            num_channels, num_channels_out
        );
        return Err(ConfigError::new(&msg).into());
    }
    Ok(())
}
