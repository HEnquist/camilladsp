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


type Res<T> = Result<T, Box<dyn error::Error>>;

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
    let _pb_handle = playback_dev.start(rx_pb, barrier_pb);


    // Capture thread
    let mut capture_dev = audiodevice::get_capture_device(conf_cap.devices);
    let _cap_handle = capture_dev.start(tx_cap, barrier_cap);

    let delay = time::Duration::from_millis(100);
    

    loop {
        thread::sleep(delay);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let configname = &args[1];
    let file = File::open(configname)
        .expect("could not open file");
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(_err) => 0
    };
    let configuration: config::Configuration = serde_yaml::from_str(&contents).unwrap();
    println!("config {:?}", configuration);

    //read_coeff_file("filter.txt");
    if let Err(e) = run(configuration) { println!("Error ({}) {}", e.description(), e); }
}
