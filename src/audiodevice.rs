// Traits for audio devices
use std::error;
use config;
use alsadevice;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

type SmpFmt = i16;
type PrcFmt = f64;

pub type Pcm16 = i16;
pub type Pcm24 = i32;
pub type Pcm32 = i32;


pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub waveforms: Vec<Vec<PrcFmt>>,
}




pub trait PlaybackDevice {
    fn get_bufsize(&mut self) -> usize;

    /// Send audio chunk for later playback
    fn put_chunk(&mut self, chunk: AudioChunk) -> Res<()>;

    // Filter a Vec
    fn play(&mut self) -> Res<usize>;
}

pub trait CaptureDevice {
    fn get_bufsize(&mut self) -> usize;

    /// Filter a single point
    fn fetch_chunk(&mut self) -> Res<AudioChunk>;

    // Filter a Vec
    fn capture(&mut self) -> Res<usize>;
}

pub fn GetCaptureDevice(conf: config::Devices) -> Box<dyn CaptureDevice> {
    match conf.capture.r#type {
        config::DeviceType::Alsa => {
            Box::new(alsadevice::AlsaCaptureDevice::open(conf.capture.device, 
                                                         conf.samplerate as u32, 
                                                         conf.buffersize as i64, 
                                                         conf.capture.channels as u32).unwrap())
        }
    } 
}

pub fn GetPlaybackDevice(conf: config::Devices) -> Box<dyn PlaybackDevice> {
    match conf.playback.r#type {
        config::DeviceType::Alsa => {
            Box::new(alsadevice::AlsaPlaybackDevice::open(conf.playback.device, 
                                                         conf.samplerate as u32, 
                                                         conf.buffersize as i64, 
                                                         conf.playback.channels as u32).unwrap())
        }
    } 
}

