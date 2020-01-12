// Traits for audio devices
use std::error;
use std::thread;
use config;
use alsadevice;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
pub type Res<T> = Result<T, Box<dyn error::Error>>;


type PrcFmt = f64;

//pub type S16LE = i16;
//pub type S24LE = i32;
//pub type S32LE = i32;

pub enum AudioMessage {
    Quit,
    Audio(AudioChunk),
}


pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub waveforms: Vec<Vec<PrcFmt>>,
}


pub trait PlaybackDevice {
    fn start(&mut self, channel: mpsc::Receiver<AudioMessage>, barrier: Arc<Barrier>) -> Res<Box<thread::JoinHandle<()>>>;
}

pub trait CaptureDevice {
    fn start(&mut self, channel: mpsc::Sender<AudioMessage>, barrier: Arc<Barrier>) -> Res<Box<thread::JoinHandle<()>>>;
}


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
    }
}

pub fn get_capture_device(conf: config::Devices) -> Box<dyn CaptureDevice> {
    match conf.capture.r#type {
        config::DeviceType::Alsa => {
            Box::new(alsadevice::AlsaCaptureDevice {
                devname: conf.capture.device, 
                samplerate: conf.samplerate, 
                bufferlength: conf.buffersize, 
                channels: conf.capture.channels,
                format: conf.capture.format,
            })
        },
    }
}