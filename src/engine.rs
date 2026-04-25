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

use crossbeam_channel::select;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
#[cfg(any(windows, feature = "websocket"))]
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Barrier};
use std::thread;
#[cfg(any(windows, feature = "websocket"))]
use std::time::Duration;

#[cfg(not(windows))]
use signal_hook::consts::TERM_SIGNALS;
#[cfg(not(windows))]
use signal_hook::consts::signal::*;
#[cfg(not(windows))]
use signal_hook::iterator::{SignalsInfo, exfiltrator::SignalOnly};

use crate::utils::countertimer;
#[cfg(feature = "websocket")]
use crate::websocket_server;
use crate::{
    CaptureStatus, CommandMessage, ControllerMessage, ExitState, PlaybackStatus,
    ProcessingParameters, ProcessingState, ProcessingStatus, SHUTDOWN_REQUESTED, SharedConfigs,
    StatusMessage, StatusStructs, StopReason,
};
use crate::{audiodevice, config, processing, statefile};

pub const EXIT_OK: i32 = 0;
pub const EXIT_BAD_CONFIG: i32 = 101;
pub const EXIT_PROCESSING_ERROR: i32 = 102;
pub const EXIT_FORCED: i32 = 103;

pub struct EngineConfig {
    pub configname: Option<String>,
    pub statefilename: Option<String>,
    pub initial_volumes: [f32; 5],
    pub initial_mutes: [bool; 5],
    pub wait: bool,
    #[cfg(feature = "websocket")]
    pub ws_port: Option<usize>,
    #[cfg(feature = "websocket")]
    pub ws_address: String,
    #[cfg(feature = "secure-websocket")]
    pub ws_cert: Option<String>,
    #[cfg(feature = "secure-websocket")]
    pub ws_pass: Option<String>,
}

