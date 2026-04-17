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

mod utils;

use self::utils::validate_nonzero_usize;
use crate::utils::wavtools::{WavParams, find_data_in_wav_stream};
use serde::{Deserialize, Serialize};
//use serde_with;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::BufReader;

//type SmpFmt = i16;
use crate::PrcFmt;
pub type ConfigError = self::utils::ConfigErrorType;
pub type Overrides = self::utils::OverridesState;
pub use self::utils::OVERRIDES;

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
// Similar to BinarySampleFormat, but also includes TEXT
pub enum FileSampleFormat {
    TEXT,
    S16_LE,
    S24_4_RJ_LE,
    S24_4_LJ_LE,
    S24_3_LE,
    S32_LE,
    F32_LE,
    F64_LE,
}

impl fmt::Display for FileSampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let formatstr = match self {
            FileSampleFormat::F32_LE => "F32_LE",
            FileSampleFormat::F64_LE => "F64_LE",
            FileSampleFormat::S16_LE => "S16_LE",
            FileSampleFormat::S24_4_RJ_LE => "S24_4_RJ_LE",
            FileSampleFormat::S24_4_LJ_LE => "S24_4_LJ_LE",
            FileSampleFormat::S24_3_LE => "S24_3_LE",
            FileSampleFormat::S32_LE => "S32_LE",
            FileSampleFormat::TEXT => "TEXT",
        };
        write!(f, "{formatstr}")
    }
}

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum BinarySampleFormat {
    /// Signed integer, 16 bits in 2 bytes, little-endian
    S16_LE,
    /// Signed integer, 24 bits in 4 bytes (padded), right justified, little-endian
    S24_4_RJ_LE,
    /// Signed integer, 24 bits in 4 bytes (padded), left justified, little-endian
    S24_4_LJ_LE,
    /// Signed integer, 24 bits in 3 bytes (packed), little-endian
    S24_3_LE,
    /// Signed integer, 32 bits in 4 bytes, little-endian
    S32_LE,
    /// Single precision floating point, 32 bits in 4 bytes, little-endian
    F32_LE,
    /// Double precision floating point, 64 bits in 8 bytes, little-endian
    F64_LE,
}

impl BinarySampleFormat {
    pub fn bits_per_sample(&self) -> usize {
        match self {
            BinarySampleFormat::S16_LE => 16,
            BinarySampleFormat::S24_4_RJ_LE => 24,
            BinarySampleFormat::S24_4_LJ_LE => 24,
            BinarySampleFormat::S24_3_LE => 24,
            BinarySampleFormat::S32_LE => 32,
            BinarySampleFormat::F32_LE => 32,
            BinarySampleFormat::F64_LE => 64,
        }
    }

    pub fn bytes_per_sample(&self) -> usize {
        match self {
            BinarySampleFormat::S16_LE => 2,
            BinarySampleFormat::S24_4_RJ_LE => 4,
            BinarySampleFormat::S24_4_LJ_LE => 4,
            BinarySampleFormat::S24_3_LE => 3,
            BinarySampleFormat::S32_LE => 4,
            BinarySampleFormat::F32_LE => 4,
            BinarySampleFormat::F64_LE => 8,
        }
    }

    pub fn from_file_sample_format(sample_format: &FileSampleFormat) -> Self {
        match sample_format {
            FileSampleFormat::S16_LE => Self::S16_LE,
            FileSampleFormat::S24_4_RJ_LE => Self::S24_4_RJ_LE,
            FileSampleFormat::S24_4_LJ_LE => Self::S24_4_LJ_LE,
            FileSampleFormat::S24_3_LE => Self::S24_3_LE,
            FileSampleFormat::S32_LE => Self::S32_LE,
            FileSampleFormat::F32_LE => Self::F32_LE,
            FileSampleFormat::F64_LE => Self::F64_LE,
            FileSampleFormat::TEXT => unreachable!(),
        }
    }

    pub fn to_file_sample_format(&self) -> FileSampleFormat {
        match self {
            Self::S16_LE => FileSampleFormat::S16_LE,
            Self::S24_4_RJ_LE => FileSampleFormat::S24_4_RJ_LE,
            Self::S24_4_LJ_LE => FileSampleFormat::S24_4_LJ_LE,
            Self::S24_3_LE => FileSampleFormat::S24_3_LE,
            Self::S32_LE => FileSampleFormat::S32_LE,
            Self::F32_LE => FileSampleFormat::F32_LE,
            Self::F64_LE => FileSampleFormat::F64_LE,
        }
    }

