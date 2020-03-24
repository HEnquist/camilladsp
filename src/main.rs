#[cfg(feature = "alsa-backend")]
extern crate alsa;
extern crate clap;
#[cfg(feature = "FFTW")]
extern crate fftw;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate num;
extern crate rand;
extern crate rand_distr;
#[cfg(not(feature = "FFTW"))]
extern crate rustfft;
extern crate serde;
extern crate signal_hook;
#[cfg(feature = "socketserver")]
extern crate ws;

#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{crate_authors, crate_description, crate_version, App, AppSettings, Arg};
use env_logger::Builder;
use log::LevelFilter;
use std::env;
use std::error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::time;

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
mod diffeq;
mod dither;
#[cfg(not(feature = "FFTW"))]
mod fftconv;
#[cfg(feature = "FFTW")]
mod fftconv_fftw;
mod fifoqueue;
mod filedevice;
mod filters;
mod mixer;
mod processing;
#[cfg(feature = "pulse-backend")]
mod pulsedevice;
#[cfg(feature = "socketserver")]
mod socketserver;

//use audiodevice::*;

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

fn get_new_config(
    active_config: &config::Configuration,
    config_path: &Arc<Mutex<String>>,
    active_config_shared: &Arc<Mutex<config::Configuration>>,
) -> Res<config::Configuration> {
    let conf = active_config_shared.lock().unwrap().clone();
    //if *config_path.lock().unwrap() == "none" {
    if &conf != active_config {
        debug!("Reload using config from websocket");
        //let conf = active_config_shared.lock().unwrap().clone();
        match config::validate_config(conf.clone()) {
            Ok(()) => {
                debug!("Config valid");
                Ok(conf)
            }
            Err(err) => {
                error!("Invalid config file!");
                error!("{}", err);
                Err(err)
            }
        }
    } else {
        match config::load_config(&config_path.lock().unwrap()) {
            Ok(conf) => match config::validate_config(conf.clone()) {
                Ok(()) => {
                    debug!("Reload using config file");
                    Ok(conf)
                }
                Err(err) => {
                    error!("Invalid config file!");
                    error!("{}", err);
                    Err(err)
                }
            },
            Err(err) => {
                error!("Config file error:");
                error!("{}", err);
                Err(err)
            }
        }
    }
}

fn run(
    conf: config::Configuration,
    signal_reload: Arc<AtomicBool>,
    active_config_shared: Arc<Mutex<config::Configuration>>,
    config_path: Arc<Mutex<String>>,
) -> Res<ExitStatus> {
    let (tx_pb, rx_pb) = mpsc::sync_channel(conf.devices.queuelimit);
    let (tx_cap, rx_cap) = mpsc::sync_channel(conf.devices.queuelimit);

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
    //let conf_yaml = serde_yaml::to_string(&active_config).unwrap();
    *active_config_shared.lock().unwrap() = active_config.clone();

    // Processing thread
    processing::run_processing(conf_proc, barrier_proc, tx_pb, rx_cap, rx_pipeconf);

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
    signal_hook::flag::register(signal_hook::SIGHUP, Arc::clone(&signal_reload))?;

    loop {
        if signal_reload.load(Ordering::Relaxed) {
            debug!("Reloading configuration...");
            signal_reload.store(false, Ordering::Relaxed);
            let new_config = get_new_config(&active_config, &config_path, &active_config_shared);

            match new_config {
                Ok(conf) => {
                    let comp = config::config_diff(&active_config, &conf);
                    match comp {
                        config::ConfigChange::Pipeline
                        | config::ConfigChange::FilterParameters { .. } => {
                            tx_pipeconf.send((comp, conf.clone())).unwrap();
                            active_config = conf;
                            *active_config_shared.lock().unwrap() = active_config.clone();
                            debug!("Sent changes to pipeline");
                        }
                        config::ConfigChange::Devices => {
                            debug!("Devices changed, restart required.");
                            tx_command_cap.send(CommandMessage::Exit).unwrap();
                            trace!("Wait for pb..");
                            pb_handle.join().unwrap();
                            trace!("Wait for cap..");
                            cap_handle.join().unwrap();
                            return Ok(ExitStatus::Restart(Box::new(conf)));
                        }
                        config::ConfigChange::None => {
                            debug!("No changes in config.");
                        }
                    };
                }
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
    let clapapp = App::new("CamillaDSP")
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
                .help("Increase message verbosity"),
        );
    #[cfg(feature = "socketserver")]
    let clapapp = clapapp.arg(
        Arg::with_name("port")
            .help("Port for websocket server")
            .short("p")
            .long("port")
            .takes_value(true)
            .default_value("0")
            .hide_default_value(true)
            .validator(|v: String| -> Result<(), String> {
                if let Ok(port) = v.parse::<usize>() {
                    if port < 65535 {
                        return Ok(());
                    }
                }
                Err(String::from("Must be an integer between 0 and 65535"))
            }),
    );
    let matches = clapapp.get_matches();

    let loglevel = match matches.occurrences_of("verbosity") {
        0 => LevelFilter::Info,
        1 => LevelFilter::Debug,
        2 => LevelFilter::Trace,
        _ => LevelFilter::Trace,
    };

    let mut builder = Builder::from_default_env();

    builder.filter(None, loglevel).init();
    // logging examples
    //trace!("trace message"); //with -vv
    //debug!("debug message"); //with -v
    //info!("info message");
    //warn!("warn message");
    //error!("error message");

    let configname = matches.value_of("configfile").unwrap(); //&args[1];

    //let mut configuration = match config::load_config(&configname) {
    //    Ok(config) => config,
    //    Err(err) => {
    //        error!("Config file error:");
    //        error!("{}", err);
    //        return;
    //    }
    //};
    //
    //match config::validate_config(configuration.clone()) {
    //    Ok(()) => {
    //        info!("Config is valid");
    //    }
    //    Err(err) => {
    //        error!("Invalid config file!");
    //        error!("{}", err);
    //        return;
    //    }
    //}

    let mut configuration = match config::load_validate_config(&configname) {
        Ok(conf) => conf,
        _ => return,
    };

    if matches.is_present("check") {
        return;
    }

    let signal_reload = Arc::new(AtomicBool::new(false));
    //let active_config = Arc::new(Mutex::new(String::new()));
    let active_config = Arc::new(Mutex::new(configuration.clone()));

    let active_config_path = Arc::new(Mutex::new(configname.to_string()));

    #[cfg(feature = "socketserver")]
    let serverport = matches.value_of("port").unwrap().parse::<usize>().unwrap();
    #[cfg(feature = "socketserver")]
    {
        if serverport > 0 {
            socketserver::start_server(
                serverport,
                signal_reload.clone(),
                active_config.clone(),
                active_config_path.clone(),
            );
        }
    }

    loop {
        let exitstatus = run(
            configuration,
            signal_reload.clone(),
            active_config.clone(),
            active_config_path.clone(),
        );
        match exitstatus {
            Err(e) => {
                error!("({}) {}", e.to_string(), e);
                break;
            }
            Ok(ExitStatus::Exit) => {
                debug!("Exiting");
                break;
            }
            Ok(ExitStatus::Restart(conf)) => {
                debug!("Restarting with new config");
                configuration = *conf
            }
        };
    }
}
