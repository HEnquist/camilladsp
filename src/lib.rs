#[cfg(all(feature = "alsa-backend", target_os = "linux"))]
extern crate alsa;
extern crate clap;
#[cfg(feature = "cpal-backend")]
extern crate cpal;
#[cfg(feature = "FFTW")]
extern crate fftw;
#[macro_use]
extern crate lazy_static;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
#[cfg(feature = "secure-websocket")]
extern crate native_tls;
#[cfg(all(feature = "alsa-backend", target_os = "linux"))]
extern crate nix;
extern crate num_complex;
extern crate num_integer;
extern crate num_traits;
extern crate rand;
extern crate rand_distr;
#[cfg(not(feature = "FFTW"))]
extern crate realfft;
extern crate rubato;
extern crate serde;
extern crate serde_with;
extern crate signal_hook;
#[cfg(feature = "websocket")]
extern crate tungstenite;

#[macro_use]
extern crate slog_scope;

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

#[cfg(all(feature = "alsa-backend", target_os = "linux"))]
pub mod alsadevice;
pub mod audiodevice;
pub mod basicfilters;
pub mod biquad;
pub mod biquadcombo;
pub mod config;
pub mod conversions;
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
pub mod filters;
pub mod helpers;
pub mod loudness;
pub mod mixer;
pub mod processing;
#[cfg(feature = "pulse-backend")]
pub mod pulsedevice;
#[cfg(feature = "websocket")]
pub mod socketserver;

pub enum StatusMessage {
    PlaybackReady,
    CaptureReady,
    PlaybackError { message: String },
    CaptureError { message: String },
    PlaybackDone,
    CaptureDone,
    SetSpeed { speed: f64 },
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
    Running,
    Paused,
    Inactive,
    Starting,
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
    pub signal_rms: Vec<f32>,
    pub signal_peak: Vec<f32>,
    pub state: ProcessingState,
    pub rate_adjust: f32,
    pub used_channels: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    pub update_interval: usize,
    pub clipped_samples: usize,
    pub buffer_level: usize,
    pub signal_rms: Vec<f32>,
    pub signal_peak: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct ProcessingStatus {
    pub volume: f32,
    pub mute: bool,
}

#[derive(Clone)]
pub struct StatusStructs {
    pub capture: Arc<RwLock<CaptureStatus>>,
    pub playback: Arc<RwLock<PlaybackStatus>>,
    pub processing: Arc<RwLock<ProcessingStatus>>,
}

impl fmt::Display for ProcessingState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = match self {
            ProcessingState::Running => "RUNNING",
            ProcessingState::Paused => "PAUSED",
            ProcessingState::Inactive => "INACTIVE",
            ProcessingState::Starting => "STARTING",
        };
        write!(f, "{}", desc)
    }
}