    pub fn from_name(label: &str) -> Option<BinarySampleFormat> {
        match label {
            "F32_LE" => Some(Self::F32_LE),
            "F64_LE" => Some(Self::F64_LE),
            "S16_LE" => Some(Self::S16_LE),
            "S24_4_RJ_LE" => Some(Self::S24_4_RJ_LE),
            "S24_4_LJ_LE" => Some(Self::S24_4_LJ_LE),
            "S24_3_LE" => Some(Self::S24_3_LE),
            "S32_LE" => Some(Self::S32_LE),
            _ => None,
        }
    }
}

impl fmt::Display for BinarySampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let formatstr = match self {
            BinarySampleFormat::F32_LE => "F32_LE",
            BinarySampleFormat::F64_LE => "F64_LE",
            BinarySampleFormat::S16_LE => "S16_LE",
            BinarySampleFormat::S24_4_RJ_LE => "S24_4_RJ_LE",
            BinarySampleFormat::S24_4_LJ_LE => "S24_4_LJ_LE",
            BinarySampleFormat::S24_3_LE => "S24_3_LE",
            BinarySampleFormat::S32_LE => "S32_LE",
        };
        write!(f, "{formatstr}")
    }
}

// API specific sample format enums

#[cfg(target_os = "windows")]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum WasapiSampleFormat {
    S16,
    S24,
    S32,
    F32,
}

#[cfg(target_os = "windows")]
impl WasapiSampleFormat {
    // Map binary format to the a corresponding wasapi format, if possible.
    // Used for overriding config values.
    pub fn from_binary_format(format: &BinarySampleFormat) -> Option<Self> {
        match format {
            BinarySampleFormat::S16_LE => Some(Self::S16),
            BinarySampleFormat::S24_3_LE => Some(Self::S24),
            BinarySampleFormat::S24_4_LJ_LE => Some(Self::S24),
            BinarySampleFormat::S24_4_RJ_LE => Some(Self::S24),
            BinarySampleFormat::S32_LE => Some(Self::S32),
            BinarySampleFormat::F32_LE => Some(Self::F32),
            _ => None,
        }
    }
}

#[cfg(all(target_os = "windows", feature = "asio-backend"))]
#[allow(clippy::upper_case_acronyms, non_camel_case_types)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum AsioSampleFormat {
    S16_LE,
    S24_4_LE,
    S24_3_LE,
    S32_LE,
    F32_LE,
    F64_LE,
}

