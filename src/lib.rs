#[cfg(target_os = "linux")]
extern crate alsa;
#[cfg(target_os = "linux")]
extern crate alsa_sys;
extern crate clap;
#[cfg(feature = "cpal-backend")]
extern crate cpal;
#[cfg(feature = "FFTW")]
extern crate fftw;
#[macro_use]
extern crate lazy_static;
#[cfg(target_os = "macos")]
extern crate coreaudio;
extern crate crossbeam_channel;
#[cfg(target_os = "macos")]
extern crate dispatch;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
#[cfg(feature = "secure-websocket")]
extern crate native_tls;
#[cfg(target_os = "linux")]
extern crate nix;
extern crate num_complex;
extern crate num_traits;
extern crate rand;
extern crate rand_distr;
extern crate rawsample;
#[cfg(not(feature = "FFTW"))]
extern crate realfft;
extern crate rubato;
extern crate serde;
extern crate serde_with;
extern crate signal_hook;
#[cfg(feature = "websocket")]
extern crate tungstenite;
#[cfg(target_os = "windows")]
extern crate wasapi;
//#[cfg(target_os = "windows")]
//extern crate winapi;

#[macro_use]
extern crate log;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::error;
use std::fmt;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};

// Sample format
#[cfg(feature = "32bit")]
pub type PrcFmt = f32;
#[cfg(not(feature = "32bit"))]
pub type PrcFmt = f64;

pub trait NewValue<T> {
    fn coerce(val: T) -> Self;
}

impl<PrcFmt> NewValue<PrcFmt> for PrcFmt {
    fn coerce(val: PrcFmt) -> PrcFmt {
        val
    }
}

pub type Res<T> = Result<T, Box<dyn error::Error>>;

#[cfg(target_os = "linux")]
pub mod alsadevice;
#[cfg(target_os = "linux")]
pub mod alsadevice_buffermanager;
#[cfg(target_os = "linux")]
pub mod alsadevice_utils;
pub mod audiodevice;
pub mod basicfilters;
pub mod biquad;
pub mod biquadcombo;
pub mod compressor;
pub mod config;
pub mod conversions;
#[cfg(target_os = "macos")]
pub mod coreaudiodevice;
pub mod countertimer;
#[cfg(feature = "cpal-backend")]
pub mod cpaldevice;
pub mod diffeq;
pub mod dither;
#[cfg(not(feature = "FFTW"))]
pub mod fftconv;
#[cfg(feature = "FFTW")]
pub mod fftconv_fftw;
pub mod filedevice;
#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
pub mod filedevice_bluez;
#[cfg(not(target_os = "linux"))]
pub mod filereader;
#[cfg(target_os = "linux")]
pub mod filereader_nonblock;
pub mod filters;
pub mod helpers;
pub mod limiter;
pub mod loudness;
pub mod mixer;
pub mod processing;
#[cfg(feature = "pulse-backend")]
pub mod pulsedevice;
#[cfg(feature = "websocket")]
pub mod socketserver;
pub mod statefile;
#[cfg(target_os = "windows")]
pub mod wasapidevice;

pub enum StatusMessage {
    PlaybackReady,
    CaptureReady,
    PlaybackError(String),
    CaptureError(String),
    PlaybackFormatChange(usize),
    CaptureFormatChange(usize),
    PlaybackDone,
    CaptureDone,
    SetSpeed(f64),
}

pub enum CommandMessage {
    SetSpeed { speed: f64 },
    Exit,
}

#[derive(Debug)]
pub enum ExitState {
    Restart,
    Exit,
}

pub enum ControllerMessage {
    // Config must be boxed, to prevent "large size difference between variants" warning
    ConfigChanged(Box<config::Configuration>),
    Stop,
    Exit,
}

#[derive(Clone, Debug, Copy, Serialize, Eq, PartialEq)]
pub enum ProcessingState {
    // Processing is running normally.
    Running,
    // Processing is paused because input is silent.
    Paused,
    // Processing is off and devices are closed, waiting for a new config.
    Inactive,
    // Opening devices and starting up processing.
    Starting,
    // Capture device isnt providing data, processing is stalled.
    Stalled,
}

#[derive(Clone, Debug)]
pub struct CaptureStatus {
    pub update_interval: usize,
    pub measured_samplerate: usize,
    pub signal_range: f32,
    pub signal_rms: countertimer::ValueHistory,
    pub signal_peak: countertimer::ValueHistory,
    pub state: ProcessingState,
    pub rate_adjust: f32,
    pub used_channels: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    pub update_interval: usize,
    pub clipped_samples: usize,
    pub buffer_level: usize,
    pub signal_rms: countertimer::ValueHistory,
    pub signal_peak: countertimer::ValueHistory,
}

#[derive(Debug)]
pub struct ProcessingParameters {
    // Optimization: volumes are actually `f32`s, but by representing their
    // bits as a `u32` of equal size we can use atomic operations instead of a
    // mutex.
    target_volume: [AtomicU32; Self::NUM_FADERS],
    current_volume: [AtomicU32; Self::NUM_FADERS],
    mute: [AtomicBool; Self::NUM_FADERS],
    processing_load: AtomicU32,
}

impl ProcessingParameters {
    pub const NUM_FADERS: usize = 5;

    pub const DEFAULT_VOLUME: f32 = 0.0;
    pub const DEFAULT_MUTE: bool = false;

