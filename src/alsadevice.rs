extern crate alsa;
//use std::{iter, error};
use alsa::{Direction, ValueOr};
use alsa::pcm::{HwParams, Format, Access, State};

//mod audiodevice;
use audiodevice::*;
// Sample format

type PrcFmt = f64;


pub struct AlsaPlaybackDeviceS16LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: Option<alsa::pcm::IO<'a, SmpFmt>>,
    buffer: Vec<i16>,
    bufferlength: usize,
    channels: usize,
}

pub struct AlsaPlaybackDeviceS24LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: Option<alsa::pcm::IO<'a, SmpFmt>>,
    buffer: Vec<i32>,
    bufferlength: usize,
    channels: usize,
}

pub struct AlsaPlaybackDeviceS32LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: Option<alsa::pcm::IO<'a, SmpFmt>>,
    buffer: Vec<i32>,
    bufferlength: usize,
    channels: usize,
}
pub struct AlsaCaptureDeviceS16LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: alsa::pcm::IO<'a, T>,
    buffer: Vec<i16>,
    bufferlength: usize,
    channels: usize,
}
pub struct AlsaCaptureDeviceS24LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: alsa::pcm::IO<'a, T>,
    buffer: Vec<i32>,
    bufferlength: usize,
    channels: usize,
}
pub struct AlsaCaptureDeviceS32LE {
    devname: String,
    samplerate: usize,
    pcmdevice: alsa::PCM,
    //io: alsa::pcm::IO<'a, T>,
    buffer: Vec<i32>,
    bufferlength: usize,
    channels: usize,
}


impl PlaybackDevice for AlsaPlaybackDeviceS16LE {
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
        for frame in 0..chunk.frames {
            for chan in 0..chunk.channels {
                value = (chunk.waveforms[chan][frame] * (1<<15) as PrcFmt) as i16;
                buf.push(value);
            }
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
        //if self.io.is_none() {
        //    self.io = Some(self.pcmdevice.io_i16()?);
        //}
        let frames = self.pcmdevice.io_i16()?.writei(&self.buffer[..])?;
        //let frames = self.io.unwrap().writei(&self.buffer[..])?;
        Ok(frames as usize)
    }
}

