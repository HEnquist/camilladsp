
extern crate num_traits;
//use std::{iter, error};
use pulse;
use std::convert::TryInto;

use psimple::Simple;
use pulse::stream::Direction;
use pulse::sample;

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


pub struct PulsePlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

pub struct PulseCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

/// Convert an AudioChunk to an interleaved buffer of ints.
fn chunk_to_buffer(chunk: AudioChunk, buf: &mut [u8], scalefactor: PrcFmt, bits: usize) -> () {
    let _num_samples = chunk.channels*chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value16;
    let mut value32;
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
            if bits == 16 {
                value16 = (float_val*scalefactor) as i16;
                let bytes = value16.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            }
            else {
                value32 = (float_val*scalefactor) as i32;
                let bytes = value32.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            }
        }
    }
    if clipped > 0 {
        println!("Clipping detected, {} samples clipped, peak {}%", clipped, peak*100.0);
    }
    //buf
}

/// Convert a buffer of interleaved ints to an AudioChunk.
fn buffer_to_chunk(buffer: &[u8], channels: usize, scalefactor: PrcFmt, bits: usize) -> AudioChunk {
    let num_samples = match bits {
        16 => buffer.len()/2,
        24 | 32 => buffer.len()/4,
        _ => 0,
    };
    let num_frames = num_samples/channels;
    let mut value: PrcFmt;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    let mut idx = 0;
    if bits == 16 {
        for _frame in 0..num_frames {
            for chan in 0..channels {
                value = i16::from_le_bytes(buffer[idx..idx+2].try_into().unwrap()) as PrcFmt;
                idx+=2;
                value = value / scalefactor;
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
        }
    }
    else {
        for _frame in 0..num_frames {
            for chan in 0..channels {
                value = i32::from_le_bytes(buffer[idx..idx+4].try_into().unwrap()) as PrcFmt;
                idx+=4;
                value = value / scalefactor;
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
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
fn open_pulse(devname: String, samplerate: u32, bufsize: i64, channels: u8, bits: usize, capture: bool) -> Res<Simple> {
    // Open the device
    let dir = match capture {
        true => Direction::Record,
        false => Direction::Playback,
    };
    
    let format = match bits {
        16 => sample::SAMPLE_S16NE,
        24 => sample::SAMPLE_S24_32NE,
        32 => sample::SAMPLE_S32NE,
        _ => panic!("invalid bits"),
    };

    let bytes = match bits {
        16 => bufsize*(channels as i64)*2,
        24 => bufsize*(channels as i64)*4,
        32 => bufsize*(channels as i64)*4,
        _ => panic!("invalid bits"),
    };

    let spec = sample::Spec {
        format: format,
        channels: channels,
        rate: samplerate,
    };
    //assert!(spec.is_valid());
    let attr = pulse::def::BufferAttr {
        maxlength: std::u32::MAX,
        tlength: std::u32::MAX,
        prebuf: std::u32::MAX,
        minreq: std::u32::MAX,
        fragsize: bytes as u32,
    };

    let pulsedev = Simple::new(
        None,                // Use the default server
        "FooApp",            // Our application’s name
        dir,                 // We want a playback stream
        Some(&devname),       // Use the default device
        "Music",             // Description of our stream
        &spec,               // Our sample format
        None,                // Use default channel map
        Some(&attr),                 // Use default buffering attributes
    ).unwrap();
    Ok(pulsedev)
}

/// Start a playback thread listening for AudioMessages via a channel. 
impl PlaybackDevice for PulsePlaybackDevice {
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
            match open_pulse(devname, samplerate as u32, bufferlength as i64, channels as u8, bits, false) {
                Ok(pulsedevice) => {
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
                            let mut buffer = vec![0u8; bufferlength*channels*2];
                            loop {
                                match channel.recv() {
                                    Ok(AudioMessage::Audio(chunk)) => {
                                        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
                                        // let _frames = match io.writei(&buffer[..]) {
                                        let write_res = pulsedevice.write(&buffer);
                                        match write_res {
                                            Ok(_) => {},
                                            Err(msg) => {
                                                status_channel.send(StatusMessage::PlaybackError{ message: format!("{}", msg) }).unwrap();
                                            }
                                        };
                                    }
                                    Err(_) => {}
                                }
                            }
                        },
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let mut buffer = vec![0u8; bufferlength*channels*4];
                            loop {
                                match channel.recv() {
                                    Ok(AudioMessage::Audio(chunk)) => {
                                        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
                                        // let _frames = match io.writei(&buffer[..]) {
                                        let write_res = pulsedevice.write(&buffer);
                                        match write_res {
                                            Ok(_) => {},
                                            Err(msg) => {
                                                status_channel.send(StatusMessage::PlaybackError{ message: format!("{}", msg) }).unwrap();
                                            }
                                        };    
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
impl CaptureDevice for PulseCaptureDevice {
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
            match open_pulse(devname, samplerate as u32, bufferlength as i64, channels as u8, bits, true) {
                Ok(pulsedevice) => {
                    match status_channel.send(StatusMessage::CaptureReady) {
                        Ok(()) => {},
                        Err(_err) => {},
                    }
                    let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
                    barrier.wait();
                    println!("starting captureloop");
                    match format {
                        SampleFormat::S16LE => {
                            let mut buf = vec![0u8; channels*bufferlength*2];
                            loop {
                                //let frames = self.io.readi(&mut buf)?;
                                let read_res = pulsedevice.read(&mut buf);
                                match read_res {
                                    Ok(_) => {},
                                    Err(msg) => {
                                        status_channel.send(StatusMessage::CaptureError{ message: format!("{}", msg) }).unwrap();
                                    }
                                };
                                //let before = Instant::now();
                                let chunk = buffer_to_chunk(&buf, channels, scalefactor, bits);
                                //let after = before.elapsed();
                                //println!("buffer to chunk {} ns", after.as_nanos());
                                let msg = AudioMessage::Audio(chunk);
                                channel.send(msg).unwrap();
                            }
                        },
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let mut buf = vec![0u8; channels*bufferlength*4];
                            loop {
                                let read_res = pulsedevice.read(&mut buf);
                                match read_res {
                                    Ok(_) => {},
                                    Err(msg) => {
                                        status_channel.send(StatusMessage::CaptureError{ message: format!("{}", msg) }).unwrap();
                                    }
                                };
                                let chunk = buffer_to_chunk(&buf, channels, scalefactor, bits);
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

