use crate::compressor;
use crate::filters;
use crate::mixer;
use crate::noisegate;
use crate::wavtools::{find_data_in_wav_stream, WavParams};
use parking_lot::RwLock;
use serde::{de, Deserialize, Serialize};
//use serde_with;
use std::collections::HashMap;
use std::error;
use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::path::{Path, PathBuf};

//type SmpFmt = i16;
use crate::PrcFmt;
type Res<T> = Result<T, Box<dyn error::Error>>;

#[derive(Clone)]
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
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
        write!(f, "{formatstr}")
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
// Similar to SampleFormat, but also includes TEXT
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

    pub fn from_sample_format(sample_format: &SampleFormat) -> Self {
        match sample_format {
            SampleFormat::S16LE => FileFormat::S16LE,
            SampleFormat::S24LE => FileFormat::S24LE,
            SampleFormat::S24LE3 => FileFormat::S24LE3,
            SampleFormat::S32LE => FileFormat::S32LE,
            SampleFormat::FLOAT32LE => FileFormat::FLOAT32LE,
            SampleFormat::FLOAT64LE => FileFormat::FLOAT64LE,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum Signal {
    Sine { freq: f64, level: PrcFmt },
    Square { freq: f64, level: PrcFmt },
    WhiteNoise { level: PrcFmt },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum CaptureDevice {
    #[cfg(target_os = "linux")]
    #[serde(alias = "ALSA", alias = "alsa")]
    Alsa {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        #[serde(default)]
        format: Option<SampleFormat>,
        #[serde(default)]
        stop_on_inactive: Option<bool>,
        #[serde(default)]
        link_volume_control: Option<String>,
        #[serde(default)]
        link_mute_control: Option<String>,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
    },
    #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
    #[serde(alias = "BLUEZ", alias = "bluez")]
    Bluez(CaptureDeviceBluez),
    #[cfg(feature = "pulse-backend")]
    #[serde(alias = "PULSE", alias = "pulse")]
    Pulse {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        format: SampleFormat,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
    },
    RawFile(CaptureDeviceRawFile),
    WavFile(CaptureDeviceWavFile),
    #[serde(alias = "STDIN", alias = "stdin")]
    Stdin(CaptureDeviceStdin),
    #[cfg(target_os = "macos")]
    #[serde(alias = "COREAUDIO", alias = "coreaudio")]
    CoreAudio(CaptureDeviceCA),
    #[cfg(target_os = "windows")]
    #[serde(alias = "WASAPI", alias = "wasapi")]
    Wasapi(CaptureDeviceWasapi),
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
    #[serde(alias = "JACK", alias = "jack")]
    Jack {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
    },
    SignalGenerator {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        signal: Signal,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
    },
}

impl CaptureDevice {
    pub fn channels(&self) -> usize {
        match self {
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { channels, .. } => *channels,
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez(dev) => dev.channels,
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { channels, .. } => *channels,
            CaptureDevice::RawFile(dev) => dev.channels,
            CaptureDevice::WavFile(dev) => {
                dev.wav_info().map(|info| info.channels).unwrap_or_default()
            }
            CaptureDevice::Stdin(dev) => dev.channels,
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => dev.channels,
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => dev.channels,
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
            CaptureDevice::Jack { channels, .. } => *channels,
            CaptureDevice::SignalGenerator { channels, .. } => *channels,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceRawFile {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub filename: String,
    pub format: SampleFormat,
    #[serde(default)]
    pub extra_samples: Option<usize>,
    #[serde(default)]
    pub skip_bytes: Option<usize>,
    #[serde(default)]
    pub read_bytes: Option<usize>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

impl CaptureDeviceRawFile {
    pub fn extra_samples(&self) -> usize {
        self.extra_samples.unwrap_or_default()
    }
    pub fn skip_bytes(&self) -> usize {
        self.skip_bytes.unwrap_or_default()
    }
    pub fn read_bytes(&self) -> usize {
        self.read_bytes.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceWavFile {
    pub filename: String,
    #[serde(default)]
    pub extra_samples: Option<usize>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

impl CaptureDeviceWavFile {
    pub fn extra_samples(&self) -> usize {
        self.extra_samples.unwrap_or_default()
    }

    pub fn wav_info(&self) -> Res<WavParams> {
        let fname = &self.filename;
        let f = match File::open(fname) {
            Ok(f) => f,
            Err(err) => {
                let msg = format!("Could not open input file '{fname}'. Reason: {err}");
                return Err(ConfigError::new(&msg).into());
            }
        };
        let file = BufReader::new(&f);
        find_data_in_wav_stream(file)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceStdin {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub format: SampleFormat,
    #[serde(default)]
    pub extra_samples: Option<usize>,
    #[serde(default)]
    pub skip_bytes: Option<usize>,
    #[serde(default)]
    pub read_bytes: Option<usize>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

impl CaptureDeviceStdin {
    pub fn extra_samples(&self) -> usize {
        self.extra_samples.unwrap_or_default()
    }
    pub fn skip_bytes(&self) -> usize {
        self.skip_bytes.unwrap_or_default()
    }
    pub fn read_bytes(&self) -> usize {
        self.read_bytes.unwrap_or_default()
    }
}

#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceBluez {
    #[serde(default)]
    service: Option<String>,
    // TODO: Allow the user to specify mac address rather than D-Bus path
    pub dbus_path: String,
    // TODO: sample format, sample rate and channel count should be determined
    // from D-Bus properties
    pub format: SampleFormat,
    pub channels: usize,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
impl CaptureDeviceBluez {
    pub fn service(&self) -> String {
        self.service.clone().unwrap_or("org.bluealsa".to_string())
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceWasapi {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    pub format: SampleFormat,
    #[serde(default)]
    exclusive: Option<bool>,
    #[serde(default)]
    loopback: Option<bool>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

#[cfg(target_os = "windows")]
impl CaptureDeviceWasapi {
    pub fn is_exclusive(&self) -> bool {
        self.exclusive.unwrap_or_default()
    }

    pub fn is_loopback(&self) -> bool {
        self.loopback.unwrap_or_default()
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceCA {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    #[serde(default)]
    pub format: Option<SampleFormat>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
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
        #[serde(default)]
        format: Option<SampleFormat>,
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
        wav_header: Option<bool>,
    },
    #[serde(alias = "STDOUT", alias = "stdout")]
    Stdout {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        format: SampleFormat,
        #[serde(default)]
        wav_header: Option<bool>,
    },
    #[cfg(target_os = "macos")]
    #[serde(alias = "COREAUDIO", alias = "coreaudio")]
    CoreAudio(PlaybackDeviceCA),
    #[cfg(target_os = "windows")]
    #[serde(alias = "WASAPI", alias = "wasapi")]
    Wasapi(PlaybackDeviceWasapi),
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
            PlaybackDevice::CoreAudio(dev) => dev.channels,
            #[cfg(target_os = "windows")]
            PlaybackDevice::Wasapi(dev) => dev.channels,
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
            PlaybackDevice::Jack { channels, .. } => *channels,
        }
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaybackDeviceWasapi {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    pub format: SampleFormat,
    #[serde(default)]
    exclusive: Option<bool>,
}

#[cfg(target_os = "windows")]
impl PlaybackDeviceWasapi {
    pub fn is_exclusive(&self) -> bool {
        self.exclusive.unwrap_or_default()
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaybackDeviceCA {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    #[serde(default)]
    pub format: Option<SampleFormat>,
    #[serde(default)]
    exclusive: Option<bool>,
}

#[cfg(target_os = "macos")]
impl PlaybackDeviceCA {
    pub fn is_exclusive(&self) -> bool {
        self.exclusive.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Devices {
    pub samplerate: usize,
    pub chunksize: usize,
    #[serde(default)]
    pub queuelimit: Option<usize>,
    #[serde(default)]
    pub silence_threshold: Option<PrcFmt>,
    #[serde(default)]
    pub silence_timeout: Option<PrcFmt>,
    pub capture: CaptureDevice,
    pub playback: PlaybackDevice,
    #[serde(default)]
    pub enable_rate_adjust: Option<bool>,
    #[serde(default)]
    pub target_level: Option<usize>,
    #[serde(default)]
    pub adjust_period: Option<f32>,
    #[serde(default)]
    pub resampler: Option<Resampler>,
    #[serde(default)]
    pub capture_samplerate: Option<usize>,
    #[serde(default)]
    pub stop_on_rate_change: Option<bool>,
    #[serde(default)]
    pub rate_measure_interval: Option<f32>,
    #[serde(default)]
    pub volume_ramp_time: Option<f32>,
    #[serde(default)]
    pub volume_limit: Option<f32>,
    #[serde(default)]
    pub multithreaded: Option<bool>,
    #[serde(default)]
    pub worker_threads: Option<usize>,
}

// Getters for all the defaults
impl Devices {
    pub fn queuelimit(&self) -> usize {
        self.queuelimit.unwrap_or(4)
    }

    pub fn adjust_period(&self) -> f32 {
        self.adjust_period.unwrap_or(10.0)
    }

    pub fn rate_measure_interval(&self) -> f32 {
        self.rate_measure_interval.unwrap_or(1.0)
    }

    pub fn silence_threshold(&self) -> PrcFmt {
        self.silence_threshold.unwrap_or(0.0)
    }

    pub fn silence_timeout(&self) -> PrcFmt {
        self.silence_timeout.unwrap_or(0.0)
    }

    pub fn capture_samplerate(&self) -> usize {
        self.capture_samplerate.unwrap_or(self.samplerate)
    }

    pub fn target_level(&self) -> usize {
        self.target_level.unwrap_or(self.chunksize)
    }

    pub fn stop_on_rate_change(&self) -> bool {
        self.stop_on_rate_change.unwrap_or(false)
    }

    pub fn rate_adjust(&self) -> bool {
        self.enable_rate_adjust.unwrap_or(false)
    }

    pub fn ramp_time(&self) -> f32 {
        self.volume_ramp_time.unwrap_or(400.0)
    }

    pub fn volume_limit(&self) -> f32 {
        self.volume_limit.unwrap_or(50.0)
    }

    pub fn multithreaded(&self) -> bool {
        self.multithreaded.unwrap_or(false)
    }

    pub fn worker_threads(&self) -> usize {
        self.worker_threads.unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum AsyncSincInterpolation {
    Nearest,
    Linear,
    Quadratic,
    Cubic,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum AsyncSincProfile {
    VeryFast,
    Fast,
    Balanced,
    Accurate,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum AsyncSincWindow {
    Hann,
    Hann2,
    Blackman,
    Blackman2,
    BlackmanHarris,
    BlackmanHarris2,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum AsyncPolyInterpolation {
    Linear,
    Cubic,
    Quintic,
    Septic,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
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
        description: Option<String>,
        parameters: ConvParameters,
    },
    Biquad {
        #[serde(default)]
        description: Option<String>,
        parameters: BiquadParameters,
    },
    BiquadCombo {
        #[serde(default)]
        description: Option<String>,
        parameters: BiquadComboParameters,
    },
    Delay {
        #[serde(default)]
        description: Option<String>,
        parameters: DelayParameters,
    },
    Gain {
        #[serde(default)]
        description: Option<String>,
        parameters: GainParameters,
    },
    Volume {
        #[serde(default)]
        description: Option<String>,
        parameters: VolumeParameters,
    },
    Loudness {
        #[serde(default)]
        description: Option<String>,
        parameters: LoudnessParameters,
    },
    Dither {
        #[serde(default)]
        description: Option<String>,
        parameters: DitherParameters,
    },
    DiffEq {
        #[serde(default)]
        description: Option<String>,
        parameters: DiffEqParameters,
    },
    Limiter {
        #[serde(default)]
        description: Option<String>,
        parameters: LimiterParameters,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum ConvParameters {
    Raw(ConvParametersRaw),
    Wav(ConvParametersWav),
    Values {
        values: Vec<PrcFmt>,
    },
    Dummy {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        length: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConvParametersRaw {
    pub filename: String,
    #[serde(default)]
    format: Option<FileFormat>,
    #[serde(default)]
    skip_bytes_lines: Option<usize>,
    #[serde(default)]
    read_bytes_lines: Option<usize>,
}

impl ConvParametersRaw {
    pub fn format(&self) -> FileFormat {
        self.format.unwrap_or(FileFormat::TEXT)
    }

    pub fn skip_bytes_lines(&self) -> usize {
        self.skip_bytes_lines.unwrap_or_default()
    }

    pub fn read_bytes_lines(&self) -> usize {
        self.read_bytes_lines.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConvParametersWav {
    pub filename: String,
    #[serde(default)]
    channel: Option<usize>,
}

impl ConvParametersWav {
    pub fn channel(&self) -> usize {
        self.channel.unwrap_or_default()
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GeneralNotchParams {
    pub freq_p: PrcFmt,
    pub freq_z: PrcFmt,
    pub q_p: PrcFmt,
    #[serde(default)]
    pub normalize_at_dc: Option<bool>,
}

impl GeneralNotchParams {
    pub fn normalize_at_dc(&self) -> bool {
        self.normalize_at_dc.unwrap_or_default()
    }
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
    GeneralNotch(GeneralNotchParams),
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
    Tilt {
        gain: PrcFmt,
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
    GraphicEqualizer(GraphicEqualizerParameters),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphicEqualizerParameters {
    #[serde(default)]
    freq_min: Option<f32>,
    #[serde(default)]
    freq_max: Option<f32>,
    pub gains: Vec<f32>,
}

impl GraphicEqualizerParameters {
    pub fn freq_min(&self) -> f32 {
        self.freq_min.unwrap_or(20.0)
    }

    pub fn freq_max(&self) -> f32 {
        self.freq_max.unwrap_or(20000.0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum VolumeFader {
    Aux1 = 1,
    Aux2 = 2,
    Aux3 = 3,
    Aux4 = 4,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VolumeParameters {
    #[serde(default)]
    pub ramp_time: Option<f32>,
    pub fader: VolumeFader,
    pub limit: Option<f32>,
}

impl VolumeParameters {
    pub fn ramp_time(&self) -> f32 {
        self.ramp_time.unwrap_or(400.0)
    }

    pub fn limit(&self) -> f32 {
        self.limit.unwrap_or(50.0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum LoudnessFader {
    Main = 0,
    Aux1 = 1,
    Aux2 = 2,
    Aux3 = 3,
    Aux4 = 4,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoudnessParameters {
    pub reference_level: f32,
    #[serde(default)]
    pub high_boost: Option<f32>,
    #[serde(default)]
    pub low_boost: Option<f32>,
    #[serde(default)]
    pub fader: Option<LoudnessFader>,
    #[serde(default)]
    pub attenuate_mid: Option<bool>,
}

impl LoudnessParameters {
    pub fn high_boost(&self) -> f32 {
        self.high_boost.unwrap_or(10.0)
    }

    pub fn low_boost(&self) -> f32 {
        self.low_boost.unwrap_or(10.0)
    }

    pub fn fader(&self) -> usize {
        self.fader.unwrap_or(LoudnessFader::Main) as usize
    }

    pub fn attenuate_mid(&self) -> bool {
        self.attenuate_mid.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GainParameters {
    pub gain: PrcFmt,
    #[serde(default)]
    pub inverted: Option<bool>,
    #[serde(default)]
    pub mute: Option<bool>,
    #[serde(default)]
    pub scale: Option<GainScale>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum GainScale {
    #[serde(rename = "linear")]
    Linear,
    #[serde(rename = "dB")]
    Decibel,
}

impl GainParameters {
    pub fn is_inverted(&self) -> bool {
        self.inverted.unwrap_or_default()
    }

    pub fn is_mute(&self) -> bool {
        self.mute.unwrap_or_default()
    }

    pub fn scale(&self) -> GainScale {
        self.scale.unwrap_or(GainScale::Decibel)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DelayParameters {
    pub delay: PrcFmt,
    #[serde(default)]
    pub unit: Option<TimeUnit>,
    #[serde(default)]
    pub subsample: Option<bool>,
}

impl DelayParameters {
    pub fn unit(&self) -> TimeUnit {
        self.unit.unwrap_or(TimeUnit::Milliseconds)
    }

    pub fn subsample(&self) -> bool {
        self.subsample.unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum TimeUnit {
    #[serde(rename = "ms")]
    Milliseconds,
    #[serde(rename = "mm")]
    Millimetres,
    #[serde(rename = "samples")]
    Samples,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum DitherParameters {
    None { bits: usize },
    Flat { bits: usize, amplitude: PrcFmt },
    Highpass { bits: usize },
    Fweighted441 { bits: usize },
    FweightedLong441 { bits: usize },
    FweightedShort441 { bits: usize },
    Gesemann441 { bits: usize },
    Gesemann48 { bits: usize },
    Lipshitz441 { bits: usize },
    LipshitzLong441 { bits: usize },
    Shibata441 { bits: usize },
    ShibataHigh441 { bits: usize },
    ShibataLow441 { bits: usize },
    Shibata48 { bits: usize },
    ShibataHigh48 { bits: usize },
    ShibataLow48 { bits: usize },
    Shibata882 { bits: usize },
    ShibataLow882 { bits: usize },
    Shibata96 { bits: usize },
    ShibataLow96 { bits: usize },
    Shibata192 { bits: usize },
    ShibataLow192 { bits: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiffEqParameters {
    #[serde(default)]
    pub a: Option<Vec<PrcFmt>>,
    #[serde(default)]
    pub b: Option<Vec<PrcFmt>>,
}

impl DiffEqParameters {
    pub fn a(&self) -> Vec<PrcFmt> {
        self.a.clone().unwrap_or_default()
    }

    pub fn b(&self) -> Vec<PrcFmt> {
        self.b.clone().unwrap_or_default()
    }
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
    pub gain: Option<PrcFmt>,
    #[serde(default)]
    pub inverted: Option<bool>,
    #[serde(default)]
    pub mute: Option<bool>,
    #[serde(default)]
    pub scale: Option<GainScale>,
}

impl MixerSource {
    pub fn gain(&self) -> PrcFmt {
        self.gain.unwrap_or_default()
    }

    pub fn is_inverted(&self) -> bool {
        self.inverted.unwrap_or_default()
    }

    pub fn is_mute(&self) -> bool {
        self.mute.unwrap_or_default()
    }

    pub fn scale(&self) -> GainScale {
        self.scale.unwrap_or(GainScale::Decibel)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MixerMapping {
    pub dest: usize,
    pub sources: Vec<MixerSource>,
    #[serde(default)]
    pub mute: Option<bool>,
}

impl MixerMapping {
    pub fn is_mute(&self) -> bool {
        self.mute.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mixer {
    #[serde(default)]
    pub description: Option<String>,
    pub channels: MixerChannels,
    pub mapping: Vec<MixerMapping>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum Processor {
    Compressor {
        #[serde(default)]
        description: Option<String>,
        parameters: CompressorParameters,
    },
    NoiseGate {
        #[serde(default)]
        description: Option<String>,
        parameters: NoiseGateParameters,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompressorParameters {
    pub channels: usize,
    #[serde(default)]
    pub monitor_channels: Option<Vec<usize>>,
    #[serde(default)]
    pub process_channels: Option<Vec<usize>>,
    pub attack: PrcFmt,
    pub release: PrcFmt,
    pub threshold: PrcFmt,
    pub factor: PrcFmt,
    #[serde(default)]
    pub makeup_gain: Option<PrcFmt>,
    #[serde(default)]
    pub soft_clip: Option<bool>,
    #[serde(default)]
    pub clip_limit: Option<PrcFmt>,
}

impl CompressorParameters {
    pub fn monitor_channels(&self) -> Vec<usize> {
        self.monitor_channels.clone().unwrap_or_default()
    }

    pub fn process_channels(&self) -> Vec<usize> {
        self.process_channels.clone().unwrap_or_default()
    }

    pub fn makeup_gain(&self) -> PrcFmt {
        self.makeup_gain.unwrap_or_default()
    }

    pub fn soft_clip(&self) -> bool {
        self.soft_clip.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseGateParameters {
    pub channels: usize,
    #[serde(default)]
    pub monitor_channels: Option<Vec<usize>>,
    #[serde(default)]
    pub process_channels: Option<Vec<usize>>,
    pub attack: PrcFmt,
    pub release: PrcFmt,
    pub threshold: PrcFmt,
    pub attenuation: PrcFmt,
}

impl NoiseGateParameters {
    pub fn monitor_channels(&self) -> Vec<usize> {
        self.monitor_channels.clone().unwrap_or_default()
    }

    pub fn process_channels(&self) -> Vec<usize> {
        self.process_channels.clone().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimiterParameters {
    #[serde(default)]
    pub soft_clip: Option<bool>,
    #[serde(default)]
    pub clip_limit: PrcFmt,
}

impl LimiterParameters {
    pub fn soft_clip(&self) -> bool {
        self.soft_clip.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum PipelineStep {
    Mixer(PipelineStepMixer),
    Filter(PipelineStepFilter),
    Processor(PipelineStepProcessor),
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PipelineStepMixer {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub bypassed: Option<bool>,
}

impl PipelineStepMixer {
    pub fn is_bypassed(&self) -> bool {
        self.bypassed.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PipelineStepFilter {
    #[serde(default)]
    pub channels: Option<Vec<usize>>,
    pub names: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub bypassed: Option<bool>,
}

impl PipelineStepFilter {
    pub fn is_bypassed(&self) -> bool {
        self.bypassed.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PipelineStepProcessor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub bypassed: Option<bool>,
}

impl PipelineStepProcessor {
    pub fn is_bypassed(&self) -> bool {
        self.bypassed.unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub devices: Devices,
    #[serde(default)]
    pub mixers: Option<HashMap<String, Mixer>>,
    #[serde(default)]
    pub filters: Option<HashMap<String, Filter>>,
    #[serde(default)]
    pub processors: Option<HashMap<String, Processor>>,
    #[serde(default)]
    pub pipeline: Option<Vec<PipelineStep>>,
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
    let configuration: Configuration = match serde_yaml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            let msg = format!("Invalid config file!\n{err}");
            return Err(ConfigError::new(&msg).into());
        }
    };
    Ok(configuration)
}

fn apply_overrides(configuration: &mut Configuration) {
    let mut overrides = OVERRIDES.read().clone();
    // Only one match arm for now, might be more later.
    #[allow(clippy::single_match)]
    match &configuration.devices.capture {
        CaptureDevice::WavFile(dev) => {
            if let Ok(wav_info) = dev.wav_info() {
                overrides.channels = Some(wav_info.channels);
                overrides.sample_format = Some(wav_info.sample_format);
                overrides.samplerate = Some(wav_info.sample_rate);
                debug!("Updating overrides with values from wav input file, rate {}, format: {}, channels: {}", wav_info.sample_rate, wav_info.sample_format, wav_info.channels);
            }
        }
        _ => {}
    }
    if let Some(rate) = overrides.samplerate {
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
            debug!("Apply override for capture_samplerate: {}", rate);
            configuration.devices.capture_samplerate = Some(rate);
            if rate == cfg_rate && !configuration.devices.rate_adjust() {
                debug!("Disabling unneccesary 1:1 resampling");
                configuration.devices.resampler = None;
            }
        }
    }
    if let Some(extra) = overrides.extra_samples {
        debug!("Apply override for extra_samples: {}", extra);
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
        debug!("Apply override for capture channels: {}", chans);
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
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => {
                dev.channels = chans;
            }
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => {
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
        debug!("Apply override for capture sample format: {}", fmt);
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
                *format = Some(fmt);
            }
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez(dev) => {
                dev.format = fmt;
            }
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { format, .. } => {
                *format = fmt;
            }
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => {
                dev.format = Some(fmt);
            }
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => {
                dev.format = fmt;
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
    apply_overrides(conf);
    replace_tokens_in_config(conf);
    if let Some(fname) = filename {
        replace_relative_paths_in_config(conf, fname);
    }
    #[cfg(target_os = "linux")]
    let target_level_limit = if matches!(conf.devices.playback, PlaybackDevice::Alsa { .. }) {
        4 * conf.devices.chunksize
    } else {
        2 * conf.devices.chunksize
    };
    #[cfg(not(target_os = "linux"))]
    let target_level_limit = 2 * conf.devices.chunksize;

    if conf.devices.target_level() > target_level_limit {
        let msg = format!("target_level cannot be larger than {}", target_level_limit);
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
        if dev.format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The Wasapi capture backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let CaptureDevice::Wasapi(dev) = &conf.devices.capture {
        if dev.format != SampleFormat::FLOAT32LE && !dev.is_exclusive() {
            return Err(ConfigError::new(
                "Wasapi shared mode capture must use FLOAT32LE sample format",
            )
            .into());
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
        if dev.format == SampleFormat::FLOAT64LE {
            return Err(ConfigError::new(
                "The Wasapi playback backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "windows")]
    if let PlaybackDevice::Wasapi(dev) = &conf.devices.playback {
        if dev.format != SampleFormat::FLOAT32LE && !dev.is_exclusive() {
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
    if let CaptureDevice::CoreAudio(dev) = &conf.devices.capture {
        if dev.format == Some(SampleFormat::FLOAT64LE) {
            return Err(ConfigError::new(
                "The CoreAudio capture backend does not support FLOAT64LE sample format",
            )
            .into());
        }
    }
    #[cfg(target_os = "macos")]
    if let PlaybackDevice::CoreAudio(dev) = &conf.devices.playback {
        if dev.format == Some(SampleFormat::FLOAT64LE) {
            return Err(ConfigError::new(
                "The CoreAudio playback backend does not support FLOAT64LE sample format",
            )
            .into());
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
                                    let msg = format!("Use of non existing channel {}", channel);
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
