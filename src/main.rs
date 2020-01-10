extern crate alsa;
extern crate serde;
extern crate rustfft;

use std::error;
//use alsa::{Direction, ValueOr};
//use alsa::pcm::{PCM, HwParams, Format, Access, State};
//use alsa::direct::pcm::MmapPlayback;
use std::{thread, time};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};


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

enum CtrlMessage {
    Start,
}

enum StatusMessage {
    PlaybackReady,
    CaptureReady,
    ProcessingReady,
}

fn run(conf: config::Configuration) -> Res<()> {

    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    let barrier = Arc::new(Barrier::new(3));
    let barrier_pb = barrier.clone();
    let barrier_cap = barrier.clone();
    let barrier_proc = barrier.clone();

    let conf_pb = conf.clone();
    let conf_cap = conf.clone();
    let conf_proc = conf.clone();

    //let mut mmap = playback_dev.direct_mmap_playback::<SF>()?;

    // Processing thread
    thread::spawn(move || {
        let mut pipeline = filters::Pipeline::from_config(conf_proc);
        println!("build filters, waiting to start processing loop");
        barrier_proc.wait();
        loop {
            match rx_cap.recv() {
                Ok(Message::Audio(mut chunk)) => {
                    chunk = pipeline.process_chunk(chunk);
                    let msg = Message::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });


    // Playback thread
    thread::spawn(move || {
        let mut playback_dev = audiodevice::GetPlaybackDevice(conf_pb.devices);
        let delay = time::Duration::from_millis(8*1000*1024/44100);
        barrier_pb.wait();
        thread::sleep(delay);
        println!("starting playback loop");
        loop {
            match rx_pb.recv() {
                Ok(Message::Audio(chunk)) => {
                    playback_dev.put_chunk(chunk).unwrap();
                    let frames = playback_dev.play().unwrap();
                    //println!("PB Chunk {}, wrote {:?} frames", m, frames);
                    //m += 1;
                }
                _ => {}
            }
        }
    });

    // Capture thread
    thread::spawn(move || {
        let mut capture_dev = audiodevice::GetCaptureDevice(conf_cap.devices);
        barrier_cap.wait();
        println!("starting capture loop");
        loop {
            let _frames = capture_dev.capture().unwrap();
            let chunk = capture_dev.fetch_chunk().unwrap();
            let msg = Message::Audio(chunk);
            tx_cap.send(msg).unwrap();
            //println!("Capture chunk {}", m);
            //m += 1;
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
