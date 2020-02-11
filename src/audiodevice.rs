// Traits for audio devices
use std::thread;
use config;
use alsadevice;
use pulsedevice;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};

use Res;
use PrcFmt;
use StatusMessage;

pub enum AudioMessage {
    //Quit,
    Audio(AudioChunk),
}

/// Main container of audio data
pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub maxval: PrcFmt,
    pub minval: PrcFmt,
    pub waveforms: Vec<Vec<PrcFmt>>,
}


/// A playback device
pub trait PlaybackDevice {
    fn start(&mut self, channel: mpsc::Receiver<AudioMessage>, barrier: Arc<Barrier>, status_channel: mpsc::Sender<StatusMessage>) -> Res<Box<thread::JoinHandle<()>>>;
}

/// A capture device
pub trait CaptureDevice {
    fn start(&mut self, channel: mpsc::Sender<AudioMessage>, barrier: Arc<Barrier>, status_channel: mpsc::Sender<StatusMessage>) -> Res<Box<thread::JoinHandle<()>>>;
}

/// Create a playback device.
pub fn get_playback_device(conf: config::Devices) -> Box<dyn PlaybackDevice> {
    match conf.playback.r#type {
        config::DeviceType::Alsa => {
            Box::new(alsadevice::AlsaPlaybackDevice {
                devname: conf.playback.device, 
                samplerate: conf.samplerate, 
                bufferlength: conf.buffersize, 
                channels: conf.playback.channels,
                format: conf.playback.format,
            })
        },
        config::DeviceType::Pulse => {
            Box::new(pulsedevice::PulsePlaybackDevice {
                devname: conf.playback.device, 
                samplerate: conf.samplerate, 
                bufferlength: conf.buffersize, 
                channels: conf.playback.channels,
                format: conf.playback.format,
            })
        },
    }
}

/// Create a capture device. Currently only Alsa is supported.
pub fn get_capture_device(conf: config::Devices) -> Box<dyn CaptureDevice> {
    match conf.capture.r#type {
        config::DeviceType::Alsa => {
            Box::new(alsadevice::AlsaCaptureDevice {
                devname: conf.capture.device, 
                samplerate: conf.samplerate, 
                bufferlength: conf.buffersize, 
                channels: conf.capture.channels,
                format: conf.capture.format,
                silence_threshold: conf.silence_threshold,
                silence_timeout: conf.silence_timeout,
            })
        },
        config::DeviceType::Pulse => {
            Box::new(pulsedevice::PulseCaptureDevice {
                devname: conf.capture.device, 
                samplerate: conf.samplerate, 
                bufferlength: conf.buffersize, 
                channels: conf.capture.channels,
                format: conf.capture.format,
                silence_threshold: conf.silence_threshold,
                silence_timeout: conf.silence_timeout,
            })
        },
    }
}