extern crate alsa;
extern crate serde;
extern crate rustfft;

use std::error;
use std::env;
//use alsa::{Direction, ValueOr};
//use alsa::pcm::{PCM, HwParams, Format, Access, State};
//use alsa::direct::pcm::MmapPlayback;
use std::{thread, time};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};

// Sample format
pub type PrcFmt = f64;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

mod filters;
mod biquad;
mod fftconv;
mod basicfilters;

mod audiodevice;
mod alsadevice;
use audiodevice::*;

mod config;

mod mixer;
//use config;

//use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;
//use std::path::PathBuf;

pub enum StatusMessage {
    PlaybackReady,
    CaptureReady,
    PlaybackError { message: String },
    CaptureError { message: String },
}

fn run(conf: config::Configuration) -> Res<()> {

    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    let (tx_status, rx_status) = mpsc::channel();
    let tx_status_pb = tx_status.clone();
    let tx_status_cap = tx_status.clone();

    let barrier = Arc::new(Barrier::new(4));
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
                Ok(AudioMessage::Audio(mut chunk)) => {
                    chunk = pipeline.process_chunk(chunk);
                    let msg = AudioMessage::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
        }
    });


    // Playback thread
    let mut playback_dev = audiodevice::get_playback_device(conf_pb.devices);
    let _pb_handle = playback_dev.start(rx_pb, barrier_pb, tx_status_pb);


    // Capture thread
    let mut capture_dev = audiodevice::get_capture_device(conf_cap.devices);
    let _cap_handle = capture_dev.start(tx_cap, barrier_cap, tx_status_cap);

    let delay = time::Duration::from_millis(100);
    
    let mut pb_ready = false;
    let mut cap_ready = false;
    loop {
        match rx_status.recv_timeout(delay) {
            Ok(msg) => {
                match msg {
                    StatusMessage::PlaybackReady => {
                        pb_ready = true;
                        if cap_ready {
                            barrier.wait();
                        }
                    }
                    StatusMessage::CaptureReady => {
                        cap_ready = true;
                        if pb_ready {
                            barrier.wait();
                        }
                    }
                    StatusMessage::PlaybackError { message } => {
                        println!("Playback error: {}", message);
                        return Ok(());
                    }
                    StatusMessage::CaptureError{ message } => {
                        println!("Capture error: {}", message);
                        return Ok(());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let configname = &args[1];
    let file = match File::open(configname) {
        Ok(f) => f,
        Err(_) => {
            println!("Could not open config file!");
            return ()
        },
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(_err) => {
            println!("Could not read config file!");
            return ()
        },
    };
    let configuration: config::Configuration = match serde_yaml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            println!("Invalid config file!");
            println!("{}", err);
            return ()
        },
    };

    match config::validate_config(configuration.clone()) {
        Ok(()) => {},
        Err(err) => {
            println!("Invalid config file!");
            println!("{}", err);
            return ()
        },
    }
    //println!("config {:?}", configuration);

    //read_coeff_file("filter.txt");
    if let Err(e) = run(configuration) { println!("Error ({}) {}", e.description(), e); }
}
