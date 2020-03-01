#[cfg(feature = "alsa-backend")]
extern crate alsa;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate rustfft;
extern crate serde;
extern crate signal_hook;

use std::env;
use std::error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::{thread, time};

// Sample format
#[cfg(feature = "32bit")]
pub type PrcFmt = f32;
#[cfg(not(feature = "32bit"))]
pub type PrcFmt = f64;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

#[cfg(feature = "alsa-backend")]
mod alsadevice;
mod audiodevice;
mod basicfilters;
mod biquad;
mod config;
mod conversions;
mod fftconv;
mod fifoqueue;
mod filedevice;
mod filters;
mod mixer;
#[cfg(feature = "pulse-backend")]
mod pulsedevice;

use audiodevice::*;

pub enum StatusMessage {
    PlaybackReady,
    CaptureReady,
    PlaybackError { message: String },
    CaptureError { message: String },
    PlaybackDone,
    CaptureDone,
}

fn run(conf: config::Configuration, configname: &str) -> Res<()> {
    let (tx_pb, rx_pb) = mpsc::channel();
    let (tx_cap, rx_cap) = mpsc::channel();

    let (tx_status, rx_status) = mpsc::channel();
    let tx_status_pb = tx_status.clone();
    let tx_status_cap = tx_status;

    let (tx_reload, rx_reload) = mpsc::channel();

    let barrier = Arc::new(Barrier::new(4));
    let barrier_pb = barrier.clone();
    let barrier_cap = barrier.clone();
    let barrier_proc = barrier.clone();

    let conf_pb = conf.clone();
    let conf_cap = conf.clone();
    let conf_proc = conf.clone();

    let mut active_config = conf;

    // Processing thread
    thread::spawn(move || {
        let mut pipeline = filters::Pipeline::from_config(conf_proc);
        eprintln!("build filters, waiting to start processing loop");
        barrier_proc.wait();
        loop {
            match rx_cap.recv() {
                Ok(AudioMessage::Audio(mut chunk)) => {
                    chunk = pipeline.process_chunk(chunk);
                    let msg = AudioMessage::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                Ok(AudioMessage::EndOfStream) => {
                    let msg = AudioMessage::EndOfStream;
                    tx_pb.send(msg).unwrap();
                }
                _ => {}
            }
            if let Ok((diff, new_config)) = rx_reload.try_recv() {
                match diff {
                    config::ConfigChange::Pipeline => {
                        eprintln!("Rebuilding pipeline.");
                        let new_pipeline = filters::Pipeline::from_config(new_config);
                        pipeline = new_pipeline;
                    }
                    config::ConfigChange::FilterParameters { filters, mixers } => {
                        eprintln!(
                            "Updating parameters of filters: {:?}, mixers: {:?}.",
                            filters, mixers
                        );
                        pipeline.update_parameters(new_config, filters, mixers);
                    }
                    config::ConfigChange::Devices => {
                        eprintln!("Devices changed, restart required.");
                    }
                    config::ConfigChange::None => {
                        eprintln!("No changes in config.");
                    }
                };
            };
        }
    });

    // Playback thread
    let mut playback_dev = audiodevice::get_playback_device(conf_pb.devices);
    let _pb_handle = playback_dev.start(rx_pb, barrier_pb, tx_status_pb);

    // Capture thread
    let mut capture_dev = audiodevice::get_capture_device(conf_cap.devices);
    let _cap_handle = capture_dev.start(tx_cap, barrier_cap, tx_status_cap);

    let delay = time::Duration::from_millis(1000);

    let mut pb_ready = false;
    let mut cap_ready = false;
    let reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::SIGHUP, Arc::clone(&reload))?;

    loop {
        if reload.load(Ordering::Relaxed) {
            eprintln!("Time to reload!");
            reload.store(false, Ordering::Relaxed);
            match config::load_config(&configname) {
                Ok(new_config) => match config::validate_config(new_config.clone()) {
                    Ok(()) => {
                        let comp = config::config_diff(&active_config, &new_config);
                        eprintln!("diff {:?}", comp);
                        if new_config.devices == active_config.devices {
                            eprintln!("Ok to reload");
                            tx_reload.send((comp, new_config.clone())).unwrap();
                            active_config = new_config;
                        }
                    }
                    Err(err) => {
                        eprintln!("Invalid config file!");
                        eprintln!("{}", err);
                    }
                },
                Err(err) => {
                    eprintln!("Config file error:");
                    eprintln!("{}", err);
                }
            };
        }
        match rx_status.recv_timeout(delay) {
            Ok(msg) => match msg {
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
                    eprintln!("Playback error: {}", message);
                    return Ok(());
                }
                StatusMessage::CaptureError { message } => {
                    eprintln!("Capture error: {}", message);
                    return Ok(());
                }
                StatusMessage::PlaybackDone => {
                    eprintln!("Playback finished");
                    return Ok(());
                }
                StatusMessage::CaptureDone => {
                    eprintln!("Capture finished");
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("No config file given!");
        return;
    }
    let configname = &args[1];
    let configuration = match config::load_config(&configname) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Config file error:");
            eprintln!("{}", err);
            return;
        }
    };

    match config::validate_config(configuration.clone()) {
        Ok(()) => {}
        Err(err) => {
            eprintln!("Invalid config file!");
            eprintln!("{}", err);
            return;
        }
    }
    if let Err(e) = run(configuration, &configname) {
        eprintln!("Error ({}) {}", e.description(), e);
    }
}
