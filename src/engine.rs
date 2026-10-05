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

use crossbeam_channel::{RecvTimeoutError, select};
use parking_lot::{Mutex, RwLockUpgradableReadGuard};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

use crate::audiodevice::query_capture_source;
use crate::controller::{
    self, ConfigSource, ControllerSettings, ControllerShared, LoadKind, LoadedConfig, Provider,
    Selected, Selection, SourceFormat, SourceState,
};
use crate::engine_pipeline::start_pipeline;
use crate::engine_process_signals::launch_process_signals_thread;
use crate::filters::fftconv::ImpulseCache;
use crate::utils::stash;
use crate::websocket_server;
use crate::{
    CommandMessage, ControllerMessage, ProcessingState, SHUTDOWN_REQUESTED, SharedConfigs,
    StatusMessage, StatusStructs, StopReason,
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

/// First wait before retrying after a device error.
const RECOVERY_FIRST_DELAY: Duration = Duration::from_secs(1);
/// Longest wait between retries.
const RECOVERY_MAX_DELAY: Duration = Duration::from_secs(30);
/// A session that runs this long past its startup resets the backoff.
const RECOVERY_RESET_AFTER: Duration = Duration::from_secs(10);
/// How often the source is queried while waiting for it to change.
const SOURCE_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Settings for the built-in controller, `None` if there are none.
    pub controller_settings: Option<ControllerSettings>,
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

/// Why a session ended.
enum SessionEnd {
    /// An Exit command.
    Exit,
    /// A Stop command.
    Stopped,
    /// The stream ended.
    Done,
    /// A device error, the stop reason is already recorded.
    Error,
    /// A device reported a format change.
    FormatChange {
        format: SourceFormat,
        capture: bool,
        /// Whether the session got past the startup barrier.
        started: bool,
    },
    /// A loaded entry needs the devices restarted with this config.
    Restart(Box<Selected>),
    /// A loaded entry has no config for the source format.
    WaitSource(SourceFormat),
    /// The command channel failed.
    Failed(Box<dyn std::error::Error>),
}

struct SessionResult {
    end: SessionEnd,
    /// The config that ran last.
    last: Box<Selected>,
    /// How long the session ran past its startup barrier.
    ran_for: Option<Duration>,
}

/// What the controller does next.
enum Next {
    /// Wait for a command.
    Idle,
    /// Select a config to run.
    Select(Pending),
    /// Run a session with this config.
    Start(Box<Selected>),
    /// Wait out the backoff, then run this config again.
    Retry(Box<Selected>),
    /// Poll the source until it changes to a format a provider has a config for.
    WaitSource(SourceFormat),
    /// Exit the process with this code.
    Exit(i32),
}

/// A loaded config waiting for selection.
enum Pending {
    /// The entry config changed, or the format did. `validated` is the entry as is,
    /// already validated, and `query` asks the source for its format first.
    Entry {
        validated: Option<Box<(config::Configuration, ImpulseCache)>>,
        query: bool,
    },
    /// A patched Specific variant.
    Variant(Box<LoadedConfig>),
}

/// Whether capture and playback run on one ASIO driver, and so share a clock.
#[allow(unused_variables)]
fn is_asio_full_duplex(conf: &config::Configuration) -> bool {
    #[cfg(target_os = "windows")]
    if let (config::CaptureDevice::Asio(cap), config::PlaybackDevice::Asio(pb)) =
        (&conf.devices.capture, &conf.devices.playback)
    {
        return cap.device == pb.device;
    }
    false
}

/// The backoff before retry number `attempt`, counting from 1.
fn recovery_delay(attempt: usize) -> Duration {
    let doublings = attempt.saturating_sub(1).min(16) as u32;
    (RECOVERY_FIRST_DELAY * 2u32.pow(doublings)).min(RECOVERY_MAX_DELAY)
}

/// Decides which config runs and what happens when a session ends.
///
/// This is the only place that does. With following and recovery off it behaves as the
/// plain supervisor loop did: run the config that was loaded, and go idle when the
/// session ends.
struct Controller {
    wait: bool,
    shared: ControllerShared,
    /// The config the user loaded explicitly.
    entry: Option<ConfigSource>,
    /// The last format the source reported, kept until the process ends.
    last_format: Option<SourceFormat>,
    /// Retries since the last session that ran long enough.
    attempts: usize,
    shared_configs: SharedConfigs,
    status_structs: StatusStructs,
    rx: crossbeam_channel::Receiver<ControllerMessage>,
}

impl Controller {
    fn following(&self, settings: &ControllerSettings) -> bool {
        self.wait && settings.following_enabled()
    }

    fn recovery(&self, settings: &ControllerSettings) -> bool {
        self.wait && settings.recovery_enabled()
    }

    fn run(&mut self) -> i32 {
        let mut next = Next::Idle;
        loop {
            next = match next {
                Next::Idle => self.idle(),
                Next::Select(pending) => {
                    let selection = self.select(pending, true);
                    self.after_selection(selection)
                }
                Next::Start(selected) => self.start(selected),
                Next::Retry(selected) => self.backoff(selected),
                Next::WaitSource(format) => self.wait_for_source(format),
                Next::Exit(code) => return code,
            };
        }
    }

    /// Take in a loaded config.
    fn accept_load(&mut self, loaded: LoadedConfig) -> Pending {
        match loaded.kind {
            LoadKind::Entry => {
                if let Some(old) = &self.entry
                    && old.raw.devices.capture.device_key()
                        != loaded.source.raw.devices.capture.device_key()
                {
                    debug!("The capture device changed, forgetting the last source format");
                    self.last_format = None;
                }
                *self.shared.entry.lock() = Some(loaded.source.clone());
                self.entry = Some(loaded.source);
                Pending::Entry {
                    validated: Some(Box::new((loaded.validated, loaded.impulses))),
                    query: true,
                }
            }
            LoadKind::Variant => Pending::Variant(Box::new(loaded)),
        }
    }

    /// Select the config to run. The source is only queried when `may_query` is set,
    /// since the capture device is busy while a session runs.
    fn select(&mut self, pending: Pending, may_query: bool) -> Selection {
        let settings = self.shared.settings();
        let following = self.following(&settings);
        match pending {
            Pending::Variant(loaded) => {
                let format = if following {
                    self.last_format.as_ref()
                } else {
                    None
                };
                match controller::select_variant(*loaded, format) {
                    Ok(selected) => Selection::Run(selected),
                    Err(err) => Selection::Invalid(err),
                }
            }
            Pending::Entry { validated, query } => {
                let Some(entry) = self.entry.clone() else {
                    return Selection::Invalid("There is no config to run".to_string());
                };
                if following
                    && query
                    && may_query
                    && let SourceState::Format(format) =
                        query_capture_source(&entry.raw.devices.capture)
                {
                    let format = format.snapped();
                    info!("The capture source reports {format}");
                    self.last_format = Some(format);
                }
                let format = if following {
                    self.last_format.clone()
                } else {
                    None
                };
                let validated = if format.is_none() {
                    validated.map(|v| *v)
                } else {
                    None
                };
                controller::select_config(&entry, format.as_ref(), &settings, validated)
            }
        }
    }

    fn after_selection(&mut self, selection: Selection) -> Next {
        match selection {
            Selection::Run(selected) => Next::Start(selected),
            Selection::Invalid(err) => {
                error!("Config is not valid, not starting: {err}");
                Next::Idle
            }
            Selection::NoConfig(format) => {
                warn!("Following: no config for {format}, waiting for the source to change");
                Next::WaitSource(format)
            }
        }
    }

    /// Wait for a command. Without wait mode, exit once nothing is queued.
    fn idle(&mut self) -> Next {
        {
            let mut status = self.shared.status.lock();
            status.active_config_file = None;
            status.recovering = false;
            status.next_retry = None;
            status.waiting_for_source = None;
        }
        *self.shared.running.lock() = None;
        debug!("Wait for config");
        let mut pending = None;
        loop {
            let has_commands = !self.rx.is_empty();
            if !has_commands {
                if let Some(pending) = pending.take() {
                    debug!("New config is available and there are no queued commands, continuing");
                    return Next::Select(pending);
                }
                if !self.wait {
                    debug!(
                        "Wait mode is disabled, there are no queued commands, and no new config. Exiting."
                    );
                    return Next::Exit(EXIT_OK);
                }
            }
            debug!("Waiting to receive a command");
            match self.rx.recv() {
                Ok(ControllerMessage::ConfigChanged(loaded)) => {
                    debug!("Config change command received");
                    pending = Some(self.accept_load(*loaded));
                }
                Ok(ControllerMessage::Stop) => {
                    debug!("Stop command received");
                    pending = None;
                }
                Ok(ControllerMessage::Exit) => {
                    debug!("Exit command received");
                    return Next::Exit(EXIT_OK);
                }
                Err(e) => {
                    warn!("Error recv from cmd queue {e}");
                    return Next::Exit(EXIT_OK);
                }
            }
        }
    }

    /// Run a session, and decide what follows it.
    fn start(&mut self, selected: Box<Selected>) -> Next {
        let settings = self.shared.settings();
        let follow = self.following(&settings);
        self.publish_running(&selected);

        debug!("Config ready, start processing");
        SHUTDOWN_REQUESTED.store(false, std::sync::atomic::Ordering::Relaxed);
        let result = self.run_session(selected, follow);

        {
            let mut active_cfg_shared = self.shared_configs.active.lock();
            let mut prev_cfg_shared = self.shared_configs.previous.lock();
            *active_cfg_shared = None;
            *prev_cfg_shared = Some(result.last.config.clone());
        }
        *self.shared.running.lock() = None;

        let last = result.last;
        match result.end {
            SessionEnd::Exit => {
                debug!("Exiting");
                Next::Exit(EXIT_OK)
            }
            SessionEnd::Failed(err) => {
                error!("{err}");
                if self.wait {
                    Next::Idle
                } else {
                    Next::Exit(EXIT_PROCESSING_ERROR)
                }
            }
            SessionEnd::Stopped => {
                self.attempts = 0;
                Next::Idle
            }
            SessionEnd::Done => Next::Idle,
            SessionEnd::Restart(selected) => {
                debug!("Restarting with new config");
                Next::Start(selected)
            }
            SessionEnd::WaitSource(format) => {
                warn!("Following: no config for {format}, waiting for the source to change");
                Next::WaitSource(format)
            }
            SessionEnd::Error => {
                if result.ran_for.is_some_and(|d| d >= RECOVERY_RESET_AFTER) {
                    self.attempts = 0;
                }
                if self.recovery(&settings) {
                    self.attempts += 1;
                    Next::Retry(last)
                } else {
                    Next::Idle
                }
            }
            SessionEnd::FormatChange {
                format,
                capture,
                started,
            } => {
                let format = format.snapped();
                if !follow || !(capture || is_asio_full_duplex(&last.config)) {
                    return Next::Idle;
                }
                if !started && last.format.as_ref().is_some_and(|f| f.same_as(&format)) {
                    error!(
                        "Following: the config selected for {format} stopped at startup with a change to the same format, not following it"
                    );
                    return Next::Idle;
                }
                info!("Following: the capture source changed to {format}");
                self.last_format = Some(format);
                self.attempts = 0;
                Next::Select(Pending::Entry {
                    validated: None,
                    query: false,
                })
            }
        }
    }

    /// Record a config as the one running, for the websocket getters.
    fn publish_running(&self, selected: &Selected) {
        match &selected.provider {
            Provider::Entry => debug!("Running the entry config"),
            Provider::Specific(file) => info!(
                "Following: running '{file}' for {}",
                selected
                    .format
                    .as_ref()
                    .map(|f| f.to_string())
                    .unwrap_or_default()
            ),
            Provider::Adapt => info!(
                "Following: running the entry config adapted to {}",
                selected
                    .format
                    .as_ref()
                    .map(|f| f.to_string())
                    .unwrap_or_default()
            ),
        }
        *self.shared_configs.active.lock() = Some(selected.config.clone());
        *self.shared.running.lock() = Some(selected.running.clone());
        let mut status = self.shared.status.lock();
        status.active_config_file = match &selected.provider {
            Provider::Specific(file) => Some(file.clone()),
            _ => None,
        };
        status.waiting_for_source = None;
    }

    /// Wait out the backoff after an error, then retry the same config.
    fn backoff(&mut self, selected: Box<Selected>) -> Next {
        let delay = recovery_delay(self.attempts);
        let reason = self.status_structs.status.read().stop_reason.clone();
        warn!(
            "Error recovery: attempt {} in {} s, after {reason:?}",
            self.attempts,
            delay.as_secs_f32()
        );
        let deadline = Instant::now() + delay;
        {
            let mut status = self.shared.status.lock();
            status.recovering = true;
            status.attempts = self.attempts;
            status.next_retry = Some(deadline);
        }
        let next = match self.rx.recv_deadline(deadline) {
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                self.shared.status.lock().next_retry = None;
                return Next::Start(selected);
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Next::Exit(EXIT_OK),
            Ok(ControllerMessage::ConfigChanged(loaded)) => {
                debug!("New config during error recovery, cancelling the retry");
                Next::Select(self.accept_load(*loaded))
            }
            Ok(ControllerMessage::Stop) => {
                debug!("Stop during error recovery, cancelling the retry");
                Next::Idle
            }
            Ok(ControllerMessage::Exit) => Next::Exit(EXIT_OK),
        };
        self.attempts = 0;
        let mut status = self.shared.status.lock();
        status.recovering = false;
        status.attempts = 0;
        status.next_retry = None;
        next
    }

    /// Poll the source until it changes to a format a provider has a config for.
    fn wait_for_source(&mut self, format: SourceFormat) -> Next {
        let Some(entry) = self.entry.clone() else {
            return Next::Idle;
        };
        let capture = entry.raw.devices.capture.clone();
        let mut current = format;
        self.shared.status.lock().waiting_for_source = Some(current.clone());
        let mut first = true;
        let next = loop {
            if !first {
                match self.rx.recv_timeout(SOURCE_POLL_INTERVAL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break Next::Exit(EXIT_OK),
                    Ok(ControllerMessage::ConfigChanged(loaded)) => {
                        break Next::Select(self.accept_load(*loaded));
                    }
                    Ok(ControllerMessage::Stop) => break Next::Idle,
                    Ok(ControllerMessage::Exit) => break Next::Exit(EXIT_OK),
                }
            }
            first = false;
            match query_capture_source(&capture) {
                SourceState::Format(reported) => {
                    let reported = reported.snapped();
                    if reported.same_as(&current) {
                        trace!("The source is still at {current}");
                        continue;
                    }
                    info!("Following: the capture source changed to {reported}");
                    self.last_format = Some(reported);
                    match self.select(
                        Pending::Entry {
                            validated: None,
                            query: false,
                        },
                        false,
                    ) {
                        Selection::Run(selected) => break Next::Start(selected),
                        Selection::Invalid(err) => {
                            error!("Config is not valid, not starting: {err}");
                            break Next::Idle;
                        }
                        Selection::NoConfig(format) => {
                            warn!("Following: no config for {format} either, still waiting");
                            current = format;
                            self.shared.status.lock().waiting_for_source = Some(current.clone());
                        }
                    }
                }
                SourceState::Inactive => {
                    info!("Following: the capture source is inactive, starting the entry config");
                    let settings = self.shared.settings();
                    match controller::select_config(&entry, None, &settings, None) {
                        Selection::Run(selected) => break Next::Start(selected),
                        Selection::Invalid(err) => {
                            error!("Config is not valid, not starting: {err}");
                            break Next::Idle;
                        }
                        Selection::NoConfig(_) => break Next::Idle,
                    }
                }
                SourceState::Unknown => {
                    warn!(
                        "Following: this capture backend can't report when the source changes, going idle"
                    );
                    break Next::Idle;
                }
            }
        };
        self.shared.status.lock().waiting_for_source = None;
        next
    }

    /// Run one processing session: open devices, process audio, and return why it ended.
    fn run_session(&mut self, selected: Box<Selected>, follow: bool) -> SessionResult {
        let mut current = selected;
        let mut is_starting = true;
        let mut started_at: Option<Instant> = None;
        let (mut pipeline, rx_status) =
            start_pipeline(&current.config, &self.status_structs, follow);
        let status_structs = self.status_structs.clone();
        let rx_ctrl = self.rx.clone();

        macro_rules! end {
            ($end:expr) => {
                return SessionResult {
                    end: $end,
                    last: current,
                    ran_for: started_at.map(|t| t.elapsed()),
                }
            };
        }

        // Record why a device stopped, and stop the pipeline.
        macro_rules! stop_with {
            ($stop_reason:expr, $end:expr) => {{
                crate::set_stop_reason(&status_structs.status, $stop_reason);
                pipeline.stop(is_starting);
                trace!("All threads stopped, returning");
                end!($end);
            }};
        }

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
                        Ok(ControllerMessage::ConfigChanged(loaded)) => {
                            if !ctrl_ch.is_empty() {
                                debug!("Dropping config change command since there are more commands in the queue");
                                continue;
                            }
                            status_structs.processing.set_processing_load(0.0);
                            status_structs.processing.set_resampler_load(0.0);
                            let pending = self.accept_load(*loaded);
                            let new = match self.select(pending, false) {
                                Selection::Run(new) => new,
                                Selection::Invalid(err) => {
                                    error!("Config is not valid, keeping the current one: {err}");
                                    continue;
                                }
                                Selection::NoConfig(format) => {
                                    pipeline.stop(is_starting);
                                    end!(SessionEnd::WaitSource(format));
                                }
                            };
                            let comp = config::config_diff(&current.config, &new.config);
                            match comp {
                                config::ConfigChange::Pipeline
                                | config::ConfigChange::MixerParameters
                                | config::ConfigChange::FilterParameters { .. } => {
                                    // A new mixer can widen the pipeline, so top up the
                                    // stash before the processing thread gets to it.
                                    if !matches!(comp, config::ConfigChange::FilterParameters { .. }) {
                                        stash::prefill_for_config(&new.config);
                                    }
                                    // Transforming the coefficients happens here, so a
                                    // config whose files went missing since it was
                                    // validated fails before anything is changed, and
                                    // the pipeline keeps running what it has.
                                    if let Err(err) = pipeline.update_processing_config(
                                        comp,
                                        new.config.clone(),
                                        &new.impulses,
                                    ) {
                                        error!("Could not prepare the new configuration, keeping the current one. Reason: {err}");
                                        continue;
                                    }
                                    current = new;
                                    self.publish_running(&current);
                                    let used_channels = config::used_capture_channels(&current.config);
                                    debug!("Using channels {used_channels:?}");
                                    status_structs
                                        .capture
                                        .read()
                                        .used_channels
                                        .set(&used_channels);
                                    debug!("Sent changes to pipeline");
                                }
                                config::ConfigChange::Devices => {
                                    debug!("Devices changed, restart required.");
                                    pipeline.stop(is_starting);
                                    trace!("All threads stopped, returning");
                                    end!(SessionEnd::Restart(new));
                                }
                                config::ConfigChange::None => {
                                    debug!("No changes in config.");
                                    current = new;
                                    self.publish_running(&current);
                                }
                            };
                        },
                        Ok(ControllerMessage::Stop) => {
                            debug!("Stop requested...");
                            pipeline.stop(is_starting);
                            trace!("All threads stopped, stopping");
                            end!(SessionEnd::Stopped);
                        },
                        Ok(ControllerMessage::Exit) => {
                            debug!("Exit requested...");
                            pipeline.stop(is_starting);
                            trace!("All threads stopped, exiting");
                            end!(SessionEnd::Exit);
                        },
                        Err(err) => {
                            end!(SessionEnd::Failed(Box::new(err)));
                        }
                    }
                },
                recv(rx_status) -> msg => {
                    match msg {
                        Ok(msg) => match msg {
                            StatusMessage::PlaybackReady => {
                                debug!("Playback thread ready to start");
                                pipeline.set_playback_ready();
                                if pipeline.release_barrier_if_ready() {
                                    is_starting = false;
                                    started_at = Some(Instant::now());
                                    self.shared.status.lock().recovering = false;
                                    // Startup is complete once the second device reports in,
                                    // and which one that is depends on how the two threads
                                    // are scheduled. So the reason the previous session
                                    // stopped is cleared in both arms. Clearing it in only
                                    // one left a new session that came up playback-last
                                    // reporting the old reason for as long as it ran.
                                    crate::set_stop_reason(&status_structs.status, StopReason::None);
                                }
                            }
                            StatusMessage::CaptureReady => {
                                debug!("Capture thread ready to start");
                                pipeline.set_capture_ready();
                                if pipeline.release_barrier_if_ready() {
                                    is_starting = false;
                                    started_at = Some(Instant::now());
                                    self.shared.status.lock().recovering = false;
                                    crate::set_stop_reason(&status_structs.status, StopReason::None);
                                }
                            }
                            StatusMessage::PlaybackError(message) => {
                                error!("Playback error: {message}");
                                stop_with!(StopReason::PlaybackError(message), SessionEnd::Error);
                            }
                            StatusMessage::CaptureError(message) => {
                                error!("Capture error: {message}");
                                stop_with!(StopReason::CaptureError(message), SessionEnd::Error);
                            }
                            StatusMessage::PlaybackFormatChange(format) => {
                                error!("Playback stopped due to external format change");
                                stop_with!(
                                    StopReason::PlaybackFormatChange(format.clone()),
                                    SessionEnd::FormatChange {
                                        format,
                                        capture: false,
                                        started: !is_starting,
                                    }
                                );
                            }
                            StatusMessage::CaptureFormatChange(format) => {
                                error!("Capture stopped due to external format change");
                                stop_with!(
                                    StopReason::CaptureFormatChange(format.clone()),
                                    SessionEnd::FormatChange {
                                        format,
                                        capture: true,
                                        started: !is_starting,
                                    }
                                );
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
                                pipeline.stop(is_starting);
                                trace!("All threads stopped, returning");
                                end!(SessionEnd::Done);
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
                            end!(SessionEnd::Error);
                        }
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

    let controller_shared = ControllerShared {
        settings: Arc::new(Mutex::new(engine_params.controller_settings)),
        wait,
        ..Default::default()
    };
    let settings = controller_shared.settings();
    if settings.any_enabled() && !wait {
        warn!(
            "Following the capture source and error recovery only work in wait mode, they are off"
        );
    }

    let (tx_command, rx_command) = crossbeam_channel::bounded(10);
    if let Some(path) = &configname {
        match LoadedConfig::from_file(path) {
            Ok(loaded) => {
                debug!("Config is valid");
                if wait && let Some(template) = settings.specific_template() {
                    controller::log_preflight(template, Some(&loaded.source));
                }
                tx_command
                    .send(ControllerMessage::ConfigChanged(Box::new(loaded)))
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
    let controller_settings_clone = controller_shared.settings.clone();
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
            controller: controller_shared.clone(),
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
                                &controller_settings_clone,
                                &unsaved_state_changes,
                            );
                        }
                        Err(_) => break,
                    }
                }
            })
            .expect("can spawn statefile thread");
    }

    let mut controller = Controller {
        wait,
        shared: controller_shared,
        entry: None,
        last_format: None,
        attempts: 0,
        shared_configs: SharedConfigs {
            active: active_config,
            previous: previous_config,
        },
        status_structs,
        rx: rx_command,
    };
    controller.run()
}

#[cfg(test)]
mod tests {
    use super::recovery_delay;
    use std::time::Duration;

    #[test]
    fn recovery_backoff_doubles_up_to_the_cap() {
        let delays: Vec<u64> = (1..=8).map(|a| recovery_delay(a).as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(recovery_delay(1000), Duration::from_secs(30));
    }
}
