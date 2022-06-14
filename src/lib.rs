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
#[cfg(any(target_os = "windows", target_os = "macos"))]
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
extern crate num_integer;
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

use serde::Serialize;
use std::error;
use std::fmt;
use std::sync::{Arc, RwLock};

// Sample format
#[cfg(feature = "32bit")]
pub type PrcFmt = f32;
#[cfg(not(feature = "32bit"))]
pub type PrcFmt = f64;

pub trait NewValue<T> {
    fn new(val: T) -> Self;
}

impl<PrcFmt> NewValue<PrcFmt> for PrcFmt {
    fn new(val: PrcFmt) -> PrcFmt {
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
pub mod fifoqueue;
pub mod filedevice;
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

pub enum ExitState {
    Restart,
    Exit,
}

#[derive(Clone, Debug, Copy, Serialize, PartialEq)]
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

pub struct ExitRequest {}

impl ExitRequest {
    pub const NONE: usize = 0;
    pub const EXIT: usize = 1;
    pub const STOP: usize = 2;
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

#[derive(Clone, Debug)]
pub struct ProcessingParameters {
    pub volume: f32,
    pub mute: bool,
}

#[derive(Clone, Debug)]
pub struct ProcessingStatus {
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
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
    pub processing: Arc<RwLock<ProcessingParameters>>,
    pub status: Arc<RwLock<ProcessingStatus>>,
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
        write!(f, "{}", desc)
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
