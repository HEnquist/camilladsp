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

use parking_lot::{Mutex, RwLockUpgradableReadGuard};
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

#[cfg(feature = "websocket")]
use crate::websocket_server;
use crate::{
    CommandMessage, ControllerMessage, ExitState, ProcessingState, SHUTDOWN_REQUESTED,
    SharedConfigs, StatusMessage, StatusStructs, StopReason,
};
use crate::{audiodevice, config, processing, statefile};

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
    /// WebSocket server port (requires `websocket` feature).
    #[cfg(feature = "websocket")]
    pub ws_port: Option<usize>,
    /// WebSocket server bind address (requires `websocket` feature).
    #[cfg(feature = "websocket")]
    pub ws_address: String,
    /// Path to TLS certificate for the secure WebSocket server (requires `secure-websocket` feature).
    #[cfg(feature = "secure-websocket")]
    pub ws_cert: Option<String>,
    /// Password for the TLS certificate (requires `secure-websocket` feature).
    #[cfg(feature = "secure-websocket")]
    pub ws_pass: Option<String>,
}

/// Handles for supervising one running pipeline (one device group).
struct RunningPipeline {
    /// Commands (set speed, exit) to this group's capture thread.
    tx_command: crossbeam_channel::Sender<CommandMessage>,
    /// Live config updates to this group's processing thread.
    tx_pipeconf: crossbeam_channel::Sender<(config::ConfigChange, config::Configuration)>,
    /// Status messages from this group's capture and playback threads.
    rx_status: crossbeam_channel::Receiver<StatusMessage>,
    /// 4-way startup barrier (capture, playback, processing, supervisor).
    barrier: Arc<Barrier>,
    pb_handle: Box<thread::JoinHandle<()>>,
    cap_handle: Box<thread::JoinHandle<()>>,
    proc_handle: thread::JoinHandle<()>,
    /// Status structs for this group. Group 0 shares the process-lifetime structs
    /// held by the WebSocket server; others share only the global run status.
    status: StatusStructs,
    pb_ready: bool,
    cap_ready: bool,
    /// Whether the supervisor has already met this group's startup barrier.
    barrier_released: bool,
}

/// The event a supervisor `select` resolved to: either a controller command, or
/// a status message from the pipeline at the given index.
enum SupervisorEvent {
    Control(Result<ControllerMessage, crossbeam_channel::RecvError>),
    Status(usize, Result<StatusMessage, crossbeam_channel::RecvError>),
}

/// Stop every pipeline in a session: tell all capture threads to exit, release
/// any startup barriers the supervisor has not yet satisfied (so blocked
/// device/processing threads can reach their exit paths), then join all threads.
fn stop_all_pipelines(pipelines: &mut Vec<RunningPipeline>) {
    for pipeline in pipelines.iter() {
        if pipeline.tx_command.send(CommandMessage::Exit).is_err() {
            debug!("Capture thread has already exited");
        }
    }
    for pipeline in pipelines.iter_mut() {
        if !pipeline.barrier_released {
            pipeline.barrier.wait();
            pipeline.barrier_released = true;
        }
    }
    for pipeline in pipelines.drain(..) {
        let _ = pipeline.pb_handle.join();
        let _ = pipeline.cap_handle.join();
        let _ = pipeline.proc_handle.join();
    }
}

/// When both capture and playback of `group` are ready, meet its startup barrier.
/// Once every group's barrier has been met, finish startup and clear the stop
/// reason.
fn try_release_barrier(
    group: usize,
    pipelines: &mut [RunningPipeline],
    is_starting: &mut bool,
    status_structs: &StatusStructs,
) {
    let pipeline = &mut pipelines[group];
    if pipeline.pb_ready && pipeline.cap_ready && !pipeline.barrier_released {
        debug!("Device group {group} ready, releasing startup barrier");
        pipeline.barrier.wait();
        pipeline.barrier_released = true;
    }
    if *is_starting && pipelines.iter().all(|p| p.barrier_released) {
        debug!("All device groups ready, supervisor loop starts now!");
        *is_starting = false;
        crate::set_stop_reason(&status_structs.status, StopReason::None);
    }
}