    pub fn new(initial_volumes: &[f32; 5], initial_mutes: &[bool; 5]) -> Self {
        Self {
            target_volume: [
                AtomicU32::new(initial_volumes[0].to_bits()),
                AtomicU32::new(initial_volumes[1].to_bits()),
                AtomicU32::new(initial_volumes[2].to_bits()),
                AtomicU32::new(initial_volumes[3].to_bits()),
                AtomicU32::new(initial_volumes[4].to_bits()),
            ],
            current_volume: [
                AtomicU32::new(initial_volumes[0].to_bits()),
                AtomicU32::new(initial_volumes[1].to_bits()),
                AtomicU32::new(initial_volumes[2].to_bits()),
                AtomicU32::new(initial_volumes[3].to_bits()),
                AtomicU32::new(initial_volumes[4].to_bits()),
            ],
            mute: [
                AtomicBool::new(initial_mutes[0]),
                AtomicBool::new(initial_mutes[1]),
                AtomicBool::new(initial_mutes[2]),
                AtomicBool::new(initial_mutes[3]),
                AtomicBool::new(initial_mutes[4]),
            ],
            processing_load: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn target_volume(&self, fader: usize) -> f32 {
        f32::from_bits(self.target_volume[fader].load(Ordering::Relaxed))
    }

    pub fn set_target_volume(&self, fader: usize, target: f32) {
        self.target_volume[fader].store(target.to_bits(), Ordering::Relaxed)
    }

    pub fn current_volume(&self, fader: usize) -> f32 {
        f32::from_bits(self.current_volume[fader].load(Ordering::Relaxed))
    }

    pub fn set_current_volume(&self, fader: usize, current: f32) {
        self.current_volume[fader].store(current.to_bits(), Ordering::Relaxed)
    }

    pub fn is_mute(&self, fader: usize) -> bool {
        self.mute[fader].load(Ordering::Relaxed)
    }

    pub fn set_mute(&self, fader: usize, mute: bool) {
        self.mute[fader].store(mute, Ordering::Relaxed)
    }

    pub fn toggle_mute(&self, fader: usize) -> bool {
        self.mute[fader].fetch_xor(true, Ordering::Relaxed)
    }

    pub fn volumes(&self) -> [f32; Self::NUM_FADERS] {
        [
            f32::from_bits(self.target_volume[0].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[1].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[2].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[3].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[4].load(Ordering::Relaxed)),
        ]
    }

    pub fn mutes(&self) -> [bool; Self::NUM_FADERS] {
        [
            self.mute[0].load(Ordering::Relaxed),
            self.mute[1].load(Ordering::Relaxed),
            self.mute[2].load(Ordering::Relaxed),
            self.mute[3].load(Ordering::Relaxed),
            self.mute[4].load(Ordering::Relaxed),
        ]
    }

    pub fn set_processing_load(&self, load: f32) {
        self.processing_load
            .store(load.to_bits(), Ordering::Relaxed)
    }

    pub fn processing_load(&self) -> f32 {
        f32::from_bits(self.processing_load.load(Ordering::Relaxed))
    }
}

impl Default for ProcessingParameters {
    fn default() -> Self {
        Self::new(
            &[
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
            ],
            &[
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
            ],
        )
    }
}

#[derive(Clone, Debug)]
pub struct ProcessingStatus {
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum StopReason {
    None,
    Done,
    CaptureError(String),
    PlaybackError(String),
    UnknownError(String),
    CaptureFormatChange(usize),
    PlaybackFormatChange(usize),
}

#[derive(Clone)]
pub struct StatusStructs {
    pub capture: Arc<RwLock<CaptureStatus>>,
    pub playback: Arc<RwLock<PlaybackStatus>>,
    pub processing: Arc<ProcessingParameters>,
    pub status: Arc<RwLock<ProcessingStatus>>,
}

pub struct SharedConfigs {
    pub active: Arc<Mutex<Option<config::Configuration>>>,
    pub previous: Arc<Mutex<Option<config::Configuration>>>,
}

impl fmt::Display for ProcessingState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = match self {
            ProcessingState::Running => "RUNNING",
            ProcessingState::Paused => "PAUSED",
            ProcessingState::Inactive => "INACTIVE",
            ProcessingState::Starting => "STARTING",
            ProcessingState::Stalled => "STALLED",
        };
        write!(f, "{desc}")
    }
}

pub fn list_supported_devices() -> (Vec<String>, Vec<String>) {
    let mut playbacktypes = vec!["File".to_owned(), "Stdout".to_owned()];
    let mut capturetypes = vec!["File".to_owned(), "Stdin".to_owned()];

    if cfg!(target_os = "linux") {
        playbacktypes.push("Alsa".to_owned());
        capturetypes.push("Alsa".to_owned());
    }
    if cfg!(feature = "pulse-backend") {
        playbacktypes.push("Pulse".to_owned());
        capturetypes.push("Pulse".to_owned());
    }
    if cfg!(feature = "bluez-backend") {
        capturetypes.push("Bluez".to_owned());
    }
    if cfg!(feature = "jack-backend") {
        playbacktypes.push("Jack".to_owned());
        capturetypes.push("Jack".to_owned());
    }
    if cfg!(target_os = "macos") {
        playbacktypes.push("CoreAudio".to_owned());
        capturetypes.push("CoreAudio".to_owned());
    }
    if cfg!(target_os = "windows") {
        playbacktypes.push("Wasapi".to_owned());
        capturetypes.push("Wasapi".to_owned());
    }
    (playbacktypes, capturetypes)
}

// Return a list of supported devices.
// Returns two strings per device, the device name and a readable name.
// Some backends do not make a diference between these, and return the same name twice.
pub fn list_available_devices(backend: &str, input: bool) -> Vec<(String, String)> {
    match backend {
        #[cfg(target_os = "linux")]
        "Alsa" => alsadevice_utils::list_device_names(input),
        #[cfg(target_os = "macos")]
        "CoreAudio" => coreaudiodevice::list_available_devices(input),
        #[cfg(target_os = "windows")]
        "Wasapi" => wasapidevice::list_device_names(input),
        _ => Vec::new(),
    }
}
