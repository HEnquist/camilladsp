
extern crate num_traits;
//use std::{iter, error};
use pulse;
use std::convert::TryInto;

use psimple::Simple;
use pulse::stream::Direction;
use pulse::sample;

use std::thread;
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
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
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
        eprintln!("Clipping detected, {} samples clipped, peak {}%", clipped, peak*100.0);
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
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
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
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
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
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
        }
    }
    let chunk = AudioChunk {
        channels: channels,
        frames: num_frames,
        maxval: maxvalue,
        minval: minvalue,
        waveforms: wfs,
    };
    chunk
}


/// Open a PulseAudio device
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
        prebuf: bytes as u32,
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
            //let delay = time::Duration::from_millis((4*1000*bufferlength/samplerate) as u64);
            match open_pulse(devname, samplerate as u32, bufferlength as i64, channels as u8, bits, false) {
                Ok(pulsedevice) => {
                    match status_channel.send(StatusMessage::PlaybackReady) {
                        Ok(()) => {},
                        Err(_err) => {},
                    }
                    //let scalefactor = (1<<bits-1) as PrcFmt;
                    let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
                    barrier.wait();
                    //thread::sleep(delay);
                    eprintln!("starting playback loop");
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
                                    Ok(AudioMessage::EndOfStream) => {
                                        status_channel.send(StatusMessage::PlaybackDone).unwrap();
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
                                    Ok(AudioMessage::EndOfStream) => {
                                        status_channel.send(StatusMessage::PlaybackDone).unwrap();
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
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold/20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / bufferlength) as PrcFmt)) as usize;
        let handle = thread::spawn(move || {
            match open_pulse(devname, samplerate as u32, bufferlength as i64, channels as u8, bits, true) {
                Ok(pulsedevice) => {
                    match status_channel.send(StatusMessage::CaptureReady) {
                        Ok(()) => {},
                        Err(_err) => {},
                    }
                    let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
                    let mut silent_nbr: usize = 0;  
                    barrier.wait();
                    eprintln!("starting captureloop");
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
                                if (chunk.maxval - chunk.minval) > silence {
                                    if silent_nbr > silent_limit {
                                        eprintln!("Resuming processing");
                                    }
                                    silent_nbr = 0;
                                }
                                else if silent_limit > 0 {
                                    if silent_nbr == silent_limit {
                                        eprintln!("Pausing processing");
                                    }
                                    silent_nbr += 1;
                                }
                                if silent_nbr <= silent_limit {
                                    let msg = AudioMessage::Audio(chunk);
                                    channel.send(msg).unwrap();
                                }
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
                                if (chunk.maxval - chunk.minval) > silence {
                                    if silent_nbr > silent_limit {
                                        eprintln!("Resuming processing");
                                    }
                                    silent_nbr = 0;
                                }
                                else if silent_limit > 0 {
                                    if silent_nbr == silent_limit {
                                        eprintln!("Pausing processing");
                                    }
                                    silent_nbr += 1;
                                }
                                if silent_nbr <= silent_limit {
                                    let msg = AudioMessage::Audio(chunk);
                                    channel.send(msg).unwrap();
                                }
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


#[cfg(test)]
mod tests {
    use pulsedevice::{chunk_to_buffer, buffer_to_chunk};
    use audiodevice::AudioChunk;
    use crate::PrcFmt;

    #[test]
    fn to_from_buffer_16() {
        let bits = 16;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: waveforms.clone(),
        };
        let mut buffer = vec![0u8; 3*2];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24() {
        let bits = 24;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: waveforms.clone(),
        };
        let mut buffer = vec![0u8; 3*4];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_32() {
        let bits = 32;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: waveforms.clone(),
        };
        let mut buffer = vec![0u8; 3*4];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_16() {
        let bits = 16;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-1.0, 0.0, 32767.0/32768.0]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: vec![vec![-2.0, 0.0, 2.0]; 1],
        };
        let mut buffer = vec![0u8; 3*2];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_24() {
        let bits = 24;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-1.0, 0.0, 8388607.0/8388608.0]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: vec![vec![-2.0, 0.0, 2.0]; 1],
        };
        let mut buffer = vec![0u8; 3*4];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_32() {
        let bits = 32;
        let scalefactor = (2.0 as PrcFmt).powf((bits-1) as PrcFmt);
        let waveforms = vec![vec![-1.0, 0.0, 2147483647.0/2147483648.0]; 1];
        let chunk = AudioChunk {
            frames: 3,
            channels: 1,
            maxval: 0.0,
            minval: 0.0,
            waveforms: vec![vec![-2.0, 0.0, 2.0]; 1],
        };
        let mut buffer = vec![0u8; 3*4];
        chunk_to_buffer(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk(&buffer, 1, scalefactor, bits);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }
}