impl PlaybackDevice for AlsaPlaybackDeviceS24LE {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn put_chunk(&mut self, chunk: AudioChunk) -> Res<()> {
        //let buf = chunk.into_iter().collect::<Vec<SF>>();
        //buf
        let num_samples = chunk.channels*chunk.frames;
        let mut buf = Vec::with_capacity(num_samples);
        let mut value: i32;
        for frame in 0..chunk.frames {
            for chan in 0..chunk.channels {
                value = (chunk.waveforms[chan][frame] * (1<<23) as PrcFmt) as i32;
                buf.push(value);
            }
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
        //if self.io.is_none() {
        //    self.io = Some(self.pcmdevice.io_i16()?);
        //}
        let frames = self.pcmdevice.io_i32()?.writei(&self.buffer[..])?;
        //let frames = self.io.unwrap().writei(&self.buffer[..])?;
        Ok(frames as usize)
    }
}

impl PlaybackDevice for AlsaPlaybackDeviceS32LE {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn put_chunk(&mut self, chunk: AudioChunk) -> Res<()> {
        //let buf = chunk.into_iter().collect::<Vec<SF>>();
        //buf
        let num_samples = chunk.channels*chunk.frames;
        let mut buf = Vec::with_capacity(num_samples);
        let mut value: i32;
        for frame in 0..chunk.frames {
            for chan in 0..chunk.channels {
                // TODO
                // check -1 .. 1
                // warn if clipping ("Warning, 14 samples clipped, peak 107%")
                // move 1<<31 factor before loop
                value = (chunk.waveforms[chan][frame] * (1<<31) as PrcFmt) as i32;
                buf.push(value);
            }
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
        //if self.io.is_none() {
        //    self.io = Some(self.pcmdevice.io_i16()?);
        //}
        let frames = self.pcmdevice.io_i32()?.writei(&self.buffer[..])?;
        //let frames = self.io.unwrap().writei(&self.buffer[..])?;
        Ok(frames as usize)
    }
}

impl CaptureDevice for AlsaCaptureDeviceS16LE {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn fetch_chunk(&mut self) -> Res<AudioChunk> {
        let num_samples = self.buffer.len();
        let num_frames = num_samples/self.channels;
        let mut value: PrcFmt;
        let mut wfs = Vec::with_capacity(self.channels);
        for _chan in 0..self.channels {
            wfs.push(Vec::with_capacity(num_frames));
        }
        //let mut idx = 0;
        let mut samples = self.buffer.iter();
        for _frame in 0..num_frames {
            for chan in 0..self.channels {
                value = (*samples.next().unwrap() as PrcFmt) / ((1<<15) as PrcFmt);
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
        }
        let chunk = AudioChunk {
            channels: self.channels,
            frames: num_frames,
            waveforms: wfs,
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

impl CaptureDevice for AlsaCaptureDeviceS24LE {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn fetch_chunk(&mut self) -> Res<AudioChunk> {
        let num_samples = self.buffer.len();
        let num_frames = num_samples/self.channels;
        let mut value: PrcFmt;
        let mut wfs = Vec::with_capacity(self.channels);
        for _chan in 0..self.channels {
            wfs.push(Vec::with_capacity(num_frames));
        }
        //let mut idx = 0;
        let mut samples = self.buffer.iter();
        for _frame in 0..num_frames {
            for chan in 0..self.channels {
                value = (*samples.next().unwrap() as PrcFmt) / ((1<<23) as PrcFmt);
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
        }
        let chunk = AudioChunk {
            channels: self.channels,
            frames: num_frames,
            waveforms: wfs,
        };
        Ok(chunk)
    }
    
    //capure to internal buffer
    fn capture(&mut self) -> Res<usize> {
        let mut buf: Vec<i32>;
        buf = vec![0; self.channels*self.bufferlength];
        let capture_state = self.pcmdevice.state();
        if capture_state == State::XRun {
            self.pcmdevice.prepare()?;
        }
        //let frames = self.io.readi(&mut buf)?;
        let frames = self.pcmdevice.io_i32()?.readi(&mut buf)?;
        self.buffer = buf;
        Ok(frames as usize)
    }
}

impl CaptureDevice for AlsaCaptureDeviceS32LE {
    fn get_bufsize(&mut self) -> usize {
        self.bufferlength
    }

    /// Send audio chunk for later playback
    fn fetch_chunk(&mut self) -> Res<AudioChunk> {
        let num_samples = self.buffer.len();
        let num_frames = num_samples/self.channels;
        let mut value: PrcFmt;
        let mut wfs = Vec::with_capacity(self.channels);
        for _chan in 0..self.channels {
            wfs.push(Vec::with_capacity(num_frames));
        }
        //let mut idx = 0;
        let mut samples = self.buffer.iter();
        for _frame in 0..num_frames {
            for chan in 0..self.channels {
                value = (*samples.next().unwrap() as PrcFmt) / ((1<<31) as PrcFmt);
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wfs[chan].push(value);
                //idx += 1;
            }
        }
        let chunk = AudioChunk {
            channels: self.channels,
            frames: num_frames,
            waveforms: wfs,
        };
        Ok(chunk)
    }
    
    //capure to internal buffer
    fn capture(&mut self) -> Res<usize> {
        let mut buf: Vec<i32>;
        buf = vec![0; self.channels*self.bufferlength];
        let capture_state = self.pcmdevice.state();
        if capture_state == State::XRun {
            self.pcmdevice.prepare()?;
        }
        //let frames = self.io.readi(&mut buf)?;
        let frames = self.pcmdevice.io_i32()?.readi(&mut buf)?;
        self.buffer = buf;
        Ok(frames as usize)
    }
}

impl AlsaPlaybackDeviceS16LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaPlaybackDeviceS16LE> {
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
        let mut device = AlsaPlaybackDeviceS16LE {
            devname: devname,
            samplerate: rate as usize,
            pcmdevice: pcmdev,
            //io: None,
            buffer: vec![0, 0],
            bufferlength: act_bufsize as usize,
            channels: channels as usize,
        };
        //let mut io = device.pcmdevice.io_i16()?;
        //device.io = Some(io);
        Ok(device)
    }
}

impl AlsaPlaybackDeviceS24LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaPlaybackDeviceS24LE> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;
        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s24())?;
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
        let mut device = AlsaPlaybackDeviceS24LE {
            devname: devname,
            samplerate: rate as usize,
            pcmdevice: pcmdev,
            //io: None,
            buffer: vec![0, 0],
            bufferlength: act_bufsize as usize,
            channels: channels as usize,
        };
        //let mut io = device.pcmdevice.io_i16()?;
        //device.io = Some(io);
        Ok(device)
    }
}


impl AlsaPlaybackDeviceS32LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaPlaybackDeviceS32LE> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;
        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s32())?;
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
        let mut device = AlsaPlaybackDeviceS32LE {
            devname: devname,
            samplerate: rate as usize,
            pcmdevice: pcmdev,
            //io: None,
            buffer: vec![0, 0],
            bufferlength: act_bufsize as usize,
            channels: channels as usize,
        };
        //let mut io = device.pcmdevice.io_i16()?;
        //device.io = Some(io);
        Ok(device)
    }
}
impl AlsaCaptureDeviceS16LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaCaptureDeviceS16LE> {
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
        let device = AlsaCaptureDeviceS16LE {
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


impl AlsaCaptureDeviceS24LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaCaptureDeviceS24LE> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;

        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s24())?;
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
        let device = AlsaCaptureDeviceS24LE {
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

impl AlsaCaptureDeviceS32LE {
    pub fn open(devname: String, samplerate: u32, bufsize: i64, channels: u32) -> Res<AlsaCaptureDeviceS32LE> {
        // Open the device
        let pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;

        // Set hardware parameters
        {
            let hwp = HwParams::any(&pcmdev)?;
            hwp.set_channels(channels)?;
            hwp.set_rate(samplerate, ValueOr::Nearest)?;
            hwp.set_format(Format::s32())?;
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
        let device = AlsaCaptureDeviceS32LE {
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
