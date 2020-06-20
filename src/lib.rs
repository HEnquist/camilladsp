#[cfg(feature = "alsa-backend")]
extern crate alsa;
extern crate clap;
#[cfg(feature = "FFTW")]
extern crate fftw;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate num;
extern crate rand;
extern crate rand_distr;
#[cfg(not(feature = "FFTW"))]
extern crate realfft;
extern crate rubato;
extern crate serde;
extern crate serde_with;
extern crate signal_hook;
#[cfg(feature = "socketserver")]
extern crate ws;

#[macro_use]
extern crate log;
extern crate env_logger;

use std::error;

// Sample format
#[cfg(feature = "32bit")]
pub type PrcFmt = f32;
#[cfg(not(feature = "32bit"))]
pub type PrcFmt = f64;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

#[cfg(feature = "alsa-backend")]
pub mod alsadevice;
pub mod audiodevice;
pub mod basicfilters;
pub mod biquad;
pub mod biquadcombo;
pub mod config;
pub mod conversions;
pub mod diffeq;
pub mod dither;
#[cfg(not(feature = "FFTW"))]
pub mod fftconv;
#[cfg(feature = "FFTW")]
pub mod fftconv_fftw;
pub mod fifoqueue;
pub mod filedevice;
pub mod filters;
pub mod mixer;
pub mod processing;
#[cfg(feature = "pulse-backend")]
pub mod pulsedevice;
#[cfg(feature = "socketserver")]
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

pub enum ExitStatus {
    Restart,
    Exit,
}
