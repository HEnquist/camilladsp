#[cfg(target_os = "linux")]
extern crate alsa;
extern crate camillalib;
extern crate clap;
#[cfg(feature = "FFTW")]
extern crate fftw;
extern crate lazy_static;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
extern crate parking_lot;
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

extern crate flexi_logger;
extern crate time;
#[macro_use]
extern crate log;

use clap::{crate_authors, crate_description, crate_version, App, AppSettings, Arg};
use crossbeam_channel::select;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use flexi_logger::DeferredNow;
use log::Record;
#[cfg(not(windows))]
use signal_hook::consts::signal::*;
#[cfg(not(windows))]
use signal_hook::consts::TERM_SIGNALS;
#[cfg(not(windows))]
use signal_hook::iterator::{exfiltrator::SignalOnly, SignalsInfo};
use time::format_description;

use camillalib::Res;

use camillalib::audiodevice;
use camillalib::config;
use camillalib::countertimer;
use camillalib::processing;
#[cfg(feature = "websocket")]
use camillalib::socketserver;
use camillalib::statefile;
use camillalib::ControllerMessage;
#[cfg(feature = "websocket")]
use std::net::IpAddr;

use camillalib::{
    list_supported_devices, CaptureStatus, CommandMessage, ExitState, PlaybackStatus,
    ProcessingParameters, ProcessingState, ProcessingStatus, SharedConfigs, StatusMessage,
    StatusStructs, StopReason,
};

const EXIT_BAD_CONFIG: i32 = 101; // Error in config file
const EXIT_PROCESSING_ERROR: i32 = 102; // Error from processing
const EXIT_FORCED: i32 = 103; // Exit was forced by a second SIGINT
const EXIT_OK: i32 = 0; // All ok

// Time format string for logger
const TS_S: &str = "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]";
lazy_static::lazy_static! {
    static ref TS: Vec<format_description::FormatItem<'static>>
        = format_description::parse(TS_S).unwrap(/*ok*/);
}

// Customized version of `colored_opt_format` from flexi_logger.
fn custom_colored_logger_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    write!(
        w,
        "{} {:<5} [{}:{}] {}",
        now.now()
            .format(&TS)
            .unwrap_or_else(|_| "Timestamping failed".to_string()),
        flexi_logger::style(level).paint(level.to_string()),
        record.file().unwrap_or("<unnamed>"),
        record.line().unwrap_or(0),
        &record.args()
    )
}

// Customized version of `opt_format` from flexi_logger.
pub fn custom_logger_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    write!(
        w,
        "{} {:<5} [{}:{}] {}",
        now.now()
            .format(&TS)
            .unwrap_or_else(|_| "Timestamping failed".to_string()),
        record.level(),
        record.file().unwrap_or("<unnamed>"),
        record.line().unwrap_or(0),
        &record.args()
    )
}

