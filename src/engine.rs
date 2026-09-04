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
use parking_lot::{Mutex, RwLockUpgradableReadGuard};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

use crate::engine_pipeline::{EnginePipeline, start_pipeline};
use crate::engine_process_signals::launch_process_signals_thread;
use crate::websocket_server;
use crate::{
    CommandMessage, ControllerMessage, ExitState, ProcessingState, SHUTDOWN_REQUESTED,
    SharedConfigs, StatusMessage, StatusStructs, StopReason,
};
use crate::{config, statefile};

/// Process exit code: clean exit.
pub const EXIT_OK: i32 = 0;
/// Process exit code: configuration error on startup.
pub const EXIT_BAD_CONFIG: i32 = 101;
/// Process exit code: unrecoverable processing error.
pub const EXIT_PROCESSING_ERROR: i32 = 102;
/// Process exit code: forced exit (e.g. repeated restarts exceeded limit).
pub const EXIT_FORCED: i32 = 103;

/// Top-level configuration passed to [`run_engine`].
pub struct EngineConfig {
    /// Path to the initial configuration YAML file, or `None` to start in standby.
    pub configname: Option<String>,
    /// Path to the state file for persisting volume/mute across restarts.
    pub statefilename: Option<String>,
    /// Initial volume (dB) for each fader.
    pub initial_volumes: [f32; 5],
    /// Initial mute state for each fader.
    pub initial_mutes: [bool; 5],
    /// If `true`, wait for a configuration via WebSocket rather than exiting when none is provided.
    pub wait: bool,
    /// WebSocket server port.
    pub ws_port: Option<usize>,
    /// WebSocket server bind address.
    pub ws_address: String,
    /// Path to TLS certificate for the secure WebSocket server (requires `secure-websocket` feature).
    #[cfg(feature = "secure-websocket")]
    pub ws_cert: Option<String>,
    /// Password for the TLS certificate (requires `secure-websocket` feature).
    #[cfg(feature = "secure-websocket")]
    pub ws_pass: Option<String>,
}

/// Stop the pipeline after a fatal device event, record the stop reason, clear
/// the active config, and return [`ExitState::Restart`].
fn fail_and_restart(
    pipeline: EnginePipeline,
    is_starting: bool,
    stop_reason: StopReason,
    active_config: config::Configuration,
    shared_configs: &SharedConfigs,
    status_structs: &StatusStructs,
) -> crate::Res<ExitState> {
    crate::set_stop_reason(&status_structs.status, stop_reason);
    pipeline.stop(is_starting);
    {
        let mut active_cfg_shared = shared_configs.active.lock();
        let mut prev_cfg_shared = shared_configs.previous.lock();
        *active_cfg_shared = None;
        *prev_cfg_shared = Some(active_config);
    }
    crate::set_capture_state(&status_structs.capture, ProcessingState::Inactive);
    trace!("All threads stopped, returning");
    Ok(ExitState::Restart)
}

/// Run one processing session: open devices, process audio, and return an [`ExitState`].
pub fn run(
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

    let (mut pipeline, rx_status) = start_pipeline(&active_config, &status_structs);

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
                                pipeline.update_processing_config(comp, *new_conf.clone());
                                active_config = *new_conf;
                                *shared_configs.active.lock() = Some(active_config.clone());
                                let used_channels = config::used_capture_channels(&active_config);
                                debug!("Using channels {used_channels:?}");
                                status_structs.capture.write().used_channels = used_channels;
                                debug!("Sent changes to pipeline");
                            }
                            config::ConfigChange::Devices => {
                                debug!("Devices changed, restart required.");
                                pipeline.stop(is_starting);
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
                        pipeline.stop(is_starting);
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
                        pipeline.stop(is_starting);
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
                /// local shortcut for bailing out with fail_and_restart()
                macro_rules! fail_and_restart {
                    ($stop_reason:expr) => {
                        return fail_and_restart(
                            pipeline,
                            is_starting,
                            $stop_reason,
                            active_config,
                            &shared_configs,
                            &status_structs,
                        );
                    };
                }

                match msg {
                    Ok(msg) => match msg {
                        StatusMessage::PlaybackReady => {
                            debug!("Playback thread ready to start");
                            pipeline.set_playback_ready();
                            if pipeline.release_barrier_if_ready() {
                                is_starting = false;
                            }
                        }
                        StatusMessage::CaptureReady => {
                            debug!("Capture thread ready to start");
                            pipeline.set_capture_ready();
                            if pipeline.release_barrier_if_ready() {
                                is_starting = false;
                                crate::set_stop_reason(&status_structs.status, StopReason::None);
                            }
                        }
                        StatusMessage::PlaybackError(message) => {
                            error!("Playback error: {message}");
                            fail_and_restart!(StopReason::PlaybackError(message));
                        }
                        StatusMessage::CaptureError(message) => {
                            error!("Capture error: {message}");
                            fail_and_restart!(StopReason::CaptureError(message));
                        }
                        StatusMessage::PlaybackFormatChange(rate) => {
                            error!("Playback stopped due to external format change");
                            fail_and_restart!(StopReason::PlaybackFormatChange(rate));
                        }
                        StatusMessage::CaptureFormatChange(rate) => {
                            error!("Capture stopped due to external format change");
                            fail_and_restart!(StopReason::CaptureFormatChange(rate));
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
                            pipeline.stop(is_starting);
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        StatusMessage::CaptureDone => {
                            info!("Capture finished");
                        }
                        StatusMessage::SetSpeed(speed) => {
                            debug!("SetSpeed message received");
                            pipeline.send_capture_command(CommandMessage::SetSpeed { speed });
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

/// Entry point for the full CamillaDSP engine: initialises state, spawns threads, and runs until exit.
/// Returns a process exit code (one of `EXIT_OK`, `EXIT_BAD_CONFIG`, etc.).
pub fn run_engine(engine_params: EngineConfig, logger: flexi_logger::LoggerHandle) -> i32 {
    let configname = engine_params.configname;
    let statefilename = engine_params.statefilename;
    let initial_volumes = engine_params.initial_volumes;
    let initial_mutes = engine_params.initial_mutes;
    let wait = engine_params.wait;
    let ws_port = engine_params.ws_port;
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

    let active_config_path = Arc::new(Mutex::new(configname));

    launch_process_signals_thread(active_config_path.clone(), tx_command.clone(), logger);

    let status_structs = StatusStructs::default();
    let capture_status = status_structs.capture.clone();
    let playback_status = status_structs.playback.clone();
    let processing_params = status_structs.processing.clone();
    let processing_status = status_structs.status.clone();

    for fader in 0..5 {
        processing_params.set_target_volume(fader, initial_volumes[fader]);
        processing_params.set_current_volume(fader, initial_volumes[fader]);
        processing_params.set_mute(fader, initial_mutes[fader]);
    }
    let active_config = Arc::new(Mutex::new(None));
    let previous_config = Arc::new(Mutex::new(None));

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

        thread::Builder::new()
            .name("statefile".to_string())
            .spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(1000));
                    match rx_state.recv() {
                        Ok(()) => {
                            debug!("saving state to {}", fname);
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
            })
            .expect("can spawn statefile thread");
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
