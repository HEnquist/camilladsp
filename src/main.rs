extern crate alsa;
extern crate serde;
extern crate rustfft;

use std::error;
//use alsa::{Direction, ValueOr};
//use alsa::pcm::{PCM, HwParams, Format, Access, State};
//use alsa::direct::pcm::MmapPlayback;
use std::{thread, time};
use std::sync::mpsc;

type Res<T> = Result<T, Box<dyn error::Error>>;

mod filters;
use filters::*;
mod biquad;
use biquad::*;
mod fftconv;
use fftconv::*;

mod audiodevice;
mod alsadevice;
use audiodevice::*;
use alsadevice::*;

mod config;

mod mixer;
//use config;

//use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
//use std::path::PathBuf;

//use serde::{Serialize, Deserialize};

//pub use crate::filters::*;
//pub use crate::biquad::*;



// Sample format
//type SmpFmt = i16;
//type PrcFmt = f64;


enum Message {
    Quit,
    Audio(AudioChunk),
}



fn run(conf: config::Configuration) -> Res<()> {
    let chunksize: i64 = 1024;

    //let mut playback_dev = AlsaPlaybackDevice::open("hw:Generic_1".to_string(), 44100, chunksize, 2)?;
    let mut playback_dev = audiodevice::GetPlaybackDevice(conf.devices.clone());
    //let mut capture_dev = AlsaCaptureDevice::open("hw:Loopback,0,0".to_string(), 44100, chunksize, 2)?;
    let mut capture_dev = audiodevice::GetCaptureDevice(conf.devices.clone());
    //let (playback_dev, play_rate) = open_audio_dev_play("hw:PCH".to_string(), 44100, 1024)?;
    //let (capture_dev, capt_rate) = open_audio_dev_capt("hw:PCH".to_string(), 44100, 1024)?;

    let pipeline = filters::Pipeline::from_config(conf);
    
    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    
    //let mut mmap = playback_dev.direct_mmap_playback::<SF>()?;

    thread::spawn(move || {
        //let coeffs = BiquadCoefficients::new(-1.79907162, 0.81748736, 0.00460394, 0.00920787, 0.00460394);
        //let mut filter_l = Biquad::new(coeffs);
        //let mut filter_r = Biquad::new(coeffs);
        let coeffs = read_coeff_file("filter.txt").unwrap();
        let mut filter_l = FFTConv::new(chunksize as usize, &coeffs);
        let mut filter_r = FFTConv::new(chunksize as usize, &coeffs);
        println!("build filters, starting processing loop");
        loop {
            match rx_cap.recv() {
                Ok(Message::Audio(mut chunk)) => {
                    //let mut buf = vec![0f64; 1024];
                    //for (i, a) in buf.iter_mut().enumerate() {
                    //    *a = (i as f64 * 2.0 * ::std::f64::consts::PI / 128.0).sin();
                    //}
                    //let mut filtered_wfs = Vec::new();
                    //for wave in wfs.iter() {
                    let _res_l = filter_l.process_waveform(&mut chunk.waveforms[0]);
                    //filtered_wfs.push(filtered_l);
                    let _res_r = filter_r.process_waveform(&mut chunk.waveforms[1]);
                    //filtered_wfs.push(filtered_r);

                    //let chunk = AudioChunk{
                    //    frames: chunksize as usize,
                    //    channels: 2,
                    //    waveforms: filtered_wfs,
                    //    //waveforms: Waveforms::Float64(vec![buf.clone(), buf]),
                    //};
                    let msg = Message::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let delay = time::Duration::from_millis(8*1000*1024/44100);
        thread::sleep(delay);
        let mut m = 0;
        println!("starting playback loop");
        loop {
            match rx_pb.recv() {
                Ok(Message::Audio(chunk)) => {
                    playback_dev.put_chunk(chunk).unwrap();
                    let frames = playback_dev.play().unwrap();
                    println!("PB Chunk {}, wrote {:?} frames", m, frames);
                    m += 1;
                }
                _ => {}
            }
        }
    });

    thread::spawn(move || {
        let mut m = 0;
        println!("starting capture loop");
        loop {
            let _frames = capture_dev.capture().unwrap();
            let chunk = capture_dev.fetch_chunk().unwrap();
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
    let file = File::open("src/simpleconfig.yml")
        .expect("could not open file");
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(_err) => 0
    };
    let configuration: config::Configuration = serde_yaml::from_str(&contents).unwrap();
    println!("config {:?}", configuration);

    for (name, mix) in configuration.mixers.clone() {
        let newmix = mixer::Mixer::from_config(mix);
    }

    //read_coeff_file("filter.txt");
    if let Err(e) = run(configuration) { println!("Error ({}) {}", e.description(), e); }
}
