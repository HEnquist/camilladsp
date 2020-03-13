#[cfg(feature = "alsa-backend")]
extern crate alsa;
extern crate clap;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate rand;
extern crate rand_distr;
extern crate rustfft;
extern crate serde;
extern crate signal_hook;

#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{crate_authors, crate_description, crate_version, App, Arg, AppSettings};
use std::env;
use std::error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::{thread, time};
use log::LevelFilter;
use env_logger::Builder;

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
        debug!("build filters, waiting to start processing loop");
        barrier_proc.wait();
        loop {
            match rx_cap.recv() {
                Ok(AudioMessage::Audio(mut chunk)) => {
                    trace!("AudioMessage::Audio received");
                    chunk = pipeline.process_chunk(chunk);
                    let msg = AudioMessage::Audio(chunk);
                    tx_pb.send(msg).unwrap();
                }
                Ok(AudioMessage::EndOfStream) => {
                    trace!("AudioMessage::EndOfStream received");
                    let msg = AudioMessage::EndOfStream;
                    tx_pb.send(msg).unwrap();
                    break;
                }
                _ => {}
            }
            if let Ok((diff, new_config)) = rx_pipeconf.try_recv() {
                trace!("Message received on config channel");
                match diff {
                    config::ConfigChange::Pipeline => {
                        debug!("Rebuilding pipeline.");
                        let new_pipeline = filters::Pipeline::from_config(new_config);
                        pipeline = new_pipeline;
                    }
                    config::ConfigChange::FilterParameters { filters, mixers } => {
                        debug!(
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
            debug!("Reloading configuration...");
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
                                debug!("Devices changed, restart required.");
                                //tx_pipeconf.send((comp, new_config.clone())).unwrap();
                                tx_command_cap.send(CommandMessage::Exit).unwrap();
                                //tx_command_pb.send(CommandMessage::Exit).unwrap();
                                trace!("Wait for pb..");
                                pb_handle.join().unwrap();
                                trace!("Wait for cap..");
                                cap_handle.join().unwrap();
                                return Ok(ExitStatus::Restart(Box::new(new_config)));
                            }
                            config::ConfigChange::None => {
                                debug!("No changes in config.");
                            }
                        };
                    }
                    Err(err) => {
                        error!("Invalid config file!");
                        error!("{}", err);
                    }
                },
                Err(err) => {
                    error!("Config file error:");
                    error!("{}", err);
                }
            };
        }
        match rx_status.recv_timeout(delay) {
            Ok(msg) => match msg {
                StatusMessage::PlaybackReady => {
                    debug!("Playback thread ready to start");
                    pb_ready = true;
                    if cap_ready {
                        barrier.wait();
                    }
                }
                StatusMessage::CaptureReady => {
                    debug!("Capture thread ready to start");
                    cap_ready = true;
                    if pb_ready {
                        barrier.wait();
                    }
                }
                StatusMessage::PlaybackError { message } => {
                    error!("Playback error: {}", message);
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::CaptureError { message } => {
                    error!("Capture error: {}", message);
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::PlaybackDone => {
                    info!("Playback finished");
                    return Ok(ExitStatus::Exit);
                }
                StatusMessage::CaptureDone => {
                    info!("Capture finished");
                }
                StatusMessage::SetSpeed { speed } => {
                    debug!("SetSpeed message reveiced");
                    tx_command_cap
                        .send(CommandMessage::SetSpeed { speed })
                        .unwrap();
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => {}
        }
    }
}

fn main() {

    let matches = App::new("CamillaDSP")
        .version(crate_version!())
        .about(crate_description!())
        .author(crate_authors!())
        .setting(AppSettings::ArgRequiredElseHelp)
        .arg(
            Arg::with_name("configfile")
                .help("The configuration file to use")
                .index(1)
                .required(true),
        )
        .arg(
            Arg::with_name("check")
                .help("Check config file and exit")
                .short("c")
                .long("check"),
        )
        .arg(
            Arg::with_name("verbosity")
                .short("v")
                .multiple(true)
                .help("Increase message verbosity"))
        .get_matches();

    let loglevel = match matches.occurrences_of("verbosity") {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        2 => LevelFilter::Trace,
        _ => LevelFilter::Trace,
    };

    let mut builder = Builder::from_default_env();

    builder.filter(None, loglevel)
           .init();
    // logging examples    
    //trace!("trace message"); //with -vv
    //debug!("debug message"); //with -v
    //info!("info message");
    //warn!("warn message");
    //error!("error message");

    let configname = matches.value_of("configfile").unwrap(); //&args[1];
    let mut configuration = match config::load_config(&configname) {
        Ok(config) => config,
        Err(err) => {
            error!("Config file error:");
            error!("{}", err);
            return;
        }
    };

    match config::validate_config(configuration.clone()) {
        Ok(()) => {
            info!("Config is valid");
        }
        Err(err) => {
            error!("Invalid config file!");
            error!("{}", err);
            return;
        }
    }
    if matches.is_present("check") {
        return;
    }
    loop {
        let exitstatus = run(configuration, &configname);
        match exitstatus {
            Err(e) => {
                error!("({}) {}", e.description(), e);
                break;
            }
            Ok(ExitStatus::Exit) => {
                debug!("Exiting");
                break;
            }
            Ok(ExitStatus::Restart(conf)) => {
                debug!("Restarting with new config");
                configuration = *conf
            },
        };
    }
}
