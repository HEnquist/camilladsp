extern crate alsa;
use std::{iter, error};
use alsa::{Direction, ValueOr};
use alsa::pcm::{PCM, HwParams, Format, Access, State};

//mod audiodevice;
use audiodevice::*;

pub struct AlsaPlaybackDevice<T> {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: alsa::pcm::IO<'a, T>,
    buffer: Vec<T>,
    bufferlength: usize,
    channels: usize,
}
pub struct AlsaCaptureDevice<T> {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: alsa::pcm::IO<'a, T>,
    buffer: Vec<T>,
    bufferlength: usize,
    channels: usize,
}


impl PlaybackDevice<i16> for AlsaPlaybackDevice<i16> {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn put_chunk(&mut self, chunk: AudioChunk) -> Res<()> {
        //let buf = chunk.into_iter().collect::<Vec<SF>>();
        //buf
        let num_samples = chunk.channels*chunk.frames;
        let mut buf = Vec::with_capacity(num_samples);
        let mut value: i16;
        match chunk.waveforms {
            Waveforms::Float32(waveforms) => {
                 for frame in 0..chunk.frames {
                    for chan in 0..chunk.channels {
                        value = (waveforms[chan][frame] * (1<<15) as f32) as i16;
                        buf.push(value);
                    }
                }
            },
            Waveforms::Float64(waveforms) => {
                 for frame in 0..chunk.frames {
                    for chan in 0..chunk.channels {
                        value = (waveforms[chan][frame] * (1<<15) as f64) as i16;
                        buf.push(value);
                    }
                }
            },
        }
        self.buffer = buf;
        Ok(())
    }
    
    // play the buffer
    fn play(&mut self) -> Res<usize> {
        let playback_state = self.pcmdevice.state();
        //println!("playback state {:?}", playback_state);
        if playback_state == State::XRun {
            println!("Prepare playback");
            self.pcmdevice.prepare()?;
        }
        //let frames = self.io.writei(&self.buffer[..])?;
        let frames = self.pcmdevice.io_i16()?.writei(&self.buffer[..])?;
        Ok(frames as usize)
    }
}


impl CaptureDevice<i16> for AlsaCaptureDevice<i16> {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn fetch_chunk(&mut self, datatype: Datatype) -> Res<AudioChunk> {
        let num_samples = self.buffer.len();
        let num_frames = num_samples/self.channels;
        let waveforms = match datatype {
            Datatype::Float32 => {
                let mut value: f32;
                let mut wfs = Vec::with_capacity(self.channels);
                for chan in 0..self.channels {
                    wfs.push(Vec::with_capacity(num_frames));
                }
        
                let mut samples = self.buffer.iter();
                for frame in 0..num_frames {
                    for chan in 0..self.channels {
                        value = (*samples.next().unwrap() as f32) / ((1<<15) as f32);
                        wfs[chan].push(value);
                    }
                }
                Waveforms::Float32(wfs)
            },
            Datatype::Float64 => {
                let mut value: f64;
                let mut wfs = Vec::with_capacity(self.channels);
                for chan in 0..self.channels {
                    wfs.push(Vec::with_capacity(num_frames));
                }
        
                let mut samples = self.buffer.iter();
                for frame in 0..num_frames {
                    for chan in 0..self.channels {
                        value = (*samples.next().unwrap() as f64) / ((1<<15) as f64);
                        wfs[chan].push(value);
                    }
                }
                Waveforms::Float64(wfs)
            },
        };
        let chunk = AudioChunk {
            channels: self.channels,
            frames: num_frames,
            waveforms: waveforms,
        };
        Ok(chunk)
    }
    
    //capure to internal buffer
    fn capture(&mut self) -> Res<usize> {
        let mut buf: Vec<i16>;
        buf = vec![0; self.channels*self.bufferlength];
        let capture_state = self.pcmdevice.state();
        if capture_state == State::XRun {
            self.pcmdevice.prepare()?;
        }
        //let frames = self.io.readi(&mut buf)?;
        let frames = self.pcmdevice.io_i16()?.readi(&mut buf)?;
        self.buffer = buf;
        Ok(frames as usize)
    }
}


impl AlsaPlaybackDevice<i16> {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaPlaybackDevice<i16>> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;

        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s16())?;
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_buffer_size(bufsize)?;
            hwp.set_period_size(bufsize / 4, alsa::ValueOr::Nearest)?;
            pcmdev.hw_params(&hwp)?;
        }

        // Set software parameters
        let (rate, act_bufsize) = {
            let hwp = pcmdev.hw_params_current()?;
            let swp = pcmdev.sw_params_current()?;
            let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
            swp.set_start_threshold(act_bufsize - act_periodsize)?;
            //swp.set_avail_min(periodsize)?;
            pcmdev.sw_params(&swp)?;
            println!("Opened audio output {:?} with parameters: {:?}, {:?}", devname, hwp, swp);
            (hwp.get_rate()?, act_bufsize) 
        };

        //let mut io = pcmdev.io_i16()?;
        let device = AlsaPlaybackDevice {
            devname: devname,
            samplerate: rate as usize,
            pcmdevice: pcmdev,
            //io: io,
            buffer: vec![0, 0],
            bufferlength: act_bufsize as usize,
            channels: channels as usize,
        };
        Ok(device)
    }
}


impl AlsaCaptureDevice<i16> {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaCaptureDevice<i16>> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;

        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s16())?;
            hwp.set_access(Access::RWInterleaved)?;
            hwp.set_buffer_size(bufsize)?;
            hwp.set_period_size(bufsize / 4, alsa::ValueOr::Nearest)?;
            pcmdev.hw_params(&hwp)?;
        }

        // Set software parameters
        let (rate, act_bufsize) = {
            let hwp = pcmdev.hw_params_current()?;
            let swp = pcmdev.sw_params_current()?;
            let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
            swp.set_start_threshold(act_bufsize - act_periodsize)?;
            //swp.set_avail_min(periodsize)?;
            pcmdev.sw_params(&swp)?;
            println!("Opened audio output {:?} with parameters: {:?}, {:?}", devname, hwp, swp);
            (hwp.get_rate()?, act_bufsize) 
        };

        //let mut io = pcmdev.io_i16()?;
        let device = AlsaCaptureDevice {
            devname: devname,
            samplerate: rate as usize,
            pcmdevice: pcmdev,
            //io: io,
            buffer: vec![0, 0],
            bufferlength: act_bufsize as usize,
            channels: channels as usize,
        };
        Ok(device)
    }
}


