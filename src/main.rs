#[cfg(feature = "alsa-backend")]
extern crate alsa;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate rand;
extern crate rand_distr;
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
mod dither;
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
    SetSpeed { speed: f32 },
}

pub enum CommandMessage {
    SetSpeed { speed: f32 },
    Exit,
}

enum ExitStatus {
    Restart(Box<config::Configuration>),
    Exit,
}

fn run(conf: config::Configuration, configname: &str) -> Res<ExitStatus> {
    let (tx_pb, rx_pb) = mpsc::sync_channel(128);
    let (tx_cap, rx_cap) = mpsc::sync_channel(128);

    let (tx_status, rx_status) = mpsc::channel();
    let tx_status_pb = tx_status.clone();
    let tx_status_cap = tx_status;

    let (tx_command_cap, rx_command_cap) = mpsc::channel();
    let (tx_pipeconf, rx_pipeconf) = mpsc::channel();

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
                    break;
                }
                _ => {}
            }
            if let Ok((diff, new_config)) = rx_pipeconf.try_recv() {
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
                        let msg = AudioMessage::EndOfStream;
                        tx_pb.send(msg).unwrap();
                        break;
                    }
                    _ => {}
                };
            };
        }
    });

    // Playback thread
    let mut playback_dev = audiodevice::get_playback_device(conf_pb.devices);
    let pb_handle = playback_dev.start(rx_pb, barrier_pb, tx_status_pb).unwrap();

    // Capture thread
    let mut capture_dev = audiodevice::get_capture_device(conf_cap.devices);
    let cap_handle = capture_dev
        .start(tx_cap, barrier_cap, tx_status_cap, rx_command_cap)
        .unwrap();

    let delay = time::Duration::from_millis(100);

    let mut pb_ready = false;
    let mut cap_ready = false;
    let signal_reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::SIGHUP, Arc::clone(&signal_reload))?;

    loop {
        if signal_reload.load(Ordering::Relaxed) {
            eprintln!("Reloading configuration...");
            signal_reload.store(false, Ordering::Relaxed);
            match config::load_config(&configname) {
                Ok(new_config) => match config::validate_config(new_config.clone()) {
                    Ok(()) => {
                        let comp = config::config_diff(&active_config, &new_config);
                        match comp {
                            config::ConfigChange::Pipeline
                            | config::ConfigChange::FilterParameters { .. } => {
                                tx_pipeconf.send((comp, new_config.clone())).unwrap();
                                active_config = new_config;
                            }
                            config::ConfigChange::Devices => {
                                eprintln!("Devices changed, restart required.");
                                //tx_pipeconf.send((comp, new_config.clone())).unwrap();
                                tx_command_cap.send(CommandMessage::Exit).unwrap();
                                //tx_command_pb.send(CommandMessage::Exit).unwrap();
                                eprintln!("Wait for pb..");
                                pb_handle.join().unwrap();
                                eprintln!("Wait for cap..");
                                cap_handle.join().unwrap();
                                return Ok(ExitStatus::Restart(Box::new(new_config)));
                            }
                            config::ConfigChange::None => {
                                eprintln!("No changes in config.");
                            }
                        };
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
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::CaptureError { message } => {
                    eprintln!("Capture error: {}", message);
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::PlaybackDone => {
                    eprintln!("Playback finished");
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::CaptureDone => {
                    eprintln!("Capture finished");
                }
                StatusMessage::SetSpeed { speed } => {
                    eprintln!("Change speed to: {}%", 100.0*speed);
                    tx_command_cap.send(CommandMessage::SetSpeed{ speed }).unwrap();
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
    let mut configuration = match config::load_config(&configname) {
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
    loop {
        let exitstatus = run(configuration, &configname);
        match exitstatus {
            Err(e) => {
                eprintln!("Error ({}) {}", e.description(), e);
                break;
            }
            Ok(ExitStatus::Exit) => {
                break;
            }
            Ok(ExitStatus::Restart(conf)) => configuration = *conf,
        };
    }
}