fn run(
    shared_configs: SharedConfigs,
    status_structs: StatusStructs,
    rx_ctrl: crossbeam_channel::Receiver<ControllerMessage>,
) -> crate::Res<ExitState> {
    let mut is_starting = true;
    let mut active_config = match shared_configs.active.lock().clone() {
        Some(cfg) => cfg,
        None => {
            error!("Tried to start without config!");
            return Ok(ExitState::Exit);
        }
    };
    let (tx_pb, rx_pb) = crossbeam_channel::bounded(active_config.devices.queuelimit());
    let (tx_cap, rx_cap) = crossbeam_channel::bounded(active_config.devices.queuelimit());

    let (tx_status, rx_status) = crossbeam_channel::unbounded();
    let tx_status_pb = tx_status.clone();
    let tx_status_cap = tx_status;

    let (tx_command_cap, rx_command_cap) = crossbeam_channel::unbounded();
    let (tx_pipeconf, rx_pipeconf) = crossbeam_channel::unbounded();

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
        status_structs.processing.clone(),
    );

    // Playback thread
    let mut playback_dev = audiodevice::new_playback_device(conf_pb.devices);
    let pb_handle = playback_dev
        .start(rx_pb, barrier_pb, tx_status_pb, status_structs.playback)
        .unwrap();

    let used_channels = config::used_capture_channels(&active_config);
    debug!("Using channels {used_channels:?}");
    {
        let mut capture_status = status_structs.capture.write();
        crate::update_capture_state(&mut capture_status, ProcessingState::Starting);
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
            status_structs.processing.clone(),
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
                        status_structs.processing.set_processing_load(0.0);
                        status_structs.processing.set_resampler_load(0.0);
                        let comp = config::config_diff(&active_config, &new_conf);
                        match comp {
                            config::ConfigChange::Pipeline
                            | config::ConfigChange::MixerParameters
                            | config::ConfigChange::FilterParameters { .. } => {
                                tx_pipeconf.send((comp, *new_conf.clone())).unwrap();
                                active_config = *new_conf;
                                *shared_configs.active.lock() = Some(active_config.clone());
                                let used_channels = config::used_capture_channels(&active_config);
                                debug!("Using channels {used_channels:?}");
                                status_structs.capture.write().used_channels = used_channels;
                                debug!("Sent changes to pipeline");
                            }
                            config::ConfigChange::Devices => {
                                debug!("Devices changed, restart required.");
                                if tx_command_cap.send(CommandMessage::Exit).is_err() {
                                    debug!("Capture thread has already exited");
                                }
                                trace!("Wait for playback thread to exit..");
                                pb_handle.join().unwrap();
                                trace!("Wait for capture thread to exit..");
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
                        trace!("Wait for playback thread to exit..");
                        pb_handle.join().unwrap();
                        trace!("Wait for capture thread to exit..");
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
                        trace!("Wait for playback thread to exit..");
                        pb_handle.join().unwrap();
                        trace!("Wait for capture thread to exit..");
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
                                crate::set_stop_reason(
                                    &status_structs.status,
                                    StopReason::None,
                                );
                            }
                        }
                        StatusMessage::PlaybackError(message) => {
                            error!("Playback error: {message}");
                            if tx_command_cap.send(CommandMessage::Exit).is_err() {
                                debug!("Capture thread has already exited");
                            }
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for capture thread to exit..");
                            crate::set_stop_reason(
                                &status_structs.status,
                                StopReason::PlaybackError(message),
                            );
                            cap_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            crate::set_capture_state(
                                &status_structs.capture,
                                ProcessingState::Inactive,
                            );
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureError(message) => {
                            error!("Capture error: {message}");
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            debug!("Wait for playback thread to exit..");
                            crate::set_stop_reason(
                                &status_structs.status,
                                StopReason::CaptureError(message),
                            );
                            pb_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            crate::set_capture_state(
                                &status_structs.capture,
                                ProcessingState::Inactive,
                            );
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
                            crate::set_stop_reason(
                                &status_structs.status,
                                StopReason::PlaybackFormatChange(rate),
                            );
                            cap_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            crate::set_capture_state(
                                &status_structs.capture,
                                ProcessingState::Inactive,
                            );
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureFormatChange(rate) => {
                            error!("Capture stopped due to external format change");
                            if is_starting {
                                debug!("Error while starting, release barrier");
                                barrier.wait();
                            }
                            crate::set_stop_reason(
                                &status_structs.status,
                                StopReason::CaptureFormatChange(rate),
                            );
                            debug!("Wait for playback thread to exit..");
                            pb_handle.join().unwrap();
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            crate::set_capture_state(
                                &status_structs.capture,
                                ProcessingState::Inactive,
                            );
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::PlaybackDone => {
                            info!("Playback finished");
                            {
                                let stat = status_structs.status.upgradable_read();
                                if stat.stop_reason == StopReason::None {
                                    crate::update_stop_reason(
                                        &mut RwLockUpgradableReadGuard::upgrade(stat),
                                        StopReason::Done,
                                    );
                                }
                            }
                            {
                                let mut active_cfg_shared = shared_configs.active.lock();
                                let mut prev_cfg_shared = shared_configs.previous.lock();
                                *active_cfg_shared = None;
                                *prev_cfg_shared = Some(active_config);
                            }
                            trace!("Wait for playback thread to exit..");
                            pb_handle.join().unwrap();
                            trace!("Wait for capture thread to exit..");
                            cap_handle.join().unwrap();
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
                        StatusMessage::SetVolume(vol) => {
                            debug!("SetVolume message to  {vol} dB received");
                            status_structs.processing.set_target_volume(0, vol);
                        }
                        StatusMessage::SetMute(mute) => {
                            debug!("SetMute message to {mute} received");
                            status_structs.processing.set_mute(0, mute);
                        }
                    },
                    Err(err) => {
                        warn!("Capture, Playback and Processing threads have exited: {err}");
                        crate::set_stop_reason(
                            &status_structs.status,
                            StopReason::UnknownError(
                                "Capture, Playback and Processing threads have exited"
                                    .to_string(),
                            ),
                        );
                        crate::set_capture_state(
                            &status_structs.capture,
                            ProcessingState::Inactive,
                        );
                        return Ok(ExitState::Restart);
                    }
                }
            }
        }
    }
}

