// Traits for audio devices
use std::error;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

pub type pcm16 = i16;
pub type pcm24 = i32;
pub type pcm32 = i32;


pub enum Waveforms {
    Float32(Vec<Vec<f32>>),
    Float64(Vec<Vec<f64>>),
}

pub struct AudioChunk {
    frames: usize,
    channels: usize,
    waveforms: Waveforms,
}




pub trait PlaybackDevice<T> {
    fn get_bufsize(&mut self) -> usize;

    /// Send audio chunk for later playback
    fn put_chunk(&mut self, chunk: AudioChunk) -> Res<()>;

    // Filter a Vec
    fn play(&mut self) -> Res<usize>;
}

pub trait CaptureDevice<T> {
    fn get_bufsize(&mut self) -> usize;

    /// Filter a single point
    fn fetch_chunk(&mut self, datatype: Waveforms) -> AudioChunk;

    // Filter a Vec
    fn capture(&mut self) -> Res<usize>;
}

