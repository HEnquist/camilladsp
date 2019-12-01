extern crate alsa;
use std::{iter, error};
use alsa::{Direction, ValueOr};
use alsa::pcm::{PCM, HwParams, Format, Access, State};
use alsa::direct::pcm::MmapPlayback;
use std::{thread, time};
use std::sync::mpsc;

type Res<T> = Result<T, Box<dyn error::Error>>;

mod filters;
mod biquad;
use biquad::*;

mod audiodevice;
mod alsadevice;
use audiodevice::*;
use alsadevice::*;

//pub use crate::filters::*;
//pub use crate::biquad::*;

fn open_audio_dev_play(req_devname: String, req_samplerate: u32, req_bufsize: i64) -> Res<(alsa::PCM, u32)> {

    // Open the device
    let pcmdev = alsa::PCM::new(&req_devname, Direction::Playback, false)?;

    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;
        hwp.set_channels(2)?;
        hwp.set_rate(req_samplerate, ValueOr::Nearest)?;
        hwp.set_format(Format::s16())?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_buffer_size(req_bufsize)?;
        hwp.set_period_size(req_bufsize / 4, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let rate = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (bufsize, periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        swp.set_start_threshold(bufsize - periodsize)?;
        //swp.set_avail_min(periodsize)?;
        pcmdev.sw_params(&swp)?;
        println!("Opened audio output {:?} with parameters: {:?}, {:?}", req_devname, hwp, swp);
        hwp.get_rate()?
    };

    Ok((pcmdev, rate))
}

fn open_audio_dev_capt(req_devname: String, req_samplerate: u32, req_bufsize: i64) -> Res<(alsa::PCM, u32)> {

    // Open the device
    let pcmdev = alsa::PCM::new(&req_devname, Direction::Capture, false)?;

    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;
        hwp.set_channels(2)?;
        hwp.set_rate(req_samplerate, ValueOr::Nearest)?;
        hwp.set_format(Format::s16())?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_buffer_size(req_bufsize)?;
        hwp.set_period_size(req_bufsize / 4, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let rate = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (bufsize, periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        swp.set_start_threshold(bufsize - periodsize)?;
        //swp.set_avail_min(periodsize)?;
        pcmdev.sw_params(&swp)?;
        println!("Opened audio input {:?} with parameters: {:?}, {:?}", req_devname, hwp, swp);
        hwp.get_rate()?
    };

    Ok((pcmdev, rate))
}

// Sample format
type SF = i16;
type PF = f64;


struct AudioChunk {
    frames: usize,
    channels: usize,
    waveforms: Vec<Vec<PF>>, //Waveform>,
}


impl AudioChunk {
    fn to_interleaved(self) -> Vec<SF> {
        //let buf = chunk.into_iter().collect::<Vec<SF>>();
        //buf
        let num_samples = self.channels*self.frames;
        let mut buf = Vec::with_capacity(num_samples);

        for frame in 0..self.frames {
            for chan in 0..self.channels {
                buf.push(self.waveforms[chan][frame]);
            }

        }
        buf
    }

        fn from_interleaved(buffer: Vec<SF>, num_channels: usize) -> AudioChunk {
        //let buf = chunk.into_iter().collect::<Vec<SF>>();
        //buf
        let num_samples = buffer.len();
        let num_frames = num_samples/num_channels;
        
        let mut waveforms = Vec::with_capacity(num_channels);
        for chan in 0..num_channels {
            waveforms.push(Vec::with_capacity(num_frames));
        }
        
        let mut samples = buffer.iter();
        for frame in 0..num_frames {
            for chan in 0..num_channels {
                waveforms[chan].push(*samples.next().unwrap());
            }

        }
        AudioChunk {
            channels: num_channels,
            frames: num_frames,
            waveforms: waveforms,
        }
    }
}

enum Message {
    Quit,
    Audio(AudioChunk),
}

fn play_from_buffer(pcmdev: &alsa::PCM, io: &mut alsa::pcm::IO<SF>, buf: Vec<SF>) -> Res<usize> {
    //let buf = chunk.into_iter().collect::<Vec<SF>>();
    //let buf = chunk.to_interleaved();
    let playback_state = pcmdev.state();
    //println!("playback state {:?}", playback_state);
    if playback_state == State::XRun {
        println!("Prepare playback");
        pcmdev.prepare().unwrap();
    }
    let frames = io.writei(&buf[..])?;
    Ok(frames)
}

fn capture_to_buffer(pcmdev: &alsa::PCM, io: &mut alsa::pcm::IO<SF>, channels: usize, frames: usize) -> Res<Vec<SF>> {
    //let buf = chunk.into_iter().collect::<Vec<SF>>();
    let mut buf: Vec<SF>;
    buf = vec![0; channels*frames];
    let capture_state = pcmdev.state();
    if capture_state == State::XRun {
        pcmdev.prepare().unwrap();
    }
    let frames = io.readi(&mut buf)?;
    //let chunk = AudioChunk::from_interleaved(buf, 2);
    Ok(buf)
}

fn run() -> Res<()> {
    let (playback_dev, play_rate) = open_audio_dev_play("hw:PCH".to_string(), 44100, 1024)?;
    let (capture_dev, capt_rate) = open_audio_dev_capt("hw:PCH".to_string(), 44100, 1024)?;

    
    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    //let mut mmap = playback_dev.direct_mmap_playback::<SF>()?;

    thread::spawn(move || {
        let coeffs = Coefficients::<f64>::new(-1.97984856, 0.98004953, 5.02413473e-5, 1.00482695e-4, 5.02413473e-5);
        let filter = BiquadDF2T::<f64>::new(coeffs);
        loop {
            match rx_cap.recv() {
                Ok(Message::Audio(chunk)) => {
                    let mut buf = vec![0i16; 1024];
                    for (i, a) in buf.iter_mut().enumerate() {
                        *a = ((i as f32 * 2.0 * ::std::f32::consts::PI / 128.0).sin() * 8192.0) as i16
                    }
                    buf = filter.process_multi(buf);

                    let chunk = AudioChunk{
                        frames: 1024,
                        channels: 2,
                        waveforms: vec![buf.clone(),
                                        buf],
                    };
                    let msg = Message::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let delay = time::Duration::from_millis(2*1000*1024/44100);
        thread::sleep(delay);
        let mut io_play = playback_dev.io_i16().unwrap();
        let mut m = 0;
        loop {
            match rx_pb.recv() {
                Ok(Message::Audio(chunk)) => {
                    let buf = chunk.to_interleaved();
                    let frames = play_from_buffer(&playback_dev, &mut io_play, buf);
                    println!("PB Chunk {}, wrote {:?} frames", m, frames);
                    m += 1;
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let mut io_capt = capture_dev.io_i16().unwrap();
        let mut m = 0;
        loop {
            let buf = capture_to_buffer(&capture_dev, &mut io_capt, 2, 1024).unwrap();
            let chunk = AudioChunk::from_interleaved(buf, 2);
            let msg = Message::Audio(chunk);
            tx_cap.send(msg).unwrap();
            println!("Capture chunk {}", m);
            m += 1;
        }
    });

    let delay = time::Duration::from_millis(100);
    

    loop {
        thread::sleep(delay);
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() { println!("Error ({}) {}", e.description(), e); }
}
