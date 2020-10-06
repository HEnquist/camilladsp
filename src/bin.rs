#[cfg(all(feature = "alsa-backend", target_os = "linux"))]
extern crate alsa;
extern crate camillalib;
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
extern crate realfft;
extern crate rubato;
extern crate serde;
extern crate serde_with;
extern crate signal_hook;
#[cfg(feature = "websocket")]
extern crate tungstenite;

#[macro_use]
extern crate log;
extern crate env_logger;

use clap::{crate_authors, crate_description, crate_version, App, AppSettings, Arg};
use env_logger::Builder;
use log::LevelFilter;
use std::env;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex, RwLock};
use std::thread;
use std::time;

use camillalib::Res;

use camillalib::audiodevice;
use camillalib::config;
use camillalib::processing;
#[cfg(feature = "websocket")]
use camillalib::socketserver;
#[cfg(feature = "websocket")]
use std::net::IpAddr;

use camillalib::StatusMessage;

use camillalib::CommandMessage;

use camillalib::ExitState;

use camillalib::CaptureStatus;
use camillalib::ExitRequest;
use camillalib::PlaybackStatus;
use camillalib::ProcessingState;

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
        Err(config::ConfigError::new("No new config supplied and no path set").into())
    }
}

fn run(
    signal_reload: Arc<AtomicBool>,
    signal_exit: Arc<AtomicUsize>,
    active_config_shared: Arc<Mutex<Option<config::Configuration>>>,
    config_path: Arc<Mutex<Option<String>>>,
    new_config_shared: Arc<Mutex<Option<config::Configuration>>>,
    capture_status: Arc<RwLock<CaptureStatus>>,
    playback_status: Arc<RwLock<PlaybackStatus>>,
) -> Res<ExitState> {
    capture_status.write().unwrap().state = ProcessingState::Starting;
    let mut is_starting = true;
    let conf = match new_config_shared.lock().unwrap().clone() {
        Some(cfg) => cfg,
        None => {
            error!("Tried to start without config!");
            return Ok(ExitState::Exit);
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
    signal_reload.store(false, Ordering::Relaxed);
    signal_exit.store(ExitRequest::NONE, Ordering::Relaxed);

    // Processing thread
    processing::run_processing(conf_proc, barrier_proc, tx_pb, rx_cap, rx_pipeconf);

    // Playback thread
    let mut playback_dev = audiodevice::get_playback_device(conf_pb.devices);
    let pb_handle = playback_dev
        .start(rx_pb, barrier_pb, tx_status_pb, playback_status)
        .unwrap();

    // Capture thread
    let mut capture_dev = audiodevice::get_capture_device(conf_cap.devices);
    let cap_handle = capture_dev
        .start(
            tx_cap,
            barrier_cap,
            tx_status_cap,
            rx_command_cap,
            capture_status.clone(),
        )
        .unwrap();

    let delay = time::Duration::from_millis(100);

    let mut pb_ready = false;
    let mut cap_ready = false;
    #[cfg(target_os = "linux")]
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
                            return Ok(ExitState::Restart);
                        }
                        config::ConfigChange::None => {
                            debug!("No changes in config.");
                            *new_config_shared.lock().unwrap() = None;
                        }
                    };
                }
                Err(err) => {
                    error!("Config file error: {}", err);
                }
            };
        }
        if !is_starting {
            match signal_exit.load(Ordering::Relaxed) {
                ExitRequest::EXIT => {
                    debug!("Exit requested...");
                    signal_exit.store(0, Ordering::Relaxed);
                    tx_command_cap.send(CommandMessage::Exit).unwrap();
                    trace!("Wait for pb..");
                    pb_handle.join().unwrap();
                    trace!("Wait for cap..");
                    cap_handle.join().unwrap();
                    return Ok(ExitState::Exit);
                }
                ExitRequest::STOP => {
                    debug!("Stop requested...");
                    signal_exit.store(0, Ordering::Relaxed);
                    tx_command_cap.send(CommandMessage::Exit).unwrap();
                    trace!("Wait for pb..");
                    pb_handle.join().unwrap();
                    trace!("Wait for cap..");
                    cap_handle.join().unwrap();
                    *new_config_shared.lock().unwrap() = None;
                    return Ok(ExitState::Restart);
                }
                _ => {}
            };
        }
        match rx_status.recv_timeout(delay) {
            Ok(msg) => match msg {
                StatusMessage::PlaybackReady => {
                    debug!("Playback thread ready to start");
                    pb_ready = true;
                    if cap_ready {
                        barrier.wait();
                        is_starting = false;
                    }
                }
                StatusMessage::CaptureReady => {
                    debug!("Capture thread ready to start");
                    cap_ready = true;
                    if pb_ready {
                        barrier.wait();
                        is_starting = false;
                    }
                }
                StatusMessage::PlaybackError { message } => {
                    error!("Playback error: {}", message);
                    return Ok(ExitState::Exit);
                }
                StatusMessage::CaptureError { message } => {
                    error!("Capture error: {}", message);
                    return Ok(ExitState::Exit);
                }
                StatusMessage::PlaybackDone => {
                    info!("Playback finished");
                    return Ok(ExitState::Exit);
                }
                StatusMessage::CaptureDone => {
                    info!("Capture finished");
                }
                StatusMessage::SetSpeed { speed } => {
                    debug!("SetSpeed message received");
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
    if cfg!(feature = "cpal-backend") {
        features.push("cpal-backend");
    }
    if cfg!(feature = "websocket") {
        features.push("websocket");
    }
    if cfg!(feature = "secure-websocket") {
        features.push("secure-websocket");
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
                .long("check")
                .requires("configfile"),
        )
        .arg(
            Arg::with_name("verbosity")
                .short("v")
                .multiple(true)
                .help("Increase message verbosity"),
        );
    #[cfg(feature = "websocket")]
    let clapapp = clapapp
        .arg(
            Arg::with_name("port")
                .help("Port for websocket server")
                .short("p")
                .long("port")
                .takes_value(true)
                .validator(|v: String| -> Result<(), String> {
                    if let Ok(port) = v.parse::<usize>() {
                        if port > 0 && port < 65535 {
                            return Ok(());
                        }
                    }
                    Err(String::from("Must be an integer between 0 and 65535"))
                }),
        )
        .arg(
            Arg::with_name("address")
                .help("IP address to bind websocket server to")
                .short("a")
                .long("address")
                .takes_value(true)
                .requires("port")
                .validator(|val: String| -> Result<(), String> {
                    if val.parse::<IpAddr>().is_ok() {
                        return Ok(());
                    }
                    Err(String::from("Must be a valid IP address"))
                }),
        )
        .arg(
            Arg::with_name("wait")
                .short("w")
                .long("wait")
                .help("Wait for config from websocket")
                .requires("port"),
        );
    #[cfg(feature = "secure-websocket")]
    let clapapp = clapapp
        .arg(
            Arg::with_name("cert")
                .long("cert")
                .takes_value(true)
                .help("Path to .pfx/.p12 certificate file")
                .requires("port"),
        )
        .arg(
            Arg::with_name("pass")
                .long("pass")
                .takes_value(true)
                .help("Password for .pfx/.p12 certificate file")
                .requires("port"),
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

    let wait = matches.is_present("wait");

    let signal_reload = Arc::new(AtomicBool::new(false));
    let signal_exit = Arc::new(AtomicUsize::new(0));
    let capture_status = Arc::new(RwLock::new(CaptureStatus {
        measured_samplerate: 0,
        update_interval: 1000,
        signal_range: 0.0,
        rate_adjust: 0.0,
        state: ProcessingState::Inactive,
    }));
    let playback_status = Arc::new(RwLock::new(PlaybackStatus {
        buffer_level: 0,
        clipped_samples: 0,
    }));
    //let active_config = Arc::new(Mutex::new(String::new()));
    let active_config = Arc::new(Mutex::new(None));
    let new_config = Arc::new(Mutex::new(configuration));

    let active_config_path = Arc::new(Mutex::new(configname));

    #[cfg(feature = "websocket")]
    {
        if let Some(port_str) = matches.value_of("port") {
            let serveraddress = match matches.value_of("address") {
                Some(addr) => addr,
                None => "127.0.0.1",
            };
            let serverport = port_str.parse::<usize>().unwrap();
            let shared_data = socketserver::SharedData {
                signal_reload: signal_reload.clone(),
                signal_exit: signal_exit.clone(),
                active_config: active_config.clone(),
                active_config_path: active_config_path.clone(),
                new_config: new_config.clone(),
                capture_status: capture_status.clone(),
                playback_status: playback_status.clone(),
            };
            let server_params = socketserver::ServerParameters {
                port: serverport,
                address: serveraddress,
                #[cfg(feature = "secure-websocket")]
                cert_file: matches.value_of("cert"),
                #[cfg(feature = "secure-websocket")]
                cert_pass: matches.value_of("pass"),
            };
            socketserver::start_server(server_params, shared_data);
        }
    }

    let delay = time::Duration::from_millis(100);
    loop {
        debug!("Wait for config");
        while new_config.lock().unwrap().is_none() {
            trace!("waiting...");
            if signal_exit.load(Ordering::Relaxed) == ExitRequest::EXIT {
                // exit requested
                return;
            } else if signal_reload.load(Ordering::Relaxed) {
                debug!("Reloading configuration...");
                signal_reload.store(false, Ordering::Relaxed);
                let conf_loaded = get_new_config(&active_config_path, &new_config);
                match conf_loaded {
                    Ok(conf) => {
                        debug!(
                            "Loaded config file: {:?}",
                            active_config_path.lock().unwrap()
                        );
                        *new_config.lock().unwrap() = Some(conf);
                    }
                    Err(err) => {
                        error!(
                            "Could not load config: {:?}, error: {}",
                            active_config_path.lock().unwrap(),
                            err
                        );
                    }
                }
            }
            thread::sleep(delay);
        }
        debug!("Config ready");
        let exitstatus = run(
            signal_reload.clone(),
            signal_exit.clone(),
            active_config.clone(),
            active_config_path.clone(),
            new_config.clone(),
            capture_status.clone(),
            playback_status.clone(),
        );
        match exitstatus {
            Err(e) => {
                *active_config.lock().unwrap() = None;
                error!("({}) {}", e.to_string(), e);
                if !wait {
                    break;
                }
            }
            Ok(ExitState::Exit) => {
                debug!("Exiting");
                *active_config.lock().unwrap() = None;
                break;
            }
            Ok(ExitState::Restart) => {
                *active_config.lock().unwrap() = None;
                debug!("Restarting with new config");
            }
        };
    }
}
