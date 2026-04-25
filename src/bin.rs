// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

extern crate camillalib;
extern crate chrono;
extern crate clap;
extern crate flexi_logger;
#[macro_use]
extern crate log;

use clap::{Arg, ArgAction, Command, crate_authors, crate_description, crate_name, crate_version};
use git_version::git_version;
#[cfg(feature = "websocket")]
use std::net::IpAddr;
use std::path::PathBuf;

use flexi_logger::DeferredNow;
use log::Record;

use camillalib::config;
use camillalib::engine::{EXIT_BAD_CONFIG, EXIT_OK, EngineConfig, run_engine};
use camillalib::statefile;
use camillalib::{ProcessingParameters, list_supported_devices};

const GIT_HASH: &str = git_version!(fallback = "unknown");

// Customized version of `colored_opt_format` from flexi_logger.
fn custom_colored_logger_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    write!(
        w,
        "{} {:<5} [{}] <{}:{}> {}",
        now.now().format("%Y-%m-%d %H:%M:%S%.6f"),
        flexi_logger::style(level).paint(level.to_string()),
        record.module_path().unwrap_or("*unknown module*"),
        record.file().unwrap_or("*unknown file*"),
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
        "{} {:<5} [{}] <{}:{}> {}",
        now.now().format("%Y-%m-%d %H:%M:%S%.6f"),
        record.level(),
        record.module_path().unwrap_or("*unknown module*"),
        record.file().unwrap_or("*unknown file*"),
        record.line().unwrap_or(0),
        &record.args()
    )
}

