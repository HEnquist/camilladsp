extern crate alsa;
extern crate num_traits;
//use std::{iter, error};
use alsa::{Direction, ValueOr};
use alsa::pcm::{HwParams, Format, Access, State};
use std::{thread, time};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
//mod audiodevice;
use audiodevice::*;
// Sample format
use config::SampleFormat;

type PrcFmt = f64;


pub struct AlsaPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

pub struct AlsaCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
}


fn chunk_to_buffer<T: num_traits::cast::NumCast>(chunk: AudioChunk, scalefactor: PrcFmt) -> Vec<T> {
    let num_samples = chunk.channels*chunk.frames;
    let mut buf = Vec::with_capacity(num_samples);
    let mut value: T;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            value = num_traits::cast(chunk.waveforms[chan][frame] * scalefactor).unwrap();
            buf.push(value);
        }
    }
    buf
}

fn buffer_to_chunk<T: num_traits::cast::AsPrimitive<PrcFmt>>(buffer: Vec<T>, channels: usize, scalefactor: PrcFmt) -> AudioChunk {
    let num_samples = buffer.len();
    let num_frames = num_samples/channels;
    let mut value: PrcFmt;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    //let mut idx = 0;
    //let mut samples = buffer.iter();
    let mut idx = 0;
    for _frame in 0..num_frames {
        for chan in 0..channels {
            value = buffer[idx].as_();
            idx+=1;
            value = value / scalefactor;
            //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
            wfs[chan].push(value);
            //idx += 1;
        }
    }
    let chunk = AudioChunk {
        channels: channels,
        frames: num_frames,
        waveforms: wfs,
    };
    chunk
}

fn open_pcm(devname: String, samplerate: u32, bufsize: i64, channels: u32, bits: usize, capture: bool) -> Res<alsa::PCM> {
    // Open the device
    let pcmdev;
    if capture {
        pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;
    }
    else {
        pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;
    }
    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;
        hwp.set_channels(channels)?;
        hwp.set_rate(samplerate, ValueOr::Nearest)?;
        match bits {
            16 => hwp.set_format(Format::s16())?,
            24 => hwp.set_format(Format::s24())?,
            32 => hwp.set_format(Format::s32())?,
            _ => {},
        }
        
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_buffer_size(bufsize)?;
        hwp.set_period_size(bufsize / 8, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let (_rate, _act_bufsize) = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        swp.set_start_threshold(act_bufsize - act_periodsize)?;
        //swp.set_avail_min(periodsize)?;
        pcmdev.sw_params(&swp)?;
        println!("Opened audio output {:?} with parameters: {:?}, {:?}", devname, hwp, swp);
        (hwp.get_rate()?, act_bufsize) 
    };
    Ok(pcmdev)
}

impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(&mut self, channel: mpsc::Receiver<AudioMessage>, barrier: Arc<Barrier>) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate.clone();
        let bufferlength = self.bufferlength.clone();
        let channels = self.channels.clone();
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
        };
        let format = self.format.clone();
        let handle = thread::spawn(move || {
            let delay = time::Duration::from_millis((2*1000*bufferlength/samplerate) as u64);
            let pcmdevice = open_pcm(devname, samplerate as u32, bufferlength as i64, channels as u32, bits, false).unwrap();
            let scalefactor = (1<<bits-1) as PrcFmt;
            barrier.wait();
            thread::sleep(delay);
            println!("starting playback loop");
            match format {
                SampleFormat::S16LE => {
                    let io = pcmdevice.io_i16().unwrap();
                    loop {
                        match channel.recv() {
                            Ok(AudioMessage::Audio(chunk)) => {
                                let buffer = chunk_to_buffer(chunk, scalefactor);
                                let playback_state = pcmdevice.state();
                                //println!("playback state {:?}", playback_state);
                                if playback_state == State::XRun {
                                    println!("Prepare playback");
                                    pcmdevice.prepare().unwrap();
                                }
                                let _frames = io.writei(&buffer[..]).unwrap();
                            }
                            _ => {}
                        }
                    }
                },
                SampleFormat::S24LE | SampleFormat::S32LE => {
                    let io = pcmdevice.io_i32().unwrap();
                    loop {
                        match channel.recv() {
                            Ok(AudioMessage::Audio(chunk)) => {
                                let buffer = chunk_to_buffer(chunk, scalefactor);
                                let playback_state = pcmdevice.state();
                                //println!("playback state {:?}", playback_state);
                                if playback_state == State::XRun {
                                    println!("Prepare playback");
                                    pcmdevice.prepare().unwrap();
                                }
                                let _frames = io.writei(&buffer[..]).unwrap();
                            }
                            _ => {}
                        }
                    }
                },
            };
        });
        Ok(Box::new(handle))
    }
}

impl CaptureDevice for AlsaCaptureDevice {
    fn start(&mut self, channel: mpsc::Sender<AudioMessage>, barrier: Arc<Barrier>) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate.clone();
        let bufferlength = self.bufferlength.clone();
        let channels = self.channels.clone();
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
        };
        let format = self.format.clone();
        let handle = thread::spawn(move || {
            let pcmdevice = open_pcm(devname, samplerate as u32, bufferlength as i64, channels as u32, bits, true).unwrap();
            let scalefactor = (1<<bits-1) as PrcFmt;
            barrier.wait();
            println!("starting captureloop");
            match format {
                SampleFormat::S16LE => {
                    let io = pcmdevice.io_i16().unwrap();
                    loop {
                        let mut buf: Vec<i16>;
                        buf = vec![0; channels*bufferlength];
                        let capture_state = pcmdevice.state();
                        if capture_state == State::XRun {
                            pcmdevice.prepare().unwrap();
                        }
                        //let frames = self.io.readi(&mut buf)?;
                        let _frames = io.readi(&mut buf).unwrap();
                        let chunk = buffer_to_chunk(buf, channels, scalefactor);
                        let msg = AudioMessage::Audio(chunk);
                        channel.send(msg).unwrap();
                    }
                },
                SampleFormat::S24LE | SampleFormat::S32LE => {
                    let io = pcmdevice.io_i32().unwrap();
                    loop {
                        let mut buf: Vec<i32>;
                        buf = vec![0; channels*bufferlength];
                        let capture_state = pcmdevice.state();
                        if capture_state == State::XRun {
                            pcmdevice.prepare().unwrap();
                        }
                        //let frames = self.io.readi(&mut buf)?;
                        let _frames = io.readi(&mut buf).unwrap();
                        let chunk = buffer_to_chunk(buf, channels, scalefactor);
                        let msg = AudioMessage::Audio(chunk);
                        channel.send(msg).unwrap();
                    }
                },
            };
        });
        Ok(Box::new(handle))
    }
}