fn run(
    shared_configs: SharedConfigs,
    status_structs: StatusStructs,
    rx_ctrl: crossbeam_channel::Receiver<ControllerMessage>,
) -> Res<ExitState> {
    let mut is_starting = true;
    let mut active_config = match shared_configs.active.lock().clone() {
        Some(cfg) => cfg,
        None => {
            error!("Tried to start without config!");
            return Ok(ExitState::Exit);
        }
    };
    let (tx_pb, rx_pb) = mpsc::sync_channel(active_config.devices.queuelimit());
    let (tx_cap, rx_cap) = mpsc::sync_channel(active_config.devices.queuelimit());

    let (tx_status, rx_status) = crossbeam_channel::unbounded();
    let tx_status_pb = tx_status.clone();
    let tx_status_cap = tx_status;

    let (tx_command_cap, rx_command_cap) = mpsc::channel();
    let (tx_pipeconf, rx_pipeconf) = mpsc::channel();

    let barrier = Arc::new(Barrier::new(4));
    let barrier_pb = barrier.clone();
    let barrier_cap = barrier.clone();
    let barrier_proc = barrier.clone();

    let conf_pb = active_config.clone();
    let conf_cap = active_config.clone();
    let conf_proc = active_config.clone();

    // Processing thread
    processing::run_processing(
        conf_proc,
        barrier_proc,
        tx_pb,
        rx_cap,
        rx_pipeconf,
        status_structs.processing,
    );

    // Playback thread
    let mut playback_dev = audiodevice::new_playback_device(conf_pb.devices);
    let pb_handle = playback_dev
        .start(rx_pb, barrier_pb, tx_status_pb, status_structs.playback)
        .unwrap();

    let used_channels = config::used_capture_channels(&active_config);
    debug!("Using channels {:?}", used_channels);
    {
        let mut capture_status = status_structs.capture.write();
        capture_status.state = ProcessingState::Starting;
        capture_status.used_channels = used_channels;
    }

    // Capture thread
    let mut capture_dev = audiodevice::new_capture_device(conf_cap.devices);
    let cap_handle = capture_dev
        .start(
            tx_cap,
            barrier_cap,
            tx_status_cap,
            rx_command_cap,
            status_structs.capture.clone(),
        )
        .unwrap();

    let mut pb_ready = false;
    let mut cap_ready = false;

    loop {
        // If startup procedure is not finished, do not process config change or exit
        let ctrl_ch = if is_starting {
            crossbeam_channel::never()
        } else {
            rx_ctrl.clone()
        };
        select! {
            recv(ctrl_ch) -> msg  => {
                match msg {
                    Ok(ControllerMessage::ConfigChanged(new_conf)) => {
                        if !ctrl_ch.is_empty() {
                            debug!("Dropping config change command since there are more commands in the queue");
                            continue;
                        }
                        let comp = config::config_diff(&active_config, &new_conf);
                        match comp {
                            config::ConfigChange::Pipeline
                            | config::ConfigChange::MixerParameters
                            | config::ConfigChange::FilterParameters { .. } => {
                                tx_pipeconf.send((comp, *new_conf.clone())).unwrap();
                                active_config = *new_conf;
                                *shared_configs.active.lock() = Some(active_config.clone());
                                let used_channels = config::used_capture_channels(&active_config);
                                debug!("Using channels {:?}", used_channels);
                                status_structs.capture.write().used_channels = used_channels;
                                debug!("Sent changes to pipeline");
                            }
                            config::ConfigChange::Devices => {
                                debug!("Devices changed, restart required.");
                                if tx_command_cap.send(CommandMessage::Exit).is_err() {
                                    debug!("Capture thread has already exited");
                                }
                                trace!("Wait for pb..");
                                pb_handle.join().unwrap();
                                trace!("Wait for cap..");
                                cap_handle.join().unwrap();
                                *shared_configs.active.lock() = Some(*new_conf);
                                trace!("All threads stopped, returning");
                                return Ok(ExitState::Restart);
                            }
                            config::ConfigChange::None => {
                                debug!("No changes in config.");
                            }
                        };
                    },
                    Ok(ControllerMessage::Stop) => {
                        debug!("Stop requested...");
                        if tx_command_cap.send(CommandMessage::Exit).is_err() {
                            debug!("Capture thread has already exited");
                        }
                        trace!("Wait for pb..");
                        pb_handle.join().unwrap();
                        trace!("Wait for cap..");
                        cap_handle.join().unwrap();
                        {
                            let mut active_cfg_shared = shared_configs.active.lock();
                            let mut prev_cfg_shared = shared_configs.previous.lock();
                            *active_cfg_shared = None;
                            *prev_cfg_shared = Some(active_config);
                        }
                        trace!("All threads stopped, stopping");
                        return Ok(ExitState::Restart);
                    },
                    Ok(ControllerMessage::Exit) => {
                        debug!("Exit requested...");
                        if tx_command_cap.send(CommandMessage::Exit).is_err() {
                            debug!("Capture thread has already exited");
                        }
                        trace!("Wait for pb..");
                        pb_handle.join().unwrap();
                        trace!("Wait for cap..");
                        cap_handle.join().unwrap();
                        *shared_configs.previous.lock() = Some(active_config);
                        trace!("All threads stopped, exiting");
                        return Ok(ExitState::Exit);
                    },
                    Err(err) => {
                        return Err(Box::new(err));
                    }
                }
            },
            recv(rx_status) -> msg => {
                match msg {
                    Ok(msg) => match msg {
                        StatusMessage::PlaybackReady => {
                            debug!("Playback thread ready to start");
                            pb_ready = true;
                            if cap_ready {
                                debug!("Both capture and playback ready, release barrier");
                                barrier.wait();
                                debug!("Supervisor loop starts now!");
                                is_starting = false;
                            }
                        }
                        StatusMessage::CaptureReady => {
                            debug!("Capture thread ready to start");
                            cap_ready = true;
                            if pb_ready {
                                debug!("Both capture and playback ready, release barrier");
                                barrier.wait();
                                debug!("Supervisor loop starts now!");
                                is_starting = false;
                                status_structs.status.write().stop_reason = StopReason::None;
                            }
                        }
                        StatusMessage::PlaybackError(message) => {
                            error!("Playback error: {}", message);
                            if tx_command_cap.send(CommandMessage::Exit).is_err() {
                                debug!("Capture thread has already exited");
                            }
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for capture thread to exit..");
                            status_structs.status.write().stop_reason = StopReason::PlaybackError(message);
                            cap_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureError(message) => {
                            error!("Capture error: {}", message);
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for playback thread to exit..");
                            status_structs.status.write().stop_reason = StopReason::CaptureError(message);
                            pb_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::PlaybackFormatChange(rate) => {
                            error!("Playback stopped due to external format change");
                            if tx_command_cap.send(CommandMessage::Exit).is_err() {
                                debug!("Capture thread has already exited");
                            }
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for capture thread to exit..");
                            status_structs.status.write().stop_reason =
                                StopReason::PlaybackFormatChange(rate);
                            cap_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureFormatChange(rate) => {
                            error!("Capture stopped due to external format change");
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for playback thread to exit..");
                            status_structs.status.write().stop_reason =
                                StopReason::CaptureFormatChange(rate);
                            pb_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::PlaybackDone => {
                            info!("Playback finished");
                            {
                                let stat = status_structs.status.upgradable_read();
                                if stat.stop_reason == StopReason::None {
                                    RwLockUpgradableReadGuard::upgrade(stat).stop_reason = StopReason::Done;
                                }
                            }
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureDone => {
                            info!("Capture finished");
                        }
                        StatusMessage::SetSpeed(speed) => {
                            debug!("SetSpeed message received");
                            if tx_command_cap
                                .send(CommandMessage::SetSpeed { speed })
                                .is_err()
                            {
                                debug!("Capture thread has already exited");
                            }
                        }
                    },
                    Err(err) => {
                        warn!("Capture, Playback and Processing threads have exited: {}", err);
                        status_structs.status.write().stop_reason = StopReason::UnknownError(
                            "Capture, Playback and Processing threads have exited".to_string(),
                        );
                        return Ok(ExitState::Restart);
                    }
                }
            }
        }
    }
}

fn main_process() -> i32 {
    let mut features = Vec::new();
    if cfg!(feature = "pulse-backend") {
        features.push("pulse-backend");
    }
    if cfg!(feature = "cpal-backend") {
        features.push("cpal-backend");
    }
    if cfg!(feature = "jack-backend") {
        features.push("jack-backend");
    }
    if cfg!(all(target_os = "linux", feature = "bluez-backend")) {
        features.push("bluez-backend");
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
    if cfg!(feature = "debug") {
        features.push("debug");
    }
    let featurelist = format!("Built with features: {}", features.join(", "));

    let (pb_types, cap_types) = list_supported_devices();
    let playback_types = format!("Playback: {}", pb_types.join(", "));
    let capture_types = format!("Capture: {}", cap_types.join(", "));

    let longabout = format!(
        "{}\n\n{}\n\nSupported device types:\n{}\n{}",
        crate_description!(),
        featurelist,
        capture_types,
        playback_types
    );

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
                .required_unless_one(&["wait", "statefile"]),
        )
        .arg(
            Arg::with_name("statefile")
                .help("Use the given file to persist the state")
                .short("s")
                .long("statefile")
                .takes_value(true)
                .display_order(2),
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
        )
        .arg(
            Arg::with_name("loglevel")
                .short("l")
                .long("loglevel")
                .display_order(100)
                .takes_value(true)
                .possible_value("trace")
                .possible_value("debug")
                .possible_value("info")
                .possible_value("warn")
                .possible_value("error")
                .possible_value("off")
                .help("Set log level")
                .conflicts_with("verbosity"),
        )
        .arg(
            Arg::with_name("logfile")
                .short("o")
                .long("logfile")
                .display_order(100)
                .takes_value(true)
                .help("Write logs to file"),
        )
        .arg(
            Arg::with_name("gain")
                .help("Set initial gain in dB for Volume and Loudness filters")
                .short("g")
                .long("gain")
                .display_order(200)
                .takes_value(true)
                .validator(|v: String| -> Result<(), String> {
                    if let Ok(gain) = v.parse::<f32>() {
                        if (-120.0..=20.0).contains(&gain) {
                            return Ok(());
                        }
                    }
                    Err(String::from("Must be a number between -120 and +20"))
                }),
        )
        .arg(
            Arg::with_name("mute")
                .help("Start with Volume and Loudness filters muted")
                .short("m")
                .long("mute")
                .display_order(200),
        )
        .arg(
            Arg::with_name("samplerate")
                .help("Override samplerate in config")
                .short("r")
                .long("samplerate")
                .display_order(300)
                .takes_value(true)
                .validator(|v: String| -> Result<(), String> {
                    if let Ok(rate) = v.parse::<usize>() {
                        if rate > 0 {
                            return Ok(());
                        }
                    }
                    Err(String::from("Must be an integer > 0"))
                }),
        )
        .arg(
            Arg::with_name("channels")
                .help("Override number of channels of capture device in config")
                .short("n")
                .long("channels")
                .display_order(300)
                .takes_value(true)
                .validator(|v: String| -> Result<(), String> {
                    if let Ok(rate) = v.parse::<usize>() {
                        if rate > 0 {
                            return Ok(());
                        }
                    }
                    Err(String::from("Must be an integer > 0"))
                }),
        )
        .arg(
            Arg::with_name("extra_samples")
                .help("Override number of extra samples in config")
                .short("e")
                .long("extra_samples")
                .display_order(300)
                .takes_value(true)
                .validator(|v: String| -> Result<(), String> {
                    if let Ok(_samples) = v.parse::<usize>() {
                        return Ok(());
                    }
                    Err(String::from("Must be an integer > 0"))
                }),
        )
        .arg(
            Arg::with_name("format")
                .short("f")
                .long("format")
                .display_order(310)
                .takes_value(true)
                .possible_value("S16LE")
                .possible_value("S24LE")
                .possible_value("S24LE3")
                .possible_value("S32LE")
                .possible_value("FLOAT32LE")
                .possible_value("FLOAT64LE")
                .help("Override sample format of capture device in config"),
        );
    #[cfg(feature = "websocket")]
    let clapapp = clapapp
        .arg(
            Arg::with_name("port")
                .help("Port for websocket server")
                .short("p")
                .long("port")
                .display_order(200)
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
                .display_order(200)
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

    let mut loglevel = match matches.occurrences_of("verbosity") {
        0 => "info",
        1 => "debug",
        2 => "trace",
        _ => "trace",
    };

    if let Some(level) = matches.value_of("loglevel") {
        loglevel = level;
    }

    let logger = if let Some(logfile) = matches.value_of("logfile") {
        let mut path = PathBuf::from(logfile);
        if !path.is_absolute() {
            let mut fullpath = std::env::current_dir().unwrap();
            fullpath.push(path);
            path = fullpath;
        }
        flexi_logger::Logger::try_with_str(loglevel)
            .unwrap()
            .format(custom_logger_format)
            .log_to_file(flexi_logger::FileSpec::try_from(path).unwrap())
            .write_mode(flexi_logger::WriteMode::Async)
            .start()
            .unwrap()
    } else {
        flexi_logger::Logger::try_with_str(loglevel)
            .unwrap()
            .format(custom_colored_logger_format)
            .set_palette("196;208;-;27;8".to_string())
            .log_to_stderr()
            .write_mode(flexi_logger::WriteMode::Async)
            .start()
            .unwrap()
    };
    info!("CamillaDSP version {}", crate_version!());
    info!(
        "Running on {}, {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    // logging examples
    //trace!("trace message"); //with -vv
    //debug!("debug message"); //with -v
    //info!("info message");
    //warn!("warn message");
    //error!("error message");

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let _signal = unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGHUP, || debug!("Received SIGHUP"))
    };

    #[cfg(target_os = "windows")]
    wasapi::initialize_mta().unwrap();

    let mut configname = matches.value_of("configfile").map(|path| path.to_string());

    {
        let mut overrides = config::OVERRIDES.write();
        overrides.samplerate = matches
            .value_of("samplerate")
            .map(|s| s.parse::<usize>().unwrap());
        overrides.extra_samples = matches
            .value_of("extra_samples")
            .map(|s| s.parse::<usize>().unwrap());
        overrides.channels = matches
            .value_of("channels")
            .map(|s| s.parse::<usize>().unwrap());
        overrides.sample_format = matches
            .value_of("format")
            .map(|s| config::SampleFormat::from_name(s).unwrap());
    }

    let statefilename = matches.value_of("statefile").map(|path| path.to_string());
    let state = if let Some(filename) = &statefilename {
        statefile::load_state(filename)
    } else {
        None
    };
    debug!("Loaded state: {state:?}");

    let initial_volumes =
        if let Some(v) = matches.value_of("gain").map(|s| s.parse::<f32>().unwrap()) {
            debug!("Using command line argument for initial volume");
            [v, v, v, v, v]
        } else if let Some(s) = &state {
            debug!("Using statefile for initial volume");
            s.volume
        } else {
            debug!("Using default initial volume");
            [
                ProcessingParameters::DEFAULT_VOLUME,
                ProcessingParameters::DEFAULT_VOLUME,
                ProcessingParameters::DEFAULT_VOLUME,
                ProcessingParameters::DEFAULT_VOLUME,
                ProcessingParameters::DEFAULT_VOLUME,
            ]
        };

    let initial_mutes = if matches.is_present("mute") {
        debug!("Using command line argument for initial mute");
        [true, true, true, true, true]
    } else if let Some(s) = &state {
        debug!("Using statefile for initial mute");
        s.mute
    } else {
        debug!("Using default initial mute");
        [
            ProcessingParameters::DEFAULT_MUTE,
            ProcessingParameters::DEFAULT_MUTE,
            ProcessingParameters::DEFAULT_MUTE,
            ProcessingParameters::DEFAULT_MUTE,
            ProcessingParameters::DEFAULT_MUTE,
        ]
    };

    debug!("Initial mute: {initial_mutes:?}");
    debug!("Initial volume: {initial_volumes:?}");

    debug!("Read config file {:?}", configname);

    if matches.is_present("check") {
        match config::load_validate_config(&configname.unwrap()) {
            Ok(_) => {
                println!("Config is valid");
                return EXIT_OK;
            }
            Err(err) => {
                println!("Config is not valid");
                println!("{err}");
                return EXIT_BAD_CONFIG;
            }
        }
    }

    if configname.is_none() {
        if let Some(s) = &state {
            configname = s.config_path.clone();
        }
    }

    let (tx_command, rx_command) = crossbeam_channel::bounded(10);
    if let Some(path) = &configname {
        match config::load_validate_config(path) {
            Ok(conf) => {
                debug!("Config is valid");
                tx_command
                    .send(ControllerMessage::ConfigChanged(Box::new(conf)))
                    .unwrap();
            }
            Err(err) => {
                error!("{}", err);
                debug!("Exiting due to config error");
                return EXIT_BAD_CONFIG;
            }
        }
    }

    let active_config_path = Arc::new(Mutex::new(configname));

    let tx_command_thread = tx_command.clone();

    #[cfg(not(windows))]
    let active_path_thread = active_config_path.clone();

    #[cfg(not(windows))]
    thread::spawn(move || {
        let mut sigs = vec![SIGHUP, SIGUSR1];
        sigs.extend(TERM_SIGNALS);
        let mut signals = SignalsInfo::<SignalOnly>::new(&sigs).unwrap();
        let mut exit_requested = false;
        for info in &mut signals {
            debug!("Received signal: {}", info);
            match info {
                SIGHUP => {
                    let path = (*active_path_thread.lock()).clone();
                    if let Some(path) = path {
                        match config::load_validate_config(path.as_str()) {
                            Ok(conf) => {
                                debug!("Config is valid");
                                if let Err(e) = tx_command_thread
                                    .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                                {
                                    error!("Error sending reload message: {}", e);
                                }
                            }
                            Err(err) => {
                                error!("Config error during reload: {}", err);
                            }
                        };
                    } else {
                        error!("Config path not specified, cannot reload");
                    }
                }
                SIGUSR1 => {
                    if let Err(e) = tx_command_thread.try_send(ControllerMessage::Stop) {
                        error!("Error sending stop message: {}", e);
                    }
                }
                _ => {
                    if exit_requested {
                        warn!("Forcing a shutdown");
                        logger.flush();
                        std::process::exit(EXIT_FORCED);
                    }
                    info!("Shutting down");
                    exit_requested = true;
                    if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                        error!("Error sending exit message: {}", e);
                    }
                }
            };
        }
    });

    #[cfg(windows)]
    thread::spawn(move || {
        // On windows we don't have signal_hook::iterator, so we just poll for signal...
        const DELAY: Duration = Duration::from_millis(100);
        let signal_exit = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&signal_exit)).unwrap();
        let mut exit_requested = false;
        loop {
            if signal_exit.load(std::sync::atomic::Ordering::Relaxed) {
                signal_exit.store(false, std::sync::atomic::Ordering::Relaxed);
                if exit_requested {
                    warn!("Forcing a shutdown");
                    logger.flush();
                    std::process::exit(EXIT_FORCED);
                }
                info!("Shutting down");
                exit_requested = true;
                if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                    error!("Error sending exit message: {}", e);
                }
            }
            thread::sleep(DELAY);
        }
    });

    let wait = matches.is_present("wait");

    let capture_status = Arc::new(RwLock::new(CaptureStatus {
        measured_samplerate: 0,
        update_interval: 1000,
        signal_range: 0.0,
        rate_adjust: 0.0,
        state: ProcessingState::Inactive,
        signal_rms: countertimer::ValueHistory::new(1024, 2),
        signal_peak: countertimer::ValueHistory::new(1024, 2),
        used_channels: Vec::new(),
    }));
    let playback_status = Arc::new(RwLock::new(PlaybackStatus {
        buffer_level: 0,
        clipped_samples: 0,
        update_interval: 1000,
        signal_rms: countertimer::ValueHistory::new(1024, 2),
        signal_peak: countertimer::ValueHistory::new(1024, 2),
    }));
    let processing_params = Arc::new(ProcessingParameters::new(&initial_volumes, &initial_mutes));
    let processing_status = Arc::new(RwLock::new(ProcessingStatus {
        stop_reason: StopReason::None,
    }));

    let status_structs = StatusStructs {
        capture: capture_status.clone(),
        playback: playback_status.clone(),
        processing: processing_params.clone(),
        status: processing_status.clone(),
    };
    let active_config = Arc::new(Mutex::new(None));
    let previous_config = Arc::new(Mutex::new(None));

    #[cfg(feature = "websocket")]
    {
        let (tx_state, rx_state) = mpsc::sync_channel(1);

        let processing_params_clone = processing_params.clone();
        let active_config_path_clone = active_config_path.clone();
        let unsaved_state_changes = Arc::new(AtomicBool::new(false));

        if let Some(port_str) = matches.value_of("port") {
            let serveraddress = matches.value_of("address").unwrap_or("127.0.0.1");
            let serverport = port_str.parse::<usize>().unwrap();

            // Send one state change to trigger an initial save
            tx_state.try_send(()).unwrap_or(());

            let shared_data = socketserver::SharedData {
                active_config: active_config.clone(),
                active_config_path,
                previous_config: previous_config.clone(),
                command_sender: tx_command,
                capture_status,
                playback_status,
                processing_params,
                processing_status,
                state_change_notify: tx_state,
                state_file_path: statefilename.clone(),
                unsaved_state_change: unsaved_state_changes.clone(),
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

        if let Some(fname) = &statefilename {
            let fname = fname.clone();

            thread::spawn(move || loop {
                thread::sleep(Duration::from_millis(1000));
                match rx_state.recv() {
                    Ok(()) => {
                        debug!("saving state to {}", &fname);
                        statefile::save_state(
                            &fname,
                            &active_config_path_clone,
                            &processing_params_clone,
                            &unsaved_state_changes,
                        );
                    }
                    Err(_) => break,
                }
            });
        }
    }

    loop {
        debug!("Wait for config");
        loop {
            let has_config = (*active_config.lock()).is_some();
            let has_commands = !rx_command.is_empty();
            if has_config && !has_commands {
                debug!("New config is available and there are no queued commands, continuing");
                break;
            }
            if !wait && !has_commands {
                if !has_config {
                    debug!("Wait mode is disabled, there are no queued commands, and no new config. Exiting.");
                    return EXIT_OK;
                }
                debug!("Wait mode is disabled and there are no queued commands, continuing");
                break;
            }
            debug!("Waiting to receive a command");
            match rx_command.recv() {
                Ok(ControllerMessage::ConfigChanged(new_conf)) => {
                    debug!("Config change command received");
                    *active_config.lock() = Some(*new_conf);
                }
                Ok(ControllerMessage::Stop) => {
                    debug!("Stop command received");
                    *active_config.lock() = None;
                }
                Ok(ControllerMessage::Exit) => {
                    debug!("Exit command received");
                    return EXIT_OK;
                }
                Err(e) => {
                    warn!("Error recv from cmd queue {}", e);
                    return EXIT_OK;
                }
            }
        }

        let shared_configs = SharedConfigs {
            active: active_config.clone(),
            previous: previous_config.clone(),
        };

        debug!("Config ready, start processing");
        let exitstatus = run(shared_configs, status_structs.clone(), rx_command.clone());
        debug!("Processing ended with status {:?}", exitstatus);

        match exitstatus {
            Err(e) => {
                error!("({}) {}", e.to_string(), e);
                if !wait {
                    return EXIT_PROCESSING_ERROR;
                }
            }
            Ok(ExitState::Exit) => {
                debug!("Exiting");
                return EXIT_OK;
            }
            Ok(ExitState::Restart) => {
                debug!("Restarting with new config");
            }
        };
    }
}

fn main() {
    std::process::exit(main_process());
}