fn parse_gain_value(v: &str) -> Result<f32, String> {
    if let Ok(gain) = v.parse::<f32>()
        && (-120.0..=20.0).contains(&gain)
    {
        return Ok(gain);
    }
    Err(String::from("Must be a number between -120 and +20"))
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

    let license_notice = if cfg!(feature = "asio-backend") {
        "License: GPLv3 only (built with ASIO backend)".to_string()
    } else {
        "License: GPLv3 or MPL-2.0".to_string()
    };

    let version_with_hash: &'static str =
        Box::leak(format!("{} ({})", crate_version!(), GIT_HASH).into_boxed_str());

    let longabout = format!(
        "{} v{} ({})\n{}\n{}\n\n{}\n\n{}\n\nSupported device types:\n{}\n{}",
        crate_name!(),
        crate_version!(),
        GIT_HASH,
        crate_authors!(),
        crate_description!(),
        license_notice,
        featurelist,
        capture_types,
        playback_types
    );

    let clapapp = Command::new("CamillaDSP")
        .version(version_with_hash)
        .about(longabout)
        .author(crate_authors!())
        //.setting(AppSettings::ArgRequiredElseHelp)
        .arg(
            Arg::new("configfile")
                .help("The configuration file to use")
                .index(1)
                .value_name("CONFIGFILE")
                .action(ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_unless_present_any(["wait", "statefile"]),
        )
        .arg(
            Arg::new("statefile")
                .help("Use the given file to persist the state")
                .short('s')
                .long("statefile")
                .value_name("STATEFILE")
                .action(ArgAction::Set)
                .display_order(2)
                .value_parser(clap::builder::NonEmptyStringValueParser::new()),
        )
        .arg(
            Arg::new("check")
                .help("Check config file and exit")
                .short('c')
                .long("check")
                .display_order(4)
                .requires("configfile")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbosity")
                .help("Increase message verbosity")
                .short('v')
                .display_order(100)
                .action(ArgAction::Count),
        )
        .arg(
            Arg::new("loglevel")
                .help("Set log level")
                .short('l')
                .long("loglevel")
                .value_name("LOGLEVEL")
                .display_order(101)
                .conflicts_with("verbosity")
                .action(ArgAction::Set)
                .value_parser(["trace", "debug", "info", "warn", "error", "off"]),
        )
        .arg(
            Arg::new("logfile")
                .help("Write logs to the given file path")
                .short('o')
                .long("logfile")
                .value_name("LOGFILE")
                .display_order(102)
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("log_rotate_size")
                .help("Rotate log file when the size in bytes exceeds this value")
                .long("log_rotate_size")
                .value_name("ROTATE_SIZE")
                .display_order(103)
                .requires("logfile")
                .value_parser(clap::value_parser!(u32).range(1000..))
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("log_keep_nbr")
                .help("Number of previous log files to keep")
                .long("log_keep_nbr")
                .value_name("KEEP_NBR")
                .display_order(104)
                .requires("log_rotate_size")
                .value_parser(clap::value_parser!(u32))
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("custom_log_spec")
                .help("Custom logger specification")
                .long("custom_log_spec")
                .value_name("LOG_SPEC")
                .display_order(105)
                .value_parser(clap::value_parser!(String))
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("gain")
                .help("Initial gain in dB for main volume control")
                .short('g')
                .long("gain")
                .value_name("GAIN")
                .display_order(300)
                .action(ArgAction::Set)
                .value_parser(parse_gain_value),
        )
        .arg(
            Arg::new("gain1")
                .help("Initial gain in dB for Aux1 fader")
                .long("gain1")
                .value_name("GAIN1")
                .display_order(301)
                .action(ArgAction::Set)
                .value_parser(parse_gain_value),
        )
        .arg(
            Arg::new("gain2")
                .help("Initial gain in dB for Aux2 fader")
                .long("gain2")
                .value_name("GAIN2")
                .display_order(302)
                .action(ArgAction::Set)
                .value_parser(parse_gain_value),
        )
        .arg(
            Arg::new("gain3")
                .help("Initial gain in dB for Aux3 fader")
                .long("gain3")
                .value_name("GAIN3")
                .display_order(303)
                .action(ArgAction::Set)
                .value_parser(parse_gain_value),
        )
        .arg(
            Arg::new("gain4")
                .help("Initial gain in dB for Aux4 fader")
                .long("gain4")
                .value_name("GAIN4")
                .display_order(304)
                .action(ArgAction::Set)
                .value_parser(parse_gain_value),
        )
        .arg(
            Arg::new("mute")
                .help("Start with main volume control muted")
                .short('m')
                .long("mute")
                .display_order(310)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mute1")
                .help("Start with Aux1 fader muted")
                .long("mute1")
                .display_order(311)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mute2")
                .help("Start with Aux2 fader muted")
                .long("mute2")
                .display_order(312)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mute3")
                .help("Start with Aux3 fader muted")
                .long("mute3")
                .display_order(313)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("mute4")
                .help("Start with Aux4 fader muted")
                .long("mute4")
                .display_order(314)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("samplerate")
                .help("Override samplerate in config")
                .short('r')
                .long("samplerate")
                .value_name("SAMPLERATE")
                .display_order(400)
                .action(ArgAction::Set)
                .value_parser(clap::builder::RangedU64ValueParser::<usize>::new().range(1..)),
        )
        .arg(
            Arg::new("channels")
                .help("Override number of channels of capture device in config")
                .short('n')
                .long("channels")
                .value_name("CHANNELS")
                .display_order(400)
                .action(ArgAction::Set)
                .value_parser(clap::builder::RangedU64ValueParser::<usize>::new().range(1..)),
        )
        .arg(
            Arg::new("extra_samples")
                .help("Override number of extra samples in config")
                .short('e')
                .long("extra_samples")
                .value_name("EXTRA_SAMPLES")
                .display_order(400)
                .action(ArgAction::Set)
                .value_parser(clap::builder::RangedU64ValueParser::<usize>::new().range(1..)),
        )
        .arg(
            Arg::new("format")
                .short('f')
                .long("format")
                .value_name("FORMAT")
                .display_order(410)
                .action(ArgAction::Set)
                .value_parser([
                    "S16_LE",
                    "S24_3_LE",
                    "S24_4_LJ_LE",
                    "S24_4_RJ_LE",
                    "S32_LE",
                    "F32_LE",
                    "F64_LE",
                ])
                .help("Override sample format of capture device in config"),
        );
    #[cfg(feature = "websocket")]
    let clapapp = clapapp
        .arg(
            Arg::new("port")
                .help("Port for websocket server")
                .short('p')
                .long("port")
                .value_name("PORT")
                .display_order(200)
                .action(ArgAction::Set)
                .value_parser(clap::builder::RangedU64ValueParser::<usize>::new().range(0..65535)),
        )
        .arg(
            Arg::new("address")
                .help("IP address to bind websocket server to")
                .short('a')
                .long("address")
                .value_name("ADDRESS")
                .display_order(200)
                .action(ArgAction::Set)
                .requires("port")
                .value_parser(|val: &str| -> Result<String, String> {
                    if val.parse::<IpAddr>().is_ok() {
                        return Ok(val.to_string());
                    }
                    Err(String::from("Must be a valid IP address"))
                }),
        )
        .arg(
            Arg::new("wait")
                .short('w')
                .long("wait")
                .display_order(200)
                .help("Wait for config from websocket")
                .requires("port")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no_config")
                .long("no_config")
                .display_order(299)
                .help("Ignore config file in statefile and start without")
                .requires("wait")
                .requires("statefile")
                .conflicts_with("configfile")
                .action(ArgAction::SetTrue),
        );
    #[cfg(feature = "secure-websocket")]
    let clapapp = clapapp
        .arg(
            Arg::new("cert")
                .help("Path to .pfx/.p12 certificate file")
                .long("cert")
                .value_name("CERT")
                .display_order(220)
                .action(ArgAction::Set)
                .requires("port"),
        )
        .arg(
            Arg::new("pass")
                .help("Password for .pfx/.p12 certificate file")
                .long("pass")
                .value_name("PASS")
                .display_order(220)
                .action(ArgAction::Set)
                .requires("port"),
        );
    let matches = clapapp.get_matches();

    let mut loglevel = match matches.get_count("verbosity") {
        0 => "info",
        1 => "debug",
        2 => "trace",
        _ => "trace",
    };

    if let Some(level) = matches.get_one::<String>("loglevel") {
        loglevel = level;
    }
    if let Some(spec) = matches.get_one::<String>("custom_log_spec") {
        loglevel = spec;
    }

    let logger = if let Some(logfile) = matches.get_one::<String>("logfile") {
        let mut path = PathBuf::from(logfile);
        if !path.is_absolute() {
            let mut fullpath = std::env::current_dir().unwrap();
            fullpath.push(path);
            path = fullpath;
        }
        let mut logger = flexi_logger::Logger::try_with_str(loglevel)
            .expect("The provided logger specification is invalid")
            .format(custom_logger_format)
            .log_to_file(flexi_logger::FileSpec::try_from(path).unwrap())
            .write_mode(flexi_logger::WriteMode::Async);

        let cleanup = if let Some(keep_nbr) = matches.get_one::<u32>("log_keep_nbr") {
            flexi_logger::Cleanup::KeepLogFiles(*keep_nbr as usize)
        } else {
            flexi_logger::Cleanup::Never
        };

        if let Some(rotate_size) = matches.get_one::<u32>("log_rotate_size") {
            logger = logger.rotate(
                flexi_logger::Criterion::Size(*rotate_size as u64),
                flexi_logger::Naming::Timestamps,
                cleanup,
            );
        }

        logger.start().unwrap()
    } else {
        flexi_logger::Logger::try_with_str(loglevel)
            .expect("The provided logger specification is invalid")
            .format(custom_colored_logger_format)
            .set_palette("196;208;-;27;8".to_string())
            .log_to_stderr()
            .write_mode(flexi_logger::WriteMode::Async)
            .start()
            .unwrap()
    };
    info!("CamillaDSP version {} ({})", crate_version!(), GIT_HASH);
    info!(
        "Running on {}, {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    {
        let mut overrides = config::OVERRIDES.write();
        overrides.samplerate = matches.get_one::<usize>("samplerate").copied();
        overrides.extra_samples = matches.get_one::<usize>("extra_samples").copied();
        overrides.channels = matches.get_one::<usize>("channels").copied();
        overrides.sample_format = matches
            .get_one::<String>("format")
            .map(|s| config::BinarySampleFormat::from_name(s).unwrap());
    }

    let statefilename: Option<String> = matches.get_one::<String>("statefile").cloned();
    let state = if let Some(filename) = &statefilename {
        statefile::load_state(filename)
    } else {
        None
    };
    debug!("Loaded state: {state:?}");

    let mut initial_volumes = if let Some(s) = &state {
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
    if let Some(v) = matches.get_one::<f32>("gain") {
        debug!("Using command line argument for initial main volume");
        initial_volumes[0] = *v;
    }
    if let Some(v) = matches.get_one::<f32>("gain1") {
        debug!("Using command line argument for initial Aux1 volume");
        initial_volumes[1] = *v;
    }
    if let Some(v) = matches.get_one::<f32>("gain2") {
        debug!("Using command line argument for initial Aux2 volume");
        initial_volumes[2] = *v;
    }
    if let Some(v) = matches.get_one::<f32>("gain3") {
        debug!("Using command line argument for initial Aux3 volume");
        initial_volumes[3] = *v;
    }
    if let Some(v) = matches.get_one::<f32>("gain4") {
        debug!("Using command line argument for initial Aux4 volume");
        initial_volumes[4] = *v;
    }

    let mut initial_mutes = if let Some(s) = &state {
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
    if matches.get_flag("mute") {
        debug!("Using command line argument for initial main mute");
        initial_mutes[0] = true;
    }
    if matches.get_flag("mute1") {
        debug!("Using command line argument for initial Aux1 mute");
        initial_mutes[1] = true;
    }
    if matches.get_flag("mute2") {
        debug!("Using command line argument for initial Aux2 mute");
        initial_mutes[2] = true;
    }
    if matches.get_flag("mute3") {
        debug!("Using command line argument for initial Aux3 mute");
        initial_mutes[3] = true;
    }
    if matches.get_flag("mute4") {
        debug!("Using command line argument for initial Aux4 mute");
        initial_mutes[4] = true;
    }

    debug!("Initial mute: {initial_mutes:?}");
    debug!("Initial volume: {initial_volumes:?}");

    let mut configname = matches.get_one::<String>("configfile").cloned();
    debug!("Read config file {configname:?}");

    if matches.get_flag("check") {
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

    if configname.is_none()
        && let Some(s) = &state
    {
        if matches.get_flag("no_config") {
            debug!("Ignoring config from statefile as per command line argument");
        } else {
            debug!("Using config from statefile");
            configname.clone_from(&s.config_path);
        }
    }

    // All state variables are prepared, save to the statefile if needed
    if let Some(fname) = &statefilename {
        let state_to_save = statefile::State {
            config_path: configname.clone(),
            volume: initial_volumes,
            mute: initial_mutes,
        };
        if state.is_none() || state.map(|s| s != state_to_save).unwrap_or(false) {
            statefile::save_state_to_file(fname, &state_to_save);
        } else {
            debug!("No change to state from {fname}, not overwriting.");
        }
    }

    let engine_config = EngineConfig {
        configname,
        statefilename,
        initial_volumes,
        initial_mutes,
        wait: matches.get_flag("wait"),
        #[cfg(feature = "websocket")]
        ws_port: matches.get_one::<usize>("port").copied(),
        #[cfg(feature = "websocket")]
        ws_address: matches
            .get_one::<String>("address")
            .cloned()
            .unwrap_or("127.0.0.1".to_string()),
        #[cfg(feature = "secure-websocket")]
        ws_cert: matches.get_one::<String>("cert").cloned(),
        #[cfg(feature = "secure-websocket")]
        ws_pass: matches.get_one::<String>("pass").cloned(),
    };

    run_engine(engine_config, logger)
}

fn main() {
    std::process::exit(main_process());
}