#[cfg(all(target_os = "windows", feature = "asio-backend"))]
impl AsioSampleFormat {
    // Map binary format to the corresponding ASIO format, if possible.
    // Used for overriding config values.
    pub fn from_binary_format(format: &BinarySampleFormat) -> Option<Self> {
        match format {
            BinarySampleFormat::S16_LE => Some(Self::S16_LE),
            BinarySampleFormat::S24_3_LE => Some(Self::S24_3_LE),
            BinarySampleFormat::S24_4_LJ_LE => Some(Self::S24_4_LE),
            BinarySampleFormat::S24_4_RJ_LE => Some(Self::S24_4_LE),
            BinarySampleFormat::S32_LE => Some(Self::S32_LE),
            BinarySampleFormat::F32_LE => Some(Self::F32_LE),
            BinarySampleFormat::F64_LE => Some(Self::F64_LE),
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum CoreAudioSampleFormat {
    S16,
    S24,
    S32,
    F32,
}

#[cfg(target_os = "macos")]
impl CoreAudioSampleFormat {
    // Map binary format to the a corresponding Core Audio format, if possible.
    // Used for overriding config values.
    pub fn from_binary_format(format: &BinarySampleFormat) -> Option<Self> {
        match format {
            BinarySampleFormat::S16_LE => Some(Self::S16),
            BinarySampleFormat::S24_3_LE => Some(Self::S24),
            BinarySampleFormat::S24_4_LJ_LE => Some(Self::S24),
            BinarySampleFormat::S24_4_RJ_LE => Some(Self::S24),
            BinarySampleFormat::S32_LE => Some(Self::S32),
            BinarySampleFormat::F32_LE => Some(Self::F32),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum AlsaSampleFormat {
    /// SND_PCM_FORMAT_S16_LE
    S16_LE,
    ///SND_PCM_FORMAT_S24_3LE
    S24_3_LE,
    /// SND_PCM_FORMAT_S24_LE
    S24_4_LE,
    /// SND_PCM_FORMAT_S32_LE
    S32_LE,
    /// SND_PCM_FORMAT_FLOAT_LE
    F32_LE,
    /// SND_PCM_FORMAT_FLOAT64_LE
    F64_LE,
}

#[cfg(target_os = "linux")]
impl AlsaSampleFormat {
    // Map binary format to the a corresponding Alsa format, if possible.
    // Used for overriding config values.
    pub fn from_binary_format(format: &BinarySampleFormat) -> Self {
        match format {
            BinarySampleFormat::S16_LE => Self::S16_LE,
            BinarySampleFormat::S24_3_LE => Self::S24_3_LE,
            BinarySampleFormat::S24_4_RJ_LE => Self::S24_4_LE,
            BinarySampleFormat::S24_4_LJ_LE => Self::S24_4_LE,
            BinarySampleFormat::S32_LE => Self::S32_LE,
            BinarySampleFormat::F32_LE => Self::F32_LE,
            BinarySampleFormat::F64_LE => Self::F64_LE,
        }
    }

    // Map the Alsa format to the corresponding binary format
    pub fn to_binary_format(&self) -> BinarySampleFormat {
        match self {
            Self::S16_LE => BinarySampleFormat::S16_LE,
            Self::S24_3_LE => BinarySampleFormat::S24_3_LE,
            Self::S24_4_LE => BinarySampleFormat::S24_4_RJ_LE,
            Self::S32_LE => BinarySampleFormat::S32_LE,
            Self::F32_LE => BinarySampleFormat::F32_LE,
            Self::F64_LE => BinarySampleFormat::F64_LE,
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
        format: Option<AlsaSampleFormat>,
        #[serde(default)]
        stop_on_inactive: Option<bool>,
        #[serde(default)]
        link_volume_control: Option<String>,
        #[serde(default)]
        link_mute_control: Option<String>,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
        #[serde(default)]
        buffersize: Option<usize>,
        #[serde(default)]
        period: Option<usize>,
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
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
    },
    #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
    #[serde(alias = "PIPEWIRE", alias = "pipewire")]
    PipeWire {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        #[serde(default)]
        node_name: Option<String>,
        #[serde(default)]
        node_description: Option<String>,
        #[serde(default)]
        node_group_name: Option<String>,
        #[serde(default)]
        labels: Option<Vec<Option<String>>>,
        #[serde(default)]
        autoconnect_to: Option<String>,
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
    #[cfg(all(target_os = "windows", feature = "asio-backend"))]
    #[serde(alias = "ASIO", alias = "asio")]
    Asio(CaptureDeviceAsio),
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
            #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
            CaptureDevice::PipeWire { channels, .. } => *channels,
            CaptureDevice::RawFile(dev) => dev.channels,
            CaptureDevice::WavFile(dev) => {
                dev.wav_info().map(|info| info.channels).unwrap_or_default()
            }
            CaptureDevice::Stdin(dev) => dev.channels,
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => dev.channels,
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => dev.channels,
            #[cfg(all(target_os = "windows", feature = "asio-backend"))]
            CaptureDevice::Asio(dev) => dev.channels,
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

    pub fn labels(&self) -> Option<Vec<Option<String>>> {
        match self {
            #[cfg(target_os = "linux")]
            CaptureDevice::Alsa { labels, .. } => labels.clone(),
            #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
            CaptureDevice::Bluez(dev) => dev.labels.clone(),
            #[cfg(feature = "pulse-backend")]
            CaptureDevice::Pulse { labels, .. } => labels.clone(),
            #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
            CaptureDevice::PipeWire { labels, .. } => labels.clone(),
            CaptureDevice::RawFile(dev) => dev.labels.clone(),
            CaptureDevice::WavFile(dev) => dev.labels.clone(),
            CaptureDevice::Stdin(dev) => dev.labels.clone(),
            #[cfg(target_os = "macos")]
            CaptureDevice::CoreAudio(dev) => dev.labels.clone(),
            #[cfg(target_os = "windows")]
            CaptureDevice::Wasapi(dev) => dev.labels.clone(),
            #[cfg(all(target_os = "windows", feature = "asio-backend"))]
            CaptureDevice::Asio(dev) => dev.labels.clone(),
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
            CaptureDevice::Jack { labels, .. } => labels.clone(),
            CaptureDevice::SignalGenerator { labels, .. } => labels.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceRawFile {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub filename: String,
    pub format: BinarySampleFormat,
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

    pub fn wav_info(&self) -> crate::Res<WavParams> {
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
    pub format: BinarySampleFormat,
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
    pub format: BinarySampleFormat,
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
    pub format: Option<WasapiSampleFormat>,
    #[serde(default)]
    exclusive: Option<bool>,
    #[serde(default)]
    loopback: Option<bool>,
    #[serde(default)]
    polling: Option<bool>,
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

    pub fn is_polling(&self) -> bool {
        self.polling.unwrap_or_default()
    }
}

#[cfg(all(target_os = "windows", feature = "asio-backend"))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceAsio {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: String,
    #[serde(default)]
    pub format: Option<AsioSampleFormat>,
    #[serde(default)]
    pub labels: Option<Vec<Option<String>>>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureDeviceCA {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    #[serde(default)]
    pub format: Option<CoreAudioSampleFormat>,
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
        format: Option<AlsaSampleFormat>,
        #[serde(default)]
        buffersize: Option<usize>,
        #[serde(default)]
        period: Option<usize>,
    },
    #[cfg(feature = "pulse-backend")]
    #[serde(alias = "PULSE", alias = "pulse")]
    Pulse {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        device: String,
    },
    #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
    #[serde(alias = "PIPEWIRE", alias = "pipewire")]
    PipeWire {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        #[serde(default)]
        node_name: Option<String>,
        #[serde(default)]
        node_description: Option<String>,
        #[serde(default)]
        node_group_name: Option<String>,
        #[serde(default)]
        autoconnect_to: Option<String>,
    },
    #[serde(alias = "FILE", alias = "file")]
    File {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        filename: String,
        format: BinarySampleFormat,
        #[serde(default)]
        wav_header: Option<bool>,
    },
    #[serde(alias = "STDOUT", alias = "stdout")]
    Stdout {
        #[serde(deserialize_with = "validate_nonzero_usize")]
        channels: usize,
        format: BinarySampleFormat,
        #[serde(default)]
        wav_header: Option<bool>,
    },
    #[cfg(target_os = "macos")]
    #[serde(alias = "COREAUDIO", alias = "coreaudio")]
    CoreAudio(PlaybackDeviceCA),
    #[cfg(target_os = "windows")]
    #[serde(alias = "WASAPI", alias = "wasapi")]
    Wasapi(PlaybackDeviceWasapi),
    #[cfg(all(target_os = "windows", feature = "asio-backend"))]
    #[serde(alias = "ASIO", alias = "asio")]
    Asio(PlaybackDeviceAsio),
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
            #[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
            PlaybackDevice::PipeWire { channels, .. } => *channels,
            PlaybackDevice::File { channels, .. } => *channels,
            PlaybackDevice::Stdout { channels, .. } => *channels,
            #[cfg(target_os = "macos")]
            PlaybackDevice::CoreAudio(dev) => dev.channels,
            #[cfg(target_os = "windows")]
            PlaybackDevice::Wasapi(dev) => dev.channels,
            #[cfg(all(target_os = "windows", feature = "asio-backend"))]
            PlaybackDevice::Asio(dev) => dev.channels,
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
    #[serde(default)]
    pub format: Option<WasapiSampleFormat>,
    #[serde(default)]
    exclusive: Option<bool>,
    #[serde(default)]
    polling: Option<bool>,
}

#[cfg(target_os = "windows")]
impl PlaybackDeviceWasapi {
    pub fn is_exclusive(&self) -> bool {
        self.exclusive.unwrap_or_default()
    }

    pub fn is_polling(&self) -> bool {
        self.polling.unwrap_or_default()
    }
}

#[cfg(all(target_os = "windows", feature = "asio-backend"))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaybackDeviceAsio {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: String,
    #[serde(default)]
    pub format: Option<AsioSampleFormat>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlaybackDeviceCA {
    #[serde(deserialize_with = "validate_nonzero_usize")]
    pub channels: usize,
    pub device: Option<String>,
    #[serde(default)]
    pub format: Option<CoreAudioSampleFormat>,
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
    format: Option<FileSampleFormat>,
    #[serde(default)]
    skip_bytes_lines: Option<usize>,
    #[serde(default)]
    read_bytes_lines: Option<usize>,
}

impl ConvParametersRaw {
    pub fn format(&self) -> FileSampleFormat {
        self.format.unwrap_or(FileSampleFormat::TEXT)
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
    #[serde(rename = "us")]
    Microseconds,
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
    RACE {
        #[serde(default)]
        description: Option<String>,
        parameters: RACEParameters,
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
pub struct RACEParameters {
    pub channels: usize,
    pub channel_a: usize,
    pub channel_b: usize,
    pub delay: PrcFmt,
    #[serde(default)]
    pub subsample_delay: Option<bool>,
    #[serde(default)]
    pub delay_unit: Option<TimeUnit>,
    pub attenuation: PrcFmt,
}

impl RACEParameters {
    pub fn subsample_delay(&self) -> bool {
        self.subsample_delay.unwrap_or_default()
    }

    pub fn delay_unit(&self) -> TimeUnit {
        self.delay_unit.unwrap_or(TimeUnit::Milliseconds)
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

pub use self::utils::capture_channel_labels;
pub use self::utils::config_diff;
pub use self::utils::load_config;
pub use self::utils::load_validate_config;
pub use self::utils::playback_channel_labels;
pub use self::utils::used_capture_channels;
pub use self::utils::validate_config;