/// Stop all pipelines after a fatal device event, record the stop reason, clear
/// the active config, and return [`ExitState::Restart`].
fn fail_and_restart(
    stop_reason: StopReason,
    shared_configs: &SharedConfigs,
    status_structs: &StatusStructs,
    pipelines: &mut Vec<RunningPipeline>,
    active_config: config::Configuration,
) -> crate::Res<ExitState> {
    crate::set_stop_reason(&status_structs.status, stop_reason);
    stop_all_pipelines(pipelines);
    {
        let mut active_cfg_shared = shared_configs.active.lock();
        let mut prev_cfg_shared = shared_configs.previous.lock();
        *active_cfg_shared = None;
        *prev_cfg_shared = Some(active_config);
    }
    crate::set_capture_state(&status_structs.capture, ProcessingState::Inactive);
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
    let num_groups = active_config.devices.len();

    // One independent capture -> processing -> playback pipeline per device
    // group. Each gets its own channels and a 4-way startup barrier (capture,
    // playback, processing, supervisor). The control channel (`rx_ctrl`) and the
    // shared configs are global across all groups.
    let mut pipelines: Vec<RunningPipeline> = Vec::with_capacity(num_groups);

    for group in 0..num_groups {
        // Group 0 reuses the process-lifetime status structs held by the
        // WebSocket server; additional groups get fresh structs that share the
        // global run status (stop reason).
        let group_status = if group == 0 {
            status_structs.clone()
        } else {
            status_structs.new_with_shared_run_status()
        };
        let group_devices = active_config.devices.group(group).clone();

        let (tx_pb, rx_pb) = crossbeam_channel::bounded(group_devices.queuelimit());
        let (tx_cap, rx_cap) = crossbeam_channel::bounded(group_devices.queuelimit());
        let (tx_status_group, rx_status_group) = crossbeam_channel::unbounded();
        let (tx_cmd, rx_cmd) = crossbeam_channel::unbounded();
        let (tx_pipeconf_group, rx_pipeconf_group) = crossbeam_channel::unbounded();
        let barrier = Arc::new(Barrier::new(4));

        // Processing thread
        let proc_handle = processing::run_processing(
            active_config.clone(),
            group,
            barrier.clone(),
            tx_pb,
            rx_cap,
            rx_pipeconf_group,
            group_status.processing.clone(),
        );

        // Playback thread
        let mut playback_dev = audiodevice::new_playback_device(group_devices.clone());
        let pb_handle = playback_dev
            .start(
                rx_pb,
                barrier.clone(),
                tx_status_group.clone(),
                group_status.playback.clone(),
            )
            .unwrap();

        let used_channels = config::used_capture_channels(&active_config, group);
        debug!("Device group {group} using channels {used_channels:?}");
        {
            let mut capture_status = group_status.capture.write();
            crate::update_capture_state(&mut capture_status, ProcessingState::Starting);
            capture_status.used_channels = used_channels;
        }

        // Capture thread
        let mut capture_dev = audiodevice::new_capture_device(group_devices);
        let cap_handle = capture_dev
            .start(
                tx_cap,
                barrier.clone(),
                tx_status_group,
                rx_cmd,
                group_status.capture.clone(),
                group_status.processing.clone(),
            )
            .unwrap();

        pipelines.push(RunningPipeline {
            tx_command: tx_cmd,
            tx_pipeconf: tx_pipeconf_group,
            rx_status: rx_status_group,
            barrier,
            pb_handle,
            cap_handle,
            proc_handle,
            status: group_status,
            pb_ready: false,
            cap_ready: false,
            barrier_released: false,
        });
    }

    loop {
        // Wait for a status message from any group, or (once startup is done) a
        // controller command. A fresh selector is built each iteration because
        // the set of receivers is dynamic (one status channel per group).
        let event = {
            let mut sel = crossbeam_channel::Select::new();
            for pipeline in &pipelines {
                sel.recv(&pipeline.rx_status);
            }
            // Only accept controller commands once every group has started.
            let ctrl_index = if is_starting {
                None
            } else {
                Some(sel.recv(&rx_ctrl))
            };
            let oper = sel.select();
            let index = oper.index();
            if Some(index) == ctrl_index {
                SupervisorEvent::Control(oper.recv(&rx_ctrl))
            } else {
                SupervisorEvent::Status(index, oper.recv(&pipelines[index].rx_status))
            }
        };

        match event {
            SupervisorEvent::Control(msg) => match msg {
                Ok(ControllerMessage::ConfigChanged(new_conf)) => {
                    if !rx_ctrl.is_empty() {
                        debug!(
                            "Dropping config change command since there are more commands in the queue"
                        );
                        continue;
                    }
                    for pipeline in &pipelines {
                        pipeline.status.processing.set_processing_load(0.0);
                        pipeline.status.processing.set_resampler_load(0.0);
                    }
                    let comp = config::config_diff(&active_config, &new_conf);
                    match comp {
                        config::ConfigChange::Pipeline
                        | config::ConfigChange::MixerParameters
                        | config::ConfigChange::FilterParameters { .. } => {
                            // Every group's processing thread rebuilds its own
                            // chain from the new config.
                            for pipeline in &pipelines {
                                pipeline
                                    .tx_pipeconf
                                    .send((comp.clone(), (*new_conf).clone()))
                                    .unwrap();
                            }
                            active_config = *new_conf;
                            *shared_configs.active.lock() = Some(active_config.clone());
                            for (group, pipeline) in pipelines.iter().enumerate() {
                                let used_channels =
                                    config::used_capture_channels(&active_config, group);
                                pipeline.status.capture.write().used_channels = used_channels;
                            }
                            debug!("Sent changes to pipelines");
                        }
                        config::ConfigChange::Devices => {
                            debug!("Devices changed, restart required.");
                            stop_all_pipelines(&mut pipelines);
                            *shared_configs.active.lock() = Some(*new_conf);
                            trace!("All threads stopped, returning");
                            return Ok(ExitState::Restart);
                        }
                        config::ConfigChange::None => {
                            debug!("No changes in config.");
                        }
                    };
                }
                Ok(ControllerMessage::Stop) => {
                    debug!("Stop requested...");
                    stop_all_pipelines(&mut pipelines);
                    {
                        let mut active_cfg_shared = shared_configs.active.lock();
                        let mut prev_cfg_shared = shared_configs.previous.lock();
                        *active_cfg_shared = None;
                        *prev_cfg_shared = Some(active_config);
                    }
                    trace!("All threads stopped, stopping");
                    return Ok(ExitState::Restart);
                }
                Ok(ControllerMessage::Exit) => {
                    debug!("Exit requested...");
                    stop_all_pipelines(&mut pipelines);
                    *shared_configs.previous.lock() = Some(active_config);
                    trace!("All threads stopped, exiting");
                    return Ok(ExitState::Exit);
                }
                Err(err) => {
                    return Err(Box::new(err));
                }
            },
            SupervisorEvent::Status(group, msg) => match msg {
                Ok(msg) => match msg {
                    StatusMessage::PlaybackReady => {
                        debug!("Playback thread for group {group} ready to start");
                        pipelines[group].pb_ready = true;
                        try_release_barrier(
                            group,
                            &mut pipelines,
                            &mut is_starting,
                            &status_structs,
                        );
                    }
                    StatusMessage::CaptureReady => {
                        debug!("Capture thread for group {group} ready to start");
                        pipelines[group].cap_ready = true;
                        try_release_barrier(
                            group,
                            &mut pipelines,
                            &mut is_starting,
                            &status_structs,
                        );
                    }
                    StatusMessage::PlaybackError(message) => {
                        error!("Playback error (group {group}): {message}");
                        return fail_and_restart(
                            StopReason::PlaybackError(message),
                            &shared_configs,
                            &status_structs,
                            &mut pipelines,
                            active_config,
                        );
                    }
                    StatusMessage::CaptureError(message) => {
                        error!("Capture error (group {group}): {message}");
                        return fail_and_restart(
                            StopReason::CaptureError(message),
                            &shared_configs,
                            &status_structs,
                            &mut pipelines,
                            active_config,
                        );
                    }
                    StatusMessage::PlaybackFormatChange(rate) => {
                        error!("Playback stopped due to external format change (group {group})");
                        return fail_and_restart(
                            StopReason::PlaybackFormatChange(rate),
                            &shared_configs,
                            &status_structs,
                            &mut pipelines,
                            active_config,
                        );
                    }
                    StatusMessage::CaptureFormatChange(rate) => {
                        error!("Capture stopped due to external format change (group {group})");
                        return fail_and_restart(
                            StopReason::CaptureFormatChange(rate),
                            &shared_configs,
                            &status_structs,
                            &mut pipelines,
                            active_config,
                        );
                    }
                    StatusMessage::PlaybackDone => {
                        info!("Playback finished (group {group})");
                        {
                            let stat = status_structs.status.upgradable_read();
                            if stat.stop_reason == StopReason::None {
                                crate::update_stop_reason(
                                    &mut RwLockUpgradableReadGuard::upgrade(stat),
                                    StopReason::Done,
                                );
                            }
                        }
                        stop_all_pipelines(&mut pipelines);
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
                        info!("Capture finished (group {group})");
                    }
                    StatusMessage::SetSpeed(speed) => {
                        debug!("SetSpeed message received (group {group})");
                        if pipelines[group]
                            .tx_command
                            .send(CommandMessage::SetSpeed { speed })
                            .is_err()
                        {
                            debug!("Capture thread has already exited");
                        }
                    }
                    StatusMessage::SetVolume(vol) => {
                        debug!("SetVolume message to {vol} dB received (group {group})");
                        pipelines[group].status.processing.set_target_volume(0, vol);
                    }
                    StatusMessage::SetMute(mute) => {
                        debug!("SetMute message to {mute} received (group {group})");
                        pipelines[group].status.processing.set_mute(0, mute);
                    }
                },
                Err(err) => {
                    warn!(
                        "Capture, Playback and Processing threads of group {group} have exited: {err}"
                    );
                    return fail_and_restart(
                        StopReason::UnknownError(
                            "Capture, Playback and Processing threads have exited".to_string(),
                        ),
                        &shared_configs,
                        &status_structs,
                        &mut pipelines,
                        active_config,
                    );
                }
            },
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
