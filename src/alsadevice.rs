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

use PrcFmt;
use StatusMessage;
use Res;


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

/// Convert an AudioChunk to an interleaved buffer of ints.
fn chunk_to_buffer<T: num_traits::cast::NumCast>(chunk: AudioChunk, buf: &mut [T], scalefactor: PrcFmt) -> () {
    let _num_samples = chunk.channels*chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value: T;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = (scalefactor - 1.0)/scalefactor;
    let minval = -1.0;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = chunk.waveforms[chan][frame];
            if float_val > maxval {
                clipped += 1;
                if float_val > peak {
                    peak = float_val;
                }
                float_val = maxval;
            }
            else if float_val < minval {
                clipped += 1;
                if -float_val > peak {
                    peak = -float_val;
                }
                float_val = minval;
            }
            value = match num_traits::cast(float_val*scalefactor) {
                Some(val) => val,
                None => {
                    println!("bad {}", float_val);
                    num_traits::cast(0.0).unwrap()
                }
            };
            buf[idx] = value;
            idx += 1;
        }
    }
    if clipped > 0 {
        println!("Clipping detected, {} samples clipped, peak {}%", clipped, peak*100.0);
    }
    //buf
}

/// Convert a buffer of interleaved ints to an AudioChunk.
fn buffer_to_chunk<T: num_traits::cast::AsPrimitive<PrcFmt>>(buffer: &[T], channels: usize, scalefactor: PrcFmt) -> AudioChunk {
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


/// Open an Alsa PCM device
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
        //println!("Opened audio output {:?} with parameters: {:?}, {:?}", devname, hwp, swp);
        (hwp.get_rate()?, act_bufsize) 
    };
    Ok(pcmdev)
}

/// Start a playback thread listening for AudioMessages via a channel. 
impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(&mut self, channel: mpsc::Receiver<AudioMessage>, barrier: Arc<Barrier>, status_channel: mpsc::Sender<StatusMessage>) -> Res<Box<thread::JoinHandle<()>>> {
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
            let delay = time::Duration::from_millis((4*1000*bufferlength/samplerate) as u64);
            match open_pcm(devname, samplerate as u32, bufferlength as i64, channels as u32, bits, false) {
                Ok(pcmdevice) => {
                    match status_channel.send(StatusMessage::PlaybackReady) {
                        Ok(()) => {},
                        Err(_err) => {},
                    }
                    //let scalefactor = (1<<bits-1) as PrcFmt;
                    let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
                    barrier.wait();
                    thread::sleep(delay);
                    println!("starting playback loop");
                    match format {
                        SampleFormat::S16LE => {
                            let io = pcmdevice.io_i16().unwrap();
                            let mut buffer = vec![0i16; bufferlength*channels];
                            loop {
                                match channel.recv() {
                                    Ok(AudioMessage::Audio(chunk)) => {
                                        //let before = Instant::now();
                                        chunk_to_buffer(chunk, &mut buffer, scalefactor);
                                        //let after = before.elapsed();
                                        //println!("chunk to buffer {} ns", after.as_nanos());
                                        let playback_state = pcmdevice.state();
                                        //println!("playback state {:?}", playback_state);
                                        if playback_state == State::XRun {
                                            println!("Prepare playback");
                                            pcmdevice.prepare().unwrap();
                                        }
                                        let _frames = match io.writei(&buffer[..]) {
                                            Ok(frames) => frames,
                                            Err(_err) => {
                                                println!("retrying playback");
                                                pcmdevice.prepare().unwrap();
                                                io.writei(&buffer[..]).unwrap()
                                            },
                                        };
                                    }
                                    _ => {}
                                }
                            }
                        },
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let io = pcmdevice.io_i32().unwrap();
                            let mut buffer = vec![0i32; bufferlength*channels];
                            loop {
                                match channel.recv() {
                                    Ok(AudioMessage::Audio(chunk)) => {
                                        //println!("p");
                                        //let before = Instant::now();
                                        chunk_to_buffer(chunk, &mut buffer, scalefactor);
                                        //let after = before.elapsed();
                                        //println!("chunk to buffer {} ns", after.as_nanos());
                                        //let before = Instant::now();
                                        let playback_state = pcmdevice.state();
                                        //println!("playback state {:?}", playback_state);
                                        if playback_state == State::XRun {
                                            println!("Prepare playback");
                                            pcmdevice.prepare().unwrap();
                                        }
                                        //let middle = before.elapsed();
                                        let _frames = match io.writei(&buffer[..]) {
                                            Ok(frames) => frames,
                                            Err(_err) => {
                                                println!("retrying playback");
                                                pcmdevice.prepare().unwrap();
                                                io.writei(&buffer[..]).unwrap()
                                            },
                                        };
                                        //let after = before.elapsed();
                                        //println!("check {} ns, write {} ns", middle.as_nanos(), after.as_nanos());
                                    }
                                    _ => {}
                                }
                            }
                        },
                    };
                },
                Err(err) => {
                    status_channel.send(StatusMessage::PlaybackError{ message: format!("{}", err)}).unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}


/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
    fn start(&mut self, channel: mpsc::Sender<AudioMessage>, barrier: Arc<Barrier>, status_channel: mpsc::Sender<StatusMessage>) -> Res<Box<thread::JoinHandle<()>>> {
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
            match open_pcm(devname, samplerate as u32, bufferlength as i64, channels as u32, bits, true) {
                Ok(pcmdevice) => {
                    match status_channel.send(StatusMessage::CaptureReady) {
                        Ok(()) => {},
                        Err(_err) => {},
                    }
                    let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
                    barrier.wait();
                    println!("starting captureloop");
                    match format {
                        SampleFormat::S16LE => {
                            let io = pcmdevice.io_i16().unwrap();
                            let mut buf = vec![0i16; channels*bufferlength];
                            loop {
                                //let mut buf: Vec<i16>;
                                //let mut buf: Vec<i16> = Vec::with_capacity(channels*bufferlength);
                                let capture_state = pcmdevice.state();
                                if capture_state == State::XRun {
                                    println!("prepare capture");
                                    pcmdevice.prepare().unwrap();
                                }
                                //let frames = self.io.readi(&mut buf)?;
                                let _frames = match io.readi(&mut buf) {
                                    Ok(frames) => frames,
                                    Err(_err) => {
                                        println!("retrying capture");
                                        pcmdevice.prepare().unwrap();
                                        io.readi(&mut buf).unwrap()
                                    },
                                };
                                //let before = Instant::now();
                                let chunk = buffer_to_chunk(&buf, channels, scalefactor);
                                //let after = before.elapsed();
                                //println!("buffer to chunk {} ns", after.as_nanos());
                                let msg = AudioMessage::Audio(chunk);
                                channel.send(msg).unwrap();
                            }
                        },
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let io = pcmdevice.io_i32().unwrap();
                            let mut buf = vec![0i32; channels*bufferlength];
                            loop {
                                //let mut buf: Vec<i32>;
                                let capture_state = pcmdevice.state();
                                if capture_state == State::XRun {
                                    pcmdevice.prepare().unwrap();
                                }
                                //let frames = self.io.readi(&mut buf)?;
                                let _frames = io.readi(&mut buf).unwrap();
                                let chunk = buffer_to_chunk(&buf, channels, scalefactor);
                                let msg = AudioMessage::Audio(chunk);
                                channel.send(msg).unwrap();
                            }
                        },
                    };
                },
                Err(err) => {
                    status_channel.send(StatusMessage::CaptureError{ message: format!("{}", err)}).unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}