pub fn run_engine(engine_params: EngineConfig, logger: flexi_logger::LoggerHandle) -> i32 {
    let configname = engine_params.configname;
    let statefilename = engine_params.statefilename;
    let initial_volumes = engine_params.initial_volumes;
    let initial_mutes = engine_params.initial_mutes;
    let wait = engine_params.wait;
    #[cfg(feature = "websocket")]
    let ws_port = engine_params.ws_port;
    #[cfg(feature = "websocket")]
    let ws_address = engine_params.ws_address;
    #[cfg(feature = "secure-websocket")]
    let ws_cert = engine_params.ws_cert;
    #[cfg(feature = "secure-websocket")]
    let ws_pass = engine_params.ws_pass;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let _signal = unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGHUP, || debug!("Received SIGHUP"))
    };

    #[cfg(target_os = "windows")]
    wasapi::initialize_mta().unwrap();

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
                error!("{err}");
                debug!("Exiting due to config error");
                return EXIT_BAD_CONFIG;
            }
        }
    }

    #[cfg(any(not(windows), feature = "websocket"))]
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
            debug!("Received signal: {info}");
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
                                    error!("Error sending reload message: {e}");
                                }
                            }
                            Err(err) => {
                                error!("Config error during reload: {err}");
                            }
                        };
                    } else {
                        error!("Config path not specified, cannot reload");
                    }
                }
                SIGUSR1 => {
                    if let Err(e) = tx_command_thread.try_send(ControllerMessage::Stop) {
                        error!("Error sending stop message: {e}");
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
                    SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                        error!("Error sending exit message: {e}");
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
                SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                    error!("Error sending exit message: {e}");
                }
            }
            thread::sleep(DELAY);
        }
    });

    let capture_status = Arc::new(RwLock::new(CaptureStatus {
        measured_samplerate: 0,
        update_interval: 1000,
        signal_range: 0.0,
        rate_adjust: 0.0,
        state: ProcessingState::Inactive,
        signal_rms: countertimer::ValueHistory::new(1024, 2),
        signal_peak: countertimer::ValueHistory::new(1024, 2),
        used_channels: Vec::new(),
        audio_buffer: Default::default(),
    }));
    let playback_status = Arc::new(RwLock::new(PlaybackStatus {
        buffer_level: 0,
        clipped_samples: 0,
        update_interval: 1000,
        signal_rms: countertimer::ValueHistory::new(1024, 2),
        signal_peak: countertimer::ValueHistory::new(1024, 2),
        audio_buffer: Default::default(),
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
        let (tx_state, rx_state) = crossbeam_channel::bounded(1);

        let processing_params_clone = processing_params.clone();
        let active_config_path_clone = active_config_path.clone();
        let unsaved_state_changes = Arc::new(AtomicBool::new(false));

        if let Some(port) = ws_port {
            let serverport = port;
            let serveraddress = ws_address.clone();

            let shared_data = websocket_server::SharedData {
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
            let server_params = websocket_server::ServerParameters {
                port: serverport,
                address: &serveraddress,
                #[cfg(feature = "secure-websocket")]
                cert_file: ws_cert.as_deref(),
                #[cfg(feature = "secure-websocket")]
                cert_pass: ws_pass.as_deref(),
            };
            websocket_server::start_server(server_params, shared_data);
        }

        if let Some(fname) = &statefilename {
            let fname = fname.clone();

            thread::spawn(move || {
                loop {
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
                    debug!(
                        "Wait mode is disabled, there are no queued commands, and no new config. Exiting."
                    );
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
                    warn!("Error recv from cmd queue {e}");
                    return EXIT_OK;
                }
            }
        }

        let shared_configs = SharedConfigs {
            active: active_config.clone(),
            previous: previous_config.clone(),
        };

        debug!("Config ready, start processing");
        SHUTDOWN_REQUESTED.store(false, std::sync::atomic::Ordering::Relaxed);
        let exitstatus = run(shared_configs, status_structs.clone(), rx_command.clone());
        debug!("Processing ended with status {exitstatus:?}");

        match exitstatus {
            Err(e) => {
                error!("{e}");
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
