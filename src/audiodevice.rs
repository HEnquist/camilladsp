// Traits for audio devices
use std::error;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

type SampleFormat = i16;
type ProcessingFormat = f64;

pub type pcm16 = i16;
pub type pcm24 = i32;
pub type pcm32 = i32;


pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub waveforms: Vec<Vec<ProcessingFormat>>,
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

