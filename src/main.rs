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
extern crate serde_with;
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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
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
mod biquadcombo;
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
    Restart,
    Exit,
}

fn get_new_config(
    config_path: &Arc<Mutex<Option<String>>>,
    new_config_shared: &Arc<Mutex<Option<config::Configuration>>>,
) -> Res<config::Configuration> {
    //let active_conf = active_config_shared.lock().unwrap().clone();
    let new_conf = new_config_shared.lock().unwrap().clone();
    let path = config_path.lock().unwrap().clone();

    //new_config is not None, this is the one to use
    if let Some(conf) = new_conf {
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
    } else if let Some(file) = path {
        match config::load_config(&file) {
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
    } else {
        error!("No new config supplied and no path set");
        Err(Box::new(config::ConfigError::new(
            "No new config supplied and no path set",
        )))
    }
}

fn run(
    signal_reload: Arc<AtomicBool>,
    signal_exit: Arc<AtomicUsize>,
    active_config_shared: Arc<Mutex<Option<config::Configuration>>>,
    config_path: Arc<Mutex<Option<String>>>,
    new_config_shared: Arc<Mutex<Option<config::Configuration>>>,
) -> Res<ExitStatus> {
    let conf = match new_config_shared.lock().unwrap().clone() {
        Some(cfg) => cfg,
        None => {
            error!("Tried to start without config!");
            return Ok(ExitStatus::Exit);
        }
    };
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
    *active_config_shared.lock().unwrap() = Some(active_config.clone());
    *new_config_shared.lock().unwrap() = None;

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
            let new_config = get_new_config(&config_path, &new_config_shared);

            match new_config {
                Ok(conf) => {
                    let comp = config::config_diff(&active_config, &conf);
                    match comp {
                        config::ConfigChange::Pipeline
                        | config::ConfigChange::FilterParameters { .. } => {
                            tx_pipeconf.send((comp, conf.clone())).unwrap();
                            active_config = conf;
                            *active_config_shared.lock().unwrap() = Some(active_config.clone());
                            *new_config_shared.lock().unwrap() = None;
                            debug!("Sent changes to pipeline");
                        }
                        config::ConfigChange::Devices => {
                            debug!("Devices changed, restart required.");
                            tx_command_cap.send(CommandMessage::Exit).unwrap();
                            trace!("Wait for pb..");
                            pb_handle.join().unwrap();
                            trace!("Wait for cap..");
                            cap_handle.join().unwrap();
                            *new_config_shared.lock().unwrap() = Some(conf);
                            return Ok(ExitStatus::Restart);
                        }
                        config::ConfigChange::None => {
                            debug!("No changes in config.");
                            *new_config_shared.lock().unwrap() = None;
                        }
                    };
                }
                Err(err) => {
                    error!("Config file error:");
                    error!("{}", err);
                }
            };
        }
        match signal_exit.load(Ordering::Relaxed) {
            1 => {
                debug!("Exit requested...");
                signal_exit.store(0, Ordering::Relaxed);
                tx_command_cap.send(CommandMessage::Exit).unwrap();
                trace!("Wait for pb..");
                pb_handle.join().unwrap();
                trace!("Wait for cap..");
                cap_handle.join().unwrap();
                return Ok(ExitStatus::Exit);
            }
            2 => {
                debug!("Stop requested...");
                signal_exit.store(0, Ordering::Relaxed);
                tx_command_cap.send(CommandMessage::Exit).unwrap();
                trace!("Wait for pb..");
                pb_handle.join().unwrap();
                trace!("Wait for cap..");
                cap_handle.join().unwrap();
                *new_config_shared.lock().unwrap() = None;
                return Ok(ExitStatus::Restart);
            }
            _ => {}
        };
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
    let mut features = Vec::new();
    if cfg!(feature = "alsa-backend") {
        features.push("alsa-backend");
    }
    if cfg!(feature = "pulse-backend") {
        features.push("pulse-backend");
    }
    if cfg!(feature = "socketserver") {
        features.push("socketserver");
    }
    if cfg!(feature = "FFTW") {
        features.push("FFTW");
    }
    if cfg!(feature = "32bit") {
        features.push("32bit");
    }
    let featurelist = format!("Built with features: {}", features.join(", "));
    let longabout = format!("{}\n\n{}", crate_description!(), featurelist);

    let clapapp = App::new("CamillaDSP")
        .version(crate_version!())
        .about(longabout.as_str())
        .author(crate_authors!())
        .setting(AppSettings::ArgRequiredElseHelp)
        .arg(
            Arg::with_name("configfile")
                .help("The configuration file to use")
                .index(1)
                //.required(true),
                .required_unless("wait"),
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
    let clapapp = clapapp
        .arg(
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
        )
        .arg(
            Arg::with_name("wait")
                .short("w")
                .long("wait")
                .help("Wait for config from websocket")
                .conflicts_with("configfile"),
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

    let configname = match matches.value_of("configfile") {
        Some(path) => Some(path.to_string()),
        None => None,
    };

    debug!("Read config file {:?}", configname);

    let configuration = match &configname {
        Some(path) => match config::load_validate_config(&path.clone()) {
            Ok(conf) => {
                debug!("Config is valid");
                Some(conf)
            }
            Err(err) => {
                error!("{}", err);
                debug!("Exiting due to config error");
                return;
            }
        },
        None => None,
    };

    if matches.is_present("check") {
        debug!("Check only, done!");
        return;
    }

    let signal_reload = Arc::new(AtomicBool::new(false));
    let signal_exit = Arc::new(AtomicUsize::new(0));
    //let active_config = Arc::new(Mutex::new(String::new()));
    let active_config = Arc::new(Mutex::new(None));
    let new_config = Arc::new(Mutex::new(configuration));

    let active_config_path = Arc::new(Mutex::new(configname));

    #[cfg(feature = "socketserver")]
    {
        let serverport = matches.value_of("port").unwrap().parse::<usize>().unwrap();
        if serverport > 0 {
            socketserver::start_server(
                serverport,
                signal_reload.clone(),
                signal_exit.clone(),
                active_config.clone(),
                active_config_path.clone(),
                new_config.clone(),
            );
        }
    }

    let delay = time::Duration::from_millis(1000);
    loop {
        debug!("Wait for config");
        while new_config.lock().unwrap().is_none() {
            trace!("waiting...");
            thread::sleep(delay);
        }
        debug!("Config ready");
        let exitstatus = run(
            signal_reload.clone(),
            signal_exit.clone(),
            active_config.clone(),
            active_config_path.clone(),
            new_config.clone(),
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
            Ok(ExitStatus::Restart) => {
                debug!("Restarting with new config");
            }
        };
    }
}
