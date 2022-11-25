use crate::compressor;
use crate::filters;
use crate::mixer;
use serde::{de, Deserialize, Serialize};
use serde_with;
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

//type SmpFmt = i16;
use crate::PrcFmt;
type Res<T> = Result<T, Box<dyn error::Error>>;

pub struct Overrides {
    pub samplerate: Option<usize>,
    pub sample_format: Option<SampleFormat>,
    pub extra_samples: Option<usize>,
    pub channels: Option<usize>,
}

lazy_static! {
    pub static ref OVERRIDES: RwLock<Overrides> = RwLock::new(Overrides {
        samplerate: None,
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

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum SampleFormat {
    S16LE,
    S24LE,
    S24LE3,
    S32LE,
    FLOAT32LE,
    FLOAT64LE,
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

#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
fn bluez_default_srv() -> String {
    "org.bluealsa".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum CaptureDevice {
    #[cfg(target_os = "linux")]
    #[serde(alias = "ALSA", alias = "alsa")]
    Alsa {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
    #[serde(alias = "BLUEZ", alias = "bluez")]
    Bluez {
        #[serde(default = "bluez_default_srv")]
        service: String,
        // TODO: Allow the user to specify mac address rather than D-Bus path
        dbus_path: String,
        // TODO: sample format, sample rate and channel count should be determined
        // from D-Bus properties
        format: SampleFormat,
        channels: usize,
    },
    #[cfg(feature = "pulse-backend")]
    #[serde(alias = "PULSE", alias = "pulse")]
    Pulse {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[serde(alias = "FILE", alias = "file")]
    File {
        #[serde(deserialize_with = "validate_nonzero_usize")]
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
    #[serde(alias = "STDIN", alias = "stdin")]
    Stdin {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        format: SampleFormat,
        #[serde(default)]
        extra_samples: usize,
        #[serde(default)]
        skip_bytes: usize,
        #[serde(default)]
        read_bytes: usize,
    },
    #[cfg(target_os = "macos")]
    #[serde(alias = "COREAUDIO", alias = "coreaudio")]
    CoreAudio {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        #[serde(default = "default_ca_format")]
        format: SampleFormat,
        #[serde(default)]
        change_format: bool,
    },
    #[cfg(target_os = "windows")]
    #[serde(alias = "WASAPI", alias = "wasapi")]
    Wasapi {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
        #[serde(default)]
        exclusive: bool,
        #[serde(default)]
        loopback: bool,
    },
    #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
    #[serde(alias = "JACK", alias = "jack")]
    Jack {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
    },
}

impl CaptureDevice {
    pub fn channels(&self) -> usize {
        match self {
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { channels, .. } => *channels,
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez { channels, .. } => *channels,
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => *channels,
            CaptureDevice::File { channels, .. } => *channels,
            CaptureDevice::Stdin { channels, .. } => *channels,
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio { channels, .. } => *channels,
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
            CaptureDevice::Jack { channels, .. } => *channels,
        }
    }

    pub fn sampleformat(&self) -> SampleFormat {
        match self {
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { format, .. } => format.clone(),
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez { format, .. } => format.clone(),
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { format, .. } => format.clone(),
            CaptureDevice::File { format, .. } => format.clone(),
            CaptureDevice::Stdin { format, .. } => format.clone(),
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio { format, .. } => format.clone(),
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi { format, .. } => format.clone(),
            #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
            CaptureDevice::Jack { .. } => SampleFormat::FLOAT32LE,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum PlaybackDevice {
    #[cfg(target_os = "linux")]
    #[serde(alias = "ALSA", alias = "alsa")]
    Alsa {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[cfg(feature = "pulse-backend")]
    #[serde(alias = "PULSE", alias = "pulse")]
    Pulse {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
    },
    #[serde(alias = "FILE", alias = "file")]
    File {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        filename: String,
        format: SampleFormat,
    },
    #[serde(alias = "STDOUT", alias = "stdout")]
    Stdout {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        format: SampleFormat,
    },
    #[cfg(target_os = "macos")]
    #[serde(alias = "COREAUDIO", alias = "coreaudio")]
    CoreAudio {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        #[serde(default = "default_ca_format")]
        format: SampleFormat,
        #[serde(default)]
        change_format: bool,
        #[serde(default)]
        exclusive: bool,
    },
    #[cfg(target_os = "windows")]
    #[serde(alias = "WASAPI", alias = "wasapi")]
    Wasapi {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
        #[serde(default)]
        exclusive: bool,
    },
    #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
    #[serde(alias = "JACK", alias = "jack")]
    Jack {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
    },
}

impl PlaybackDevice {
    pub fn channels(&self) -> usize {
        match self {
            #[cfg(target_os = "linux")]
            PlaybackDevice::Alsa { channels, .. } => *channels,
            #[cfg(feature = "pulse-backend")]
            PlaybackDevice::Pulse { channels, .. } => *channels,
            PlaybackDevice::File { channels, .. } => *channels,
            PlaybackDevice::Stdout { channels, .. } => *channels,
            #[cfg(target_os = "macos")]
            PlaybackDevice::CoreAudio { channels, .. } => *channels,
            #[cfg(target_os = "windows")]
            PlaybackDevice::Wasapi { channels, .. } => *channels,
            #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
            PlaybackDevice::Jack { channels, .. } => *channels,
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
    pub resampler: Option<Resampler>,
    #[serde(default)]
    pub capture_samplerate: Option<usize>,
    #[serde(default)]
    pub stop_on_rate_change: bool,
    #[serde(default = "default_measure_interval")]
    pub rate_measure_interval: f32,
}

fn default_period() -> f32 {
    10.0
}

fn default_queuelimit() -> usize {
    4
}

fn default_measure_interval() -> f32 {
    1.0
}

#[cfg(target_os = "macos")]
fn default_ca_format() -> SampleFormat {
    SampleFormat::S32LE
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum AsyncSincInterpolation {
    Nearest,
    Linear,
    Quadratic,
    Cubic,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum AsyncSincProfile {
    VeryFast,
    Fast,
    Balanced,
    Accurate,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
#[serde(deny_unknown_fields)]
pub enum AsyncSincParameters {
    Profile {
        profile: AsyncSincProfile,
    },
    Free {
        sinc_len: usize,
        interpolation: AsyncSincInterpolation,
        window: AsyncSincWindow,
        f_cutoff: Option<f32>,
        oversampling_factor: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum AsyncSincWindow {
    Hann,
    Hann2,
    Blackman,
    Blackman2,
    BlackmanHarris,
    BlackmanHarris2,
}

impl Default for AsyncSincParameters {
    fn default() -> Self {
        AsyncSincParameters::Profile {
            profile: AsyncSincProfile::Balanced,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum AsyncPolyInterpolation {
    Linear,
    Cubic,
    Quintic,
    Septic,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum Resampler {
    AsyncPoly {
        interpolation: AsyncPolyInterpolation,
    },
    AsyncSinc(AsyncSincParameters),
    Synchronous,
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
    Volume {
        parameters: VolumeParameters,
    },
    Loudness {
        parameters: LoudnessParameters,
    },
    Dither {
        parameters: DitherParameters,
    },
    DiffEq {
        parameters: DiffEqParameters,
    },
    Limiter {
        parameters: LimiterParameters,
    },
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
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

impl FileFormat {
    pub fn bits_per_sample(&self) -> usize {
        match self {
            FileFormat::S16LE => 16,
            FileFormat::S24LE => 24,
            FileFormat::S24LE3 => 24,
            FileFormat::S32LE => 32,
            FileFormat::FLOAT32LE => 32,
            FileFormat::FLOAT64LE => 64,
            FileFormat::TEXT => 0,
        }
    }

    pub fn bytes_per_sample(&self) -> usize {
        match self {
            FileFormat::S16LE => 2,
            FileFormat::S24LE => 4,
            FileFormat::S24LE3 => 3,
            FileFormat::S32LE => 4,
            FileFormat::FLOAT32LE => 4,
            FileFormat::FLOAT64LE => 8,
            FileFormat::TEXT => 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum ConvParameters {
    #[serde(alias = "File")]
    Raw {
        filename: String,
        #[serde(default)]
        format: FileFormat,
        #[serde(default)]
        skip_bytes_lines: usize,
        #[serde(default)]
        read_bytes_lines: usize,
    },
    Wav {
        filename: String,
        #[serde(default)]
        channel: usize,
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
#[serde(untagged)]
pub enum ShelfSteepness {
    Q {
        freq: PrcFmt,
        q: PrcFmt,
        gain: PrcFmt,
    },
    Slope {
        freq: PrcFmt,
        slope: PrcFmt,
        gain: PrcFmt,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PeakingWidth {
    Q {
        freq: PrcFmt,
        q: PrcFmt,
        gain: PrcFmt,
    },
    Bandwidth {
        freq: PrcFmt,
        bandwidth: PrcFmt,
        gain: PrcFmt,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum NotchWidth {
    Q { freq: PrcFmt, q: PrcFmt },
    Bandwidth { freq: PrcFmt, bandwidth: PrcFmt },
}

#[allow(clippy::upper_case_acronyms)]
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
    Peaking(PeakingWidth),
    Highshelf(ShelfSteepness),
    HighshelfFO {
        freq: PrcFmt,
        gain: PrcFmt,
    },
    Lowshelf(ShelfSteepness),
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
    Allpass(NotchWidth),
    AllpassFO {
        freq: PrcFmt,
    },
    Bandpass(NotchWidth),
    Notch(NotchWidth),
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
    LinkwitzRileyHighpass {
        freq: PrcFmt,
        order: usize,
    },
    LinkwitzRileyLowpass {
        freq: PrcFmt,
        order: usize,
    },
    ButterworthHighpass {
        freq: PrcFmt,
        order: usize,
    },
    ButterworthLowpass {
        freq: PrcFmt,
        order: usize,
    },
    FivePointPeq {
        fls: PrcFmt,
        qls: PrcFmt,
        gls: PrcFmt,
        fp1: PrcFmt,
        qp1: PrcFmt,
        gp1: PrcFmt,
        fp2: PrcFmt,
        qp2: PrcFmt,
        gp2: PrcFmt,
        fp3: PrcFmt,
        qp3: PrcFmt,
        gp3: PrcFmt,
        fhs: PrcFmt,
        qhs: PrcFmt,
        ghs: PrcFmt,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VolumeParameters {
    #[serde(default = "default_ramp_time")]
    pub ramp_time: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoudnessParameters {
    #[serde(default = "default_ramp_time")]
    pub ramp_time: f32,
    pub reference_level: f32,
    #[serde(default = "default_loudness_boost")]
    pub high_boost: f32,
    #[serde(default = "default_loudness_boost")]
    pub low_boost: f32,
}

fn default_loudness_boost() -> f32 {
    10.0
}

fn default_ramp_time() -> f32 {
    200.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GainParameters {
    pub gain: PrcFmt,
    #[serde(default)]
    pub inverted: bool,
    #[serde(default)]
    pub mute: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DelayParameters {
    pub delay: PrcFmt,
    #[serde(default)]
    pub unit: TimeUnit,
    #[serde(default)]
    pub subsample: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum TimeUnit {
    #[serde(rename = "ms")]
    Milliseconds,
    #[serde(rename = "mm")]
    Millimetres,
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
    ShibataLow441 { bits: usize },
    ShibataLow48 { bits: usize },
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

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerChannels {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub r#in: usize,
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub out: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerSource {
    pub channel: usize,
    #[serde(default)]
    pub gain: PrcFmt,
    #[serde(default)]
    pub inverted: bool,
    #[serde(default)]
    pub mute: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
    #[serde(default)]
    pub mute: bool,
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
pub enum Processor {
    Compressor { parameters: CompressorParameters },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompressorParameters {
    pub channels: usize,
    #[serde(default)]
    pub monitor_channels: Vec<usize>,
    #[serde(default)]
    pub process_channels: Vec<usize>,
    pub attack: PrcFmt,
    pub release: PrcFmt,
    pub threshold: PrcFmt,
    pub factor: PrcFmt,
    #[serde(default)]
    pub makeup_gain: PrcFmt,
    #[serde(default)]
    pub soft_clip: bool,
    #[serde(default)]
    pub enable_clip: bool,
    #[serde(default)]
    pub clip_limit: PrcFmt,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimiterParameters {
    #[serde(default)]
    pub soft_clip: bool,
    #[serde(default)]
    pub clip_limit: PrcFmt,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum PipelineStep {
    Mixer {
        name: String,
        #[serde(default)]
        bypassed: bool,
    },
    Filter {
        channel: usize,
        names: Vec<String>,
        #[serde(default)]
        bypassed: bool,
    },
    Processor {
        name: String,
        #[serde(default)]
        bypassed: bool,
    },
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
    pub processors: HashMap<String, Processor>,
    #[serde(default)]
    pub pipeline: Vec<PipelineStep>,
}

fn validate_nonzero_usize<'de, D>(d: D) -> Result<usize, D::Error>
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
    let configuration: Configuration = match serde_yaml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            let msg = format!("Invalid config file!\n{}", err);
            return Err(ConfigError::new(&msg).into());
        }
    };
    //Ok(configuration)
    //apply_overrides(&mut configuration);
    //replace_tokens_in_config(&mut configuration);
    //replace_relative_paths_in_config(&mut configuration, filename);
    Ok(configuration)
}

fn apply_overrides(configuration: &mut Configuration) {
    if let Some(rate) = OVERRIDES.read().unwrap().samplerate {
        let cfg_rate = configuration.devices.samplerate;
        let cfg_chunksize = configuration.devices.chunksize;

        if configuration.devices.resampler.is_none() {
            debug!("Apply override for samplerate: {}", rate);
            configuration.devices.samplerate = rate;
            let scaled_chunksize = if rate > cfg_rate {
                cfg_chunksize * (rate as f32 / cfg_rate as f32).round() as usize
            } else {
                cfg_chunksize / (cfg_rate as f32 / rate as f32).round() as usize
            };
            debug!(
                "Samplerate changed, adjusting chunksize: {} -> {}",
                cfg_chunksize, scaled_chunksize
            );
            configuration.devices.chunksize = scaled_chunksize;
            #[allow(unreachable_patterns)]
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
        } else {
            debug!("Apply override for capture_samplerate: {}", rate);
            configuration.devices.capture_samplerate = Some(rate);
            if rate == cfg_rate && !configuration.devices.enable_rate_adjust {
                debug!("Disabling unneccesary 1:1 resampling");
                configuration.devices.resampler = None;
            }
        }
    }
    if let Some(extra) = OVERRIDES.read().unwrap().extra_samples {
        debug!("Apply override for extra_samples: {}", extra);
        #[allow(unreachable_patterns)]
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
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez { channels, .. } => {
                *channels = chans;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => {
                *channels = chans;
            }
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio { channels, .. } => {
                *channels = chans;
            }
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi { channels, .. } => {
                *channels = chans;
            }
            #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
            CaptureDevice::Jack { channels, .. } => {
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
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { format, .. } => {
                *format = fmt;
            }
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez { format, .. } => {
                *format = fmt;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { format, .. } => {
                *format = fmt;
            }
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio { format, .. } => {
                *format = fmt;
            }
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi { format, .. } => {
                *format = fmt;
            }
            #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
            CaptureDevice::Jack { .. } => {
                error!("Not possible to override capture format for Jack, ignoring");
            }
        }
    }
}

fn replace_tokens(string: &str, samplerate: usize, channels: usize) -> String {
    let srate = format!("{}", samplerate);
    let ch = format!("{}", channels);
    string
        .replace("$samplerate$", &srate)
        .replace("$channels$", &ch)
}

fn replace_tokens_in_config(config: &mut Configuration) {
    let samplerate = config.devices.samplerate;
    let num_channels = config.devices.capture.channels();
    for (_name, filter) in config.filters.iter_mut() {
        match filter {
            Filter::Conv {
                parameters: ConvParameters::Raw { filename, .. },
            }
            | Filter::Conv {
                parameters: ConvParameters::Wav { filename, .. },
            } => {
                *filename = replace_tokens(filename, samplerate, num_channels);
            }
            _ => {}
        }
    }
    for mut step in config.pipeline.iter_mut() {
        match &mut step {
            PipelineStep::Filter { names, .. } => {
                for name in names.iter_mut() {
                    *name = replace_tokens(name, samplerate, num_channels);
                }
            }
            PipelineStep::Mixer { name, .. } => {
                *name = replace_tokens(name, samplerate, num_channels);
            }
            PipelineStep::Processor { name, .. } => {
                *name = replace_tokens(name, samplerate, num_channels);
            }
        }
    }
}

// Check if coefficent files with relative paths are relative to the config file path, replace path if they are
fn replace_relative_paths_in_config(config: &mut Configuration, configname: &str) {
    if let Ok(config_file) = PathBuf::from(configname.to_owned()).canonicalize() {
        if let Some(config_dir) = config_file.parent() {
            for (_name, filter) in config.filters.iter_mut() {
                if let Filter::Conv {
                    parameters: ConvParameters::Raw { filename, .. },
                } = filter
                {
                    check_and_replace_relative_path(filename, config_dir);
                } else if let Filter::Conv {
                    parameters: ConvParameters::Wav { filename, .. },
                } = filter
                {
                    check_and_replace_relative_path(filename, config_dir);
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
        trace!("{} is absolute, no change", path_str);
    } else {
        debug!("{} is relative", path_str);
        let mut in_config_dir = config_path.to_path_buf();
        in_config_dir.push(&path_str);
        if in_config_dir.exists() {
            debug!("Using {} found relative to config file dir", path_str);
            *path_str = in_config_dir.to_string_lossy().into();
        } else {
            trace!(
                "{} not found relative to config file dir, not changing path",
                path_str
            );
        }
    }
}

#[derive(Debug)]
pub enum ConfigChange {
    FilterParameters {
        filters: Vec<String>,
        mixers: Vec<String>,
        processors: Vec<String>,
    },
    MixerParameters,
    Pipeline,
    Devices,
    None,
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
    for (filter, params) in &newconf.filters {
        // The pipeline didn't change, any added filter isn't included and can be skipped
        if let Some(current_filter) = currentconf.filters.get(filter) {
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
                | (Filter::Loudness { .. }, Filter::Loudness { .. }) => {}
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
    for (mixer, params) in &newconf.mixers {
        // The pipeline didn't change, any added mixer isn't included and can be skipped
        if let Some(current_mixer) = currentconf.mixers.get(mixer) {
            if params != current_mixer {
                mixers.push(mixer.to_string());
            }
        }
    }
    for (proc, params) in &newconf.processors {
        // The pipeline didn't change, any added compressor isn't included and can be skipped
        if let Some(current_proc) = currentconf.processors.get(proc) {
            if params != current_proc {
                processors.push(proc.to_string());
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
    apply_overrides(conf);
    replace_tokens_in_config(conf);
    if let Some(fname) = filename {
        replace_relative_paths_in_config(conf, fname);
    }

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
    if conf.devices.silence_threshold > 0.0 {
        return Err(ConfigError::new("silence_threshold must be less than or equal to 0").into());
    }
    if conf.devices.silence_timeout < 0.0 {
        return Err(ConfigError::new("silence_timeout cannot be negative").into());
    }
    if let Some(rate) = conf.devices.capture_samplerate {
        if rate != conf.devices.samplerate && conf.devices.resampler.is_none() {
            return Err(ConfigError::new(
                "capture_samplerate must match samplerate when resampling is disabled",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi { format, .. } = &conf.devices.capture {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The Wasapi capture backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi {
        format, exclusive, ..
    } = &conf.devices.capture
    {
        if *format != SampleFormat::FLOAT32LE && !*exclusive {
            return Err(ConfigError::new(
                "Wasapi shared mode capture must use FLOAT32LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi {
        loopback,
        exclusive,
        ..
    } = &conf.devices.capture
    {
        if *loopback && *exclusive {
            return Err(ConfigError::new(
                "Wasapi loopback capture is only supported in shared mode",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let PlaybackDevice::Wasapi { format, .. } = &conf.devices.playback {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The Wasapi playback backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let PlaybackDevice::Wasapi {
        format, exclusive, ..
    } = &conf.devices.playback
    {
        if *format != SampleFormat::FLOAT32LE && !*exclusive {
            return Err(ConfigError::new(
                "Wasapi shared mode playback must use FLOAT32LE sample format",
            )
            .into());
        }
    }
    #[cfg(feature = "pulse-backend")]
    if let CaptureDevice::Pulse { format, .. } = &conf.devices.capture {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The PulseAudio capture backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(feature = "pulse-backend")]
    if let PlaybackDevice::Pulse { format, .. } = &conf.devices.playback {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The PulseAudio playback backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "macos")]
    if let CaptureDevice::CoreAudio { format, .. } = &conf.devices.capture {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The CoreAudio capture backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "macos")]
    if let PlaybackDevice::CoreAudio { format, .. } = &conf.devices.playback {
        if *format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The CoreAudio playback backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    let mut num_channels = conf.devices.capture.channels();
    let fs = conf.devices.samplerate;
    for step in &conf.pipeline {
        match step {
            PipelineStep::Mixer { name, bypassed } => {
                if !*bypassed {
                    if !conf.mixers.contains_key(name) {
                        let msg = format!("Use of missing mixer '{}'", name);
                        return Err(ConfigError::new(&msg).into());
                    } else {
                        let chan_in = conf.mixers.get(name).unwrap().channels.r#in;
                        if chan_in != num_channels {
                            let msg = format!(
                                "Mixer '{}' has wrong number of input channels. Expected {}, found {}.",
                                name, num_channels, chan_in
                            );
                            return Err(ConfigError::new(&msg).into());
                        }
                        num_channels = conf.mixers.get(name).unwrap().channels.out;
                        match mixer::validate_mixer(conf.mixers.get(name).unwrap()) {
                            Ok(_) => {}
                            Err(err) => {
                                let msg = format!("Invalid mixer '{}'. Reason: {}", name, err);
                                return Err(ConfigError::new(&msg).into());
                            }
                        }
                    }
                }
            }
            PipelineStep::Filter {
                channel,
                names,
                bypassed,
            } => {
                if !*bypassed {
                    if *channel >= num_channels {
                        let msg = format!("Use of non existing channel {}", channel);
                        return Err(ConfigError::new(&msg).into());
                    }
                    for name in names {
                        if !conf.filters.contains_key(name) {
                            let msg = format!("Use of missing filter '{}'", name);
                            return Err(ConfigError::new(&msg).into());
                        }
                        match filters::validate_filter(fs, conf.filters.get(name).unwrap()) {
                            Ok(_) => {}
                            Err(err) => {
                                let msg = format!("Invalid filter '{}'. Reason: {}", name, err);
                                return Err(ConfigError::new(&msg).into());
                            }
                        }
                    }
                }
            }
            PipelineStep::Processor { name, bypassed } => {
                if !*bypassed {
                    if !conf.processors.contains_key(name) {
                        let msg = format!("Use of missing processor '{}'", name);
                        return Err(ConfigError::new(&msg).into());
                    } else {
                        let procconf = conf.processors.get(name).unwrap();
                        match procconf {
                            Processor::Compressor { parameters } => {
                                let channels = parameters.channels;
                                if channels != num_channels {
                                    let msg = format!(
                                        "Compressor '{}' has wrong number of channels. Expected {}, found {}.",
                                        name, num_channels, channels
                                    );
                                    return Err(ConfigError::new(&msg).into());
                                }
                                match compressor::validate_compressor(parameters) {
                                    Ok(_) => {}
                                    Err(err) => {
                                        let msg = format!(
                                            "Invalid processor '{}'. Reason: {}",
                                            name, err
                                        );
                                        return Err(ConfigError::new(&msg).into());
                                    }
                                }
                            }
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

/// Get a vector telling which channels are actually used in the pipeline
pub fn get_used_capture_channels(conf: &Configuration) -> Vec<bool> {
    for step in conf.pipeline.iter() {
        if let PipelineStep::Mixer { name, bypassed } = step {
            if !*bypassed {
                let mixerconf = conf.mixers.get(name).unwrap();
                return mixer::get_used_input_channels(mixerconf);
            }
        }
    }
    let capture_channels = conf.devices.capture.channels();
    vec![true; capture_channels]
}
