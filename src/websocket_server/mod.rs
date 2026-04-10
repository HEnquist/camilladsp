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

mod datastructures;
mod utils;

use clap::crate_version;
use crossbeam_channel::TrySendError;
use json_patch::merge;
#[cfg(feature = "secure-websocket")]
use native_tls::{TlsAcceptor, TlsStream};
use parking_lot::{Mutex, RwLock};
use serde_json;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::Message;
use tungstenite::WebSocket;

use self::datastructures::{
    AllLevels, ChannelLabels, Fader, PbCapLevels, ValueWithOptionalLimits, VuLevels,
    VuSubscription, WsCommand, WsReply, WsResult, WsSignalLevelSide,
};
use self::utils::{
    accept_plain_stream, capture_signal_global_peak, capture_signal_peak,
    capture_signal_peak_since, capture_signal_peak_since_last, capture_signal_rms,
    capture_signal_rms_since, capture_signal_rms_since_last, clamped_volume,
    current_processing_state, get_signal_levels_values_linear, get_state_event,
    get_stream_levels_event, is_timeout_error, parse_command, playback_signal_global_peak,
    playback_signal_peak, playback_signal_peak_since, playback_signal_peak_since_last,
    playback_signal_rms, playback_signal_rms_since, playback_signal_rms_since_last,
    reset_capture_signal_global_peak, reset_playback_signal_global_peak, set_stream_timeout,
    smooth_levels, smoothing_alpha, stream_invalid_reply, validate_vu_subscription,
};
#[cfg(feature = "secure-websocket")]
use self::utils::{accept_secure_stream, make_acceptor};
use crate::ProcessingState;
use crate::Res;
use crate::signal_monitor::{self, SignalLevelSide as MonitorSignalLevelSide};
use crate::utils::decibels::linear_to_db_inplace;
use crate::{
    CaptureStatus, PlaybackStatus, ProcessingParameters, ProcessingStatus, list_available_devices,
    list_supported_devices,
};
use crate::{ControllerMessage, config};

const SUBSCRIPTION_READ_TIMEOUT_MS: u64 = 10;

#[derive(Debug, Clone)]
pub struct SharedData {
    pub active_config: Arc<Mutex<Option<config::Configuration>>>,
    pub active_config_path: Arc<Mutex<Option<String>>>,
    pub previous_config: Arc<Mutex<Option<config::Configuration>>>,
    pub command_sender: crossbeam_channel::Sender<ControllerMessage>,
    pub capture_status: Arc<RwLock<CaptureStatus>>,
    pub playback_status: Arc<RwLock<PlaybackStatus>>,
    pub processing_params: Arc<ProcessingParameters>,
    pub processing_status: Arc<RwLock<ProcessingStatus>>,
    pub state_change_notify: crossbeam_channel::Sender<()>,
    pub state_file_path: Option<String>,
    pub unsaved_state_change: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalData {
    pub last_cap_rms_time: Instant,
    pub last_cap_peak_time: Instant,
    pub last_pb_rms_time: Instant,
    pub last_pb_peak_time: Instant,
}

#[derive(Debug, Clone)]
pub struct ServerParameters<'a> {
    pub address: &'a str,
    pub port: usize,
    #[cfg(feature = "secure-websocket")]
    pub cert_file: Option<&'a str>,
    #[cfg(feature = "secure-websocket")]
    pub cert_pass: Option<&'a str>,
}

#[derive(Debug, Default)]
struct VuSideState {
    last_update: Option<Instant>,
    rms: Vec<f32>,
    peak: Vec<f32>,
}

impl VuSideState {
    fn seed(&mut self, now: Instant, rms: Vec<f32>, peak: Vec<f32>) {
        self.last_update = Some(now);
        self.rms = rms;
        self.peak = peak;
    }

    fn update(
        &mut self,
        now: Instant,
        rms: Vec<f32>,
        peak: Vec<f32>,
        attack_ms: f32,
        release_ms: f32,
    ) {
        match self.last_update {
            None => self.seed(now, rms, peak),
            Some(previous) => {
                let attack = smoothing_alpha(now.duration_since(previous), attack_ms);
                let release = smoothing_alpha(now.duration_since(previous), release_ms);
                // Peak rises should stay immediate so short transients are not hidden.
                let peak_attack = 1.0;
                self.rms = smooth_levels(&self.rms, rms, attack, release);
                self.peak = smooth_levels(&self.peak, peak, peak_attack, release);
                self.last_update = Some(now);
            }
        }
    }
}

#[derive(Debug)]
struct VuSubscriptionState {
    publish_interval: Option<Duration>,
    attack_ms: f32,
    release_ms: f32,
    playback: VuSideState,
    capture: VuSideState,
    last_publish: Option<Instant>,
    pending_publish: bool,
}

impl VuSubscriptionState {
    fn new(config: VuSubscription) -> Self {
        let publish_interval = if config.max_rate > 0.0 {
            Some(Duration::from_secs_f32(1.0 / config.max_rate))
        } else {
            None
        };

        Self {
            publish_interval,
            attack_ms: config.attack.max(0.0),
            release_ms: config.release.max(0.0),
            playback: VuSideState::default(),
            capture: VuSideState::default(),
            last_publish: None,
            pending_publish: false,
        }
    }

    fn seed_from_shared(&mut self, shared_data: &SharedData) {
        let now = Instant::now();
        if let Some((rms, peak)) =
            get_signal_levels_values_linear(WsSignalLevelSide::Playback, shared_data)
        {
            self.playback.seed(now, rms, peak);
        }
        if let Some((rms, peak)) =
            get_signal_levels_values_linear(WsSignalLevelSide::Capture, shared_data)
        {
            self.capture.seed(now, rms, peak);
        }
    }

    fn update_side(
        &mut self,
        side: MonitorSignalLevelSide,
        rms: Vec<f32>,
        peak: Vec<f32>,
        now: Instant,
    ) {
        match side {
            MonitorSignalLevelSide::Playback => {
                self.playback
                    .update(now, rms, peak, self.attack_ms, self.release_ms);
            }
            MonitorSignalLevelSide::Capture => {
                self.capture
                    .update(now, rms, peak, self.attack_ms, self.release_ms);
            }
        }
        self.pending_publish = true;
    }

    fn should_publish_at(&self, now: Instant) -> bool {
        if !self.pending_publish {
            return false;
        }

        match self.publish_interval {
            None => true,
            Some(interval) => self
                .last_publish
                .map(|last| now.duration_since(last) >= interval)
                .unwrap_or(true),
        }
    }

    fn publish_at(&mut self, now: Instant) -> WsReply {
        self.last_publish = Some(now);
        self.pending_publish = false;
        let mut playback_rms = self.playback.rms.clone();
        let mut playback_peak = self.playback.peak.clone();
        let mut capture_rms = self.capture.rms.clone();
        let mut capture_peak = self.capture.peak.clone();
        linear_to_db_inplace(&mut playback_rms);
        linear_to_db_inplace(&mut playback_peak);
        linear_to_db_inplace(&mut capture_rms);
        linear_to_db_inplace(&mut capture_peak);
        WsReply::VuLevelsEvent {
            result: WsResult::Ok,
            value: VuLevels {
                playback_rms,
                playback_peak,
                capture_rms,
                capture_peak,
            },
        }
    }
}

#[derive(Debug)]
enum ActiveStream {
    SignalLevels(WsSignalLevelSide),
    VuLevels(VuSubscriptionState),
    State,
}

pub fn start_server(parameters: ServerParameters, shared_data: SharedData) {
    let address = parameters.address.to_string();
    let port = parameters.port;
    debug!("Start websocket server on {}:{}", address, parameters.port);
    #[cfg(feature = "secure-websocket")]
    let acceptor = make_acceptor(&parameters.cert_file, &parameters.cert_pass);

    thread::spawn(move || {
        let ws_result = TcpListener::bind(format!("{address}:{port}"));
        if let Ok(server) = ws_result {
            for stream in server.incoming() {
                match &stream {
                    Ok(s) => {
                        let local_addr = s
                            .local_addr()
                            .map(|a| a.to_string())
                            .unwrap_or("unknown".to_string());
                        let peer_addr = s
                            .peer_addr()
                            .map(|a| a.to_string())
                            .unwrap_or("unknown".to_string());
                        debug!(
                            "Accepted new incoming connection on {local_addr} from {peer_addr}."
                        );
                    }
                    Err(err) => {
                        debug!("Ignoring incoming connection with error: {err}");
                        continue;
                    }
                };
                let shared_data_inst = shared_data.clone();
                let now = Instant::now();
                let local_data = LocalData {
                    last_cap_peak_time: now,
                    last_cap_rms_time: now,
                    last_pb_peak_time: now,
                    last_pb_rms_time: now,
                };
                #[cfg(feature = "secure-websocket")]
                let acceptor_inst = acceptor.clone();

                #[cfg(feature = "secure-websocket")]
                thread::spawn(move || match acceptor_inst {
                    None => {
                        let websocket_res = accept_plain_stream(stream);
                        handle_tcp(websocket_res, &shared_data_inst, local_data);
                    }
                    Some(acc) => {
                        let websocket_res = accept_secure_stream(acc, stream);
                        handle_tls(websocket_res, &shared_data_inst, local_data);
                    }
                });
                #[cfg(not(feature = "secure-websocket"))]
                thread::spawn(move || {
                    let websocket_res = accept_plain_stream(stream);
                    handle_tcp(websocket_res, &shared_data_inst, local_data);
                });
            }
        } else if let Err(err) = ws_result {
            error!("Failed to start websocket server: {err}");
        }
    });
}

macro_rules! make_handler {
    ($t:ty, $n:ident) => {
        fn $n(
            websocket_res: Res<WebSocket<$t>>,
            shared_data_inst: &SharedData,
            mut local_data: LocalData,
        ) {
            match websocket_res {
                Ok(mut websocket) => {
                    set_stream_timeout(&mut websocket, None);
                    let mut active_stream: Option<ActiveStream> = None;
                    let mut last_playback_stream_generation = 0_u64;
                    let mut last_capture_stream_generation = 0_u64;
                    let mut last_state_stream_generation = 0_u64;
                    let mut last_state_stream_value = ProcessingState::Inactive;
                    loop {
                        if let Some(stream) = active_stream.as_mut() {
                            match stream {
                                ActiveStream::SignalLevels(side) => match *side {
                                    WsSignalLevelSide::Playback => {
                                        let generation = signal_monitor::generation(
                                            MonitorSignalLevelSide::Playback,
                                        );
                                        if generation != last_playback_stream_generation {
                                            last_playback_stream_generation = generation;
                                            if let Some(reply) = get_stream_levels_event(
                                                WsSignalLevelSide::Playback,
                                                shared_data_inst,
                                            ) {
                                                let write_result = websocket.send(Message::text(
                                                    serde_json::to_string(&reply).unwrap(),
                                                ));
                                                if let Err(err) = write_result {
                                                    warn!("Failed to write: {}", err);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    WsSignalLevelSide::Capture => {
                                        let generation = signal_monitor::generation(
                                            MonitorSignalLevelSide::Capture,
                                        );
                                        if generation != last_capture_stream_generation {
                                            last_capture_stream_generation = generation;
                                            if let Some(reply) = get_stream_levels_event(
                                                WsSignalLevelSide::Capture,
                                                shared_data_inst,
                                            ) {
                                                let write_result = websocket.send(Message::text(
                                                    serde_json::to_string(&reply).unwrap(),
                                                ));
                                                if let Err(err) = write_result {
                                                    warn!("Failed to write: {}", err);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    WsSignalLevelSide::Both => {
                                        let playback_generation = signal_monitor::generation(
                                            MonitorSignalLevelSide::Playback,
                                        );
                                        if playback_generation != last_playback_stream_generation {
                                            last_playback_stream_generation = playback_generation;
                                            if let Some(reply) = get_stream_levels_event(
                                                WsSignalLevelSide::Playback,
                                                shared_data_inst,
                                            ) {
                                                let write_result = websocket.send(Message::text(
                                                    serde_json::to_string(&reply).unwrap(),
                                                ));
                                                if let Err(err) = write_result {
                                                    warn!("Failed to write: {}", err);
                                                    break;
                                                }
                                            }
                                        }

                                        let capture_generation = signal_monitor::generation(
                                            MonitorSignalLevelSide::Capture,
                                        );
                                        if capture_generation != last_capture_stream_generation {
                                            last_capture_stream_generation = capture_generation;
                                            if let Some(reply) = get_stream_levels_event(
                                                WsSignalLevelSide::Capture,
                                                shared_data_inst,
                                            ) {
                                                let write_result = websocket.send(Message::text(
                                                    serde_json::to_string(&reply).unwrap(),
                                                ));
                                                if let Err(err) = write_result {
                                                    warn!("Failed to write: {}", err);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                },
                                ActiveStream::VuLevels(vu_state) => {
                                    let playback_generation = signal_monitor::generation(
                                        MonitorSignalLevelSide::Playback,
                                    );
                                    if playback_generation != last_playback_stream_generation {
                                        last_playback_stream_generation = playback_generation;
                                        if let Some((rms, peak)) = get_signal_levels_values_linear(
                                            WsSignalLevelSide::Playback,
                                            shared_data_inst,
                                        ) {
                                            vu_state.update_side(
                                                MonitorSignalLevelSide::Playback,
                                                rms,
                                                peak,
                                                Instant::now(),
                                            );
                                        }
                                    }

                                    let capture_generation =
                                        signal_monitor::generation(MonitorSignalLevelSide::Capture);
                                    if capture_generation != last_capture_stream_generation {
                                        last_capture_stream_generation = capture_generation;
                                        if let Some((rms, peak)) = get_signal_levels_values_linear(
                                            WsSignalLevelSide::Capture,
                                            shared_data_inst,
                                        ) {
                                            vu_state.update_side(
                                                MonitorSignalLevelSide::Capture,
                                                rms,
                                                peak,
                                                Instant::now(),
                                            );
                                        }
                                    }

                                    let now = Instant::now();
                                    if vu_state.should_publish_at(now) {
                                        let reply = vu_state.publish_at(now);
                                        let write_result = websocket.send(Message::text(
                                            serde_json::to_string(&reply).unwrap(),
                                        ));
                                        if let Err(err) = write_result {
                                            warn!("Failed to write: {}", err);
                                            break;
                                        }
                                    }
                                }
                                ActiveStream::State => {
                                    let generation = signal_monitor::wait_for_state_change(
                                        last_state_stream_generation,
                                        Duration::from_millis(SUBSCRIPTION_READ_TIMEOUT_MS),
                                    );
                                    if generation != last_state_stream_generation {
                                        last_state_stream_generation = generation;
                                        let current_state = current_processing_state(shared_data_inst);
                                        if current_state != last_state_stream_value {
                                            last_state_stream_value = current_state;
                                            let reply =
                                                get_state_event(current_state, shared_data_inst);
                                            let write_result = websocket.send(Message::text(
                                                serde_json::to_string(&reply).unwrap(),
                                            ));
                                            if let Err(err) = write_result {
                                                warn!("Failed to write: {}", err);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let msg_res = websocket.read();
                        match msg_res {
                            Ok(msg) => {
                                trace!("received: {:?}", msg);
                                let command = parse_command(msg);
                                debug!("parsed command: {:?}", command);
                                let reply = match command {
                                    Ok(cmd) => {
                                        if active_stream.is_some() {
                                            if cmd != WsCommand::StopSubscription {
                                                Some(stream_invalid_reply())
                                            } else {
                                                active_stream = None;
                                                set_stream_timeout(&mut websocket, None);
                                                Some(WsReply::StopSubscription {
                                                    result: WsResult::Ok,
                                                })
                                            }
                                        } else {
                                            match cmd {
                                                WsCommand::SubscribeSignalLevels(side) => {
                                                    active_stream =
                                                        Some(ActiveStream::SignalLevels(side));
                                                    set_stream_timeout(
                                                        &mut websocket,
                                                        Some(Duration::from_millis(
                                                            SUBSCRIPTION_READ_TIMEOUT_MS,
                                                        )),
                                                    );
                                                    last_playback_stream_generation =
                                                        signal_monitor::generation(
                                                            MonitorSignalLevelSide::Playback,
                                                        );
                                                    last_capture_stream_generation =
                                                        signal_monitor::generation(
                                                            MonitorSignalLevelSide::Capture,
                                                        );
                                                    Some(WsReply::SubscribeSignalLevels {
                                                        result: WsResult::Ok,
                                                    })
                                                }
                                                WsCommand::SubscribeVuLevels(config) => {
                                                    match validate_vu_subscription(config) {
                                                        Ok(config) => {
                                                            let mut vu_state =
                                                                VuSubscriptionState::new(config);
                                                            vu_state.seed_from_shared(
                                                                shared_data_inst,
                                                            );
                                                            active_stream = Some(
                                                                ActiveStream::VuLevels(vu_state),
                                                            );
                                                            set_stream_timeout(
                                                                &mut websocket,
                                                                Some(Duration::from_millis(
                                                                    SUBSCRIPTION_READ_TIMEOUT_MS,
                                                                )),
                                                            );
                                                            last_playback_stream_generation =
                                                                signal_monitor::generation(
                                                                    MonitorSignalLevelSide::Playback,
                                                                );
                                                            last_capture_stream_generation =
                                                                signal_monitor::generation(
                                                                    MonitorSignalLevelSide::Capture,
                                                                );
                                                            Some(WsReply::SubscribeVuLevels {
                                                                result: WsResult::Ok,
                                                            })
                                                        }
                                                        Err(result) => {
                                                            Some(WsReply::SubscribeVuLevels {
                                                                result,
                                                            })
                                                        }
                                                    }
                                                }
                                                WsCommand::SubscribeState => {
                                                    active_stream = Some(ActiveStream::State);
                                                    set_stream_timeout(
                                                        &mut websocket,
                                                        Some(Duration::from_millis(
                                                            SUBSCRIPTION_READ_TIMEOUT_MS,
                                                        )),
                                                    );
                                                    last_state_stream_generation =
                                                        signal_monitor::state_generation();
                                                    last_state_stream_value =
                                                        current_processing_state(shared_data_inst);
                                                    Some(WsReply::SubscribeState {
                                                        result: WsResult::Ok,
                                                    })
                                                }
                                                WsCommand::StopSubscription => {
                                                    Some(WsReply::Invalid {
                                                        error: "No active subscription".to_string(),
                                                    })
                                                }
                                                _ => handle_command(
                                                    cmd,
                                                    &shared_data_inst,
                                                    &mut local_data,
                                                ),
                                            }
                                        }
                                    }
                                    Err(err) => Some(WsReply::Invalid {
                                        error: err.to_string(),
                                    }),
                                };
                                if let Some(rep) = reply {
                                    let write_result = websocket
                                        .send(Message::text(serde_json::to_string(&rep).unwrap()));
                                    if let Err(err) = write_result {
                                        warn!("Failed to write: {}", err);
                                        break;
                                    }
                                } else {
                                    debug!("Sending no reply");
                                }
                            }
                            Err(tungstenite::error::Error::ConnectionClosed) => {
                                debug!("Connection was closed");
                                break;
                            }
                            Err(err) if is_timeout_error(&err) => {
                                continue;
                            }
                            Err(err) => {
                                warn!("Lost connection: {}", err);
                                break;
                            }
                        }
                    }
                }
                Err(err) => warn!("Connection failed: {}", err),
            };
        }
    };
}

make_handler!(TcpStream, handle_tcp);
#[cfg(feature = "secure-websocket")]
make_handler!(TlsStream<TcpStream>, handle_tls);

fn handle_command(
    command: WsCommand,
    shared_data_inst: &SharedData,
    local_data: &mut LocalData,
) -> Option<WsReply> {
    match command {
        WsCommand::Reload => {
            let cfg_path = shared_data_inst.active_config_path.lock().clone();
            match cfg_path {
                Some(path) => match config::load_config(path.as_str()) {
                    Ok(mut conf) => match config::validate_config(&mut conf, Some(path.as_str())) {
                        Ok(()) => {
                            debug!("WS: Config file loaded successfully, send to controller");
                            match shared_data_inst
                                .command_sender
                                .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                            {
                                Ok(()) => Some(WsReply::Reload {
                                    result: WsResult::Ok,
                                }),
                                Err(TrySendError::Full(_)) => {
                                    debug!("Error sending reload message, too many requests");
                                    Some(WsReply::Reload {
                                        result: WsResult::RateLimitExceededError,
                                    })
                                }
                                Err(TrySendError::Disconnected(_)) => {
                                    debug!(
                                        "Error sending reload message, channel was disconnected"
                                    );
                                    Some(WsReply::Reload {
                                        result: WsResult::ShutdownInProgressError,
                                    })
                                }
                            }
                        }
                        Err(err) => {
                            debug!("Invalid config file: {err}");
                            Some(WsReply::Reload {
                                result: WsResult::ConfigReadError(err.to_string()),
                            })
                        }
                    },
                    Err(err) => {
                        debug!("Config file validation error: {err}");
                        Some(WsReply::Reload {
                            result: WsResult::ConfigValidationError(err.to_string()),
                        })
                    }
                },
                None => {
                    warn!("Config path not given, cannot reload");
                    Some(WsReply::Reload {
                        result: WsResult::InvalidRequestError(
                            "Config path not given, cannot reload".to_string(),
                        ),
                    })
                }
            }
        }
        WsCommand::GetCaptureRate => {
            let capstat = shared_data_inst.capture_status.read();
            Some(WsReply::GetCaptureRate {
                result: WsResult::Ok,
                value: capstat.measured_samplerate,
            })
        }
        WsCommand::GetSignalRange => {
            let capstat = shared_data_inst.capture_status.read();
            Some(WsReply::GetSignalRange {
                result: WsResult::Ok,
                value: capstat.signal_range,
            })
        }
        WsCommand::GetCaptureSignalRms => {
            let values = capture_signal_rms(shared_data_inst);
            Some(WsReply::GetCaptureSignalRms {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalRmsSince(secs) => {
            let values = capture_signal_rms_since(shared_data_inst, secs);
            Some(WsReply::GetCaptureSignalRmsSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalRmsSinceLast => {
            let values = capture_signal_rms_since_last(shared_data_inst, local_data);
            Some(WsReply::GetCaptureSignalRmsSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRms => {
            let values = playback_signal_rms(shared_data_inst);
            Some(WsReply::GetPlaybackSignalRms {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRmsSince(secs) => {
            let values = playback_signal_rms_since(shared_data_inst, secs);
            Some(WsReply::GetPlaybackSignalRmsSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRmsSinceLast => {
            let values = playback_signal_rms_since_last(shared_data_inst, local_data);
            Some(WsReply::GetPlaybackSignalRmsSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeak => {
            let values = capture_signal_peak(shared_data_inst);
            Some(WsReply::GetCaptureSignalPeak {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeakSince(secs) => {
            let values = capture_signal_peak_since(shared_data_inst, secs);
            Some(WsReply::GetCaptureSignalPeakSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeakSinceLast => {
            let values = capture_signal_peak_since_last(shared_data_inst, local_data);
            Some(WsReply::GetCaptureSignalPeakSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeak => {
            let values = playback_signal_peak(shared_data_inst);
            Some(WsReply::GetPlaybackSignalPeak {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeakSince(secs) => {
            let values = playback_signal_peak_since(shared_data_inst, secs);
            Some(WsReply::GetPlaybackSignalPeakSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeakSinceLast => {
            let values = playback_signal_peak_since_last(shared_data_inst, local_data);
            Some(WsReply::GetPlaybackSignalPeakSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetSignalLevels => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms(shared_data_inst),
                playback_peak: playback_signal_peak(shared_data_inst),
                capture_rms: capture_signal_rms(shared_data_inst),
                capture_peak: capture_signal_peak(shared_data_inst),
            };
            let result = WsReply::GetSignalLevels {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::GetSignalLevelsSince(secs) => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms_since(shared_data_inst, secs),
                playback_peak: playback_signal_peak_since(shared_data_inst, secs),
                capture_rms: capture_signal_rms_since(shared_data_inst, secs),
                capture_peak: capture_signal_peak_since(shared_data_inst, secs),
            };
            let result = WsReply::GetSignalLevelsSince {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::GetSignalLevelsSinceLast => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms_since_last(shared_data_inst, local_data),
                playback_peak: playback_signal_peak_since_last(shared_data_inst, local_data),
                capture_rms: capture_signal_rms_since_last(shared_data_inst, local_data),
                capture_peak: capture_signal_peak_since_last(shared_data_inst, local_data),
            };
            let result = WsReply::GetSignalLevelsSinceLast {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::SubscribeSignalLevels(_) => Some(WsReply::Invalid {
            error: "SubscribeSignalLevels can only be handled by the websocket stream loop"
                .to_string(),
        }),
        WsCommand::SubscribeVuLevels(_) => Some(WsReply::Invalid {
            error: "SubscribeVuLevels can only be handled by the websocket stream loop".to_string(),
        }),
        WsCommand::SubscribeState => Some(WsReply::Invalid {
            error: "SubscribeState can only be handled by the websocket stream loop".to_string(),
        }),
        WsCommand::StopSubscription => Some(WsReply::Invalid {
            error: "No active subscription".to_string(),
        }),
        WsCommand::GetSignalPeaksSinceStart => {
            let levels = PbCapLevels {
                playback: playback_signal_global_peak(shared_data_inst),
                capture: capture_signal_global_peak(shared_data_inst),
            };
            let result = WsReply::GetSignalPeaksSinceStart {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::ResetSignalPeaksSinceStart => {
            reset_playback_signal_global_peak(shared_data_inst);
            reset_capture_signal_global_peak(shared_data_inst);
            let result = WsReply::ResetSignalPeaksSinceStart {
                result: WsResult::Ok,
            };
            Some(result)
        }
        WsCommand::GetChannelLabels => {
            let optional_config = shared_data_inst.active_config.lock();
            Some(WsReply::GetChannelLabels {
                result: WsResult::Ok,
                value: ChannelLabels {
                    playback: config::playback_channel_labels(&optional_config),
                    capture: config::capture_channel_labels(&optional_config),
                },
            })
        }
        WsCommand::GetVersion => Some(WsReply::GetVersion {
            result: WsResult::Ok,
            value: crate_version!().to_string(),
        }),
        WsCommand::GetState => {
            let capstat = shared_data_inst.capture_status.read();
            Some(WsReply::GetState {
                result: WsResult::Ok,
                value: capstat.state,
            })
        }
        WsCommand::GetStopReason => {
            let stat = shared_data_inst.processing_status.read();
            let value = stat.stop_reason.clone();
            Some(WsReply::GetStopReason {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetRateAdjust => {
            let capstat = shared_data_inst.capture_status.read();
            Some(WsReply::GetRateAdjust {
                result: WsResult::Ok,
                value: capstat.rate_adjust,
            })
        }
        WsCommand::GetClippedSamples => {
            let pbstat = shared_data_inst.playback_status.read();
            Some(WsReply::GetClippedSamples {
                result: WsResult::Ok,
                value: pbstat.clipped_samples,
            })
        }
        WsCommand::ResetClippedSamples => {
            let mut pbstat = shared_data_inst.playback_status.write();
            pbstat.clipped_samples = 0;
            Some(WsReply::ResetClippedSamples {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetBufferLevel => {
            let pbstat = shared_data_inst.playback_status.read();
            Some(WsReply::GetBufferLevel {
                result: WsResult::Ok,
                value: pbstat.buffer_level,
            })
        }
        WsCommand::GetUpdateInterval => {
            let capstat = shared_data_inst.capture_status.read();
            Some(WsReply::GetUpdateInterval {
                result: WsResult::Ok,
                value: capstat.update_interval,
            })
        }
        WsCommand::SetUpdateInterval(nbr) => {
            {
                let mut captstat = shared_data_inst.capture_status.write();
                let mut playstat = shared_data_inst.playback_status.write();
                captstat.update_interval = nbr;
                playstat.update_interval = nbr;
            }
            Some(WsReply::SetUpdateInterval {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetVolume => Some(WsReply::GetVolume {
            result: WsResult::Ok,
            value: shared_data_inst.processing_params.target_volume(0),
        }),
        WsCommand::SetVolume(nbr) => {
            let new_vol = clamped_volume(nbr);
            shared_data_inst
                .processing_params
                .set_target_volume(0, new_vol);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::SetVolume {
                result: WsResult::Ok,
            })
        }
        WsCommand::AdjustVolume(value) => {
            let mut tempvol = shared_data_inst.processing_params.target_volume(0);
            let (volchange, minvol, maxvol) = match value {
                ValueWithOptionalLimits::Plain(vol) => (vol, -150.0, 50.0),
                ValueWithOptionalLimits::Limited(vol, min, max) => (vol, min, max),
            };
            if maxvol < minvol {
                return Some(WsReply::AdjustVolume {
                    result: WsResult::InvalidValueError(
                        "Max volume must be bigger than min volume".to_string(),
                    ),
                    value: tempvol,
                });
            }
            tempvol += volchange;
            if tempvol < minvol {
                tempvol = minvol;
                warn!("Clamped volume at {minvol} dB")
            }
            if tempvol > maxvol {
                tempvol = maxvol;
                warn!("Clamped volume at {maxvol} dB")
            }

            shared_data_inst
                .processing_params
                .set_target_volume(0, tempvol);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::AdjustVolume {
                result: WsResult::Ok,
                value: tempvol,
            })
        }
        WsCommand::GetMute => Some(WsReply::GetMute {
            result: WsResult::Ok,
            value: shared_data_inst.processing_params.is_mute(0),
        }),
        WsCommand::SetMute(mute) => {
            shared_data_inst.processing_params.set_mute(0, mute);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::SetMute {
                result: WsResult::Ok,
            })
        }
        WsCommand::ToggleMute => {
            let tempmute = shared_data_inst.processing_params.toggle_mute(0);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::ToggleMute {
                result: WsResult::Ok,
                value: !tempmute,
            })
        }
        WsCommand::GetFaders => {
            let volumes = shared_data_inst.processing_params.volumes();
            let mutes = shared_data_inst.processing_params.mutes();
            let faders = volumes
                .iter()
                .zip(mutes)
                .map(|(v, m)| Fader {
                    volume: *v,
                    mute: m,
                })
                .collect();
            Some(WsReply::GetFaders {
                result: WsResult::Ok,
                value: faders,
            })
        }
        WsCommand::GetFaderVolume(ctrl) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::GetFaderVolume {
                    result: WsResult::InvalidFaderError,
                    value: (ctrl, ProcessingParameters::DEFAULT_VOLUME),
                });
            }
            Some(WsReply::GetFaderVolume {
                result: WsResult::Ok,
                value: (ctrl, shared_data_inst.processing_params.target_volume(ctrl)),
            })
        }
        WsCommand::SetFaderVolume(ctrl, nbr) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::SetFaderVolume {
                    result: WsResult::InvalidFaderError,
                });
            }
            let new_vol = clamped_volume(nbr);
            shared_data_inst
                .processing_params
                .set_target_volume(ctrl, new_vol);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::SetFaderVolume {
                result: WsResult::Ok,
            })
        }
        WsCommand::SetFaderExternalVolume(ctrl, nbr) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::SetFaderExternalVolume {
                    result: WsResult::InvalidFaderError,
                });
            }
            let new_vol = clamped_volume(nbr);
            shared_data_inst
                .processing_params
                .set_target_volume(ctrl, new_vol);
            shared_data_inst
                .processing_params
                .set_current_volume(ctrl, new_vol);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::SetFaderExternalVolume {
                result: WsResult::Ok,
            })
        }
        WsCommand::AdjustFaderVolume(ctrl, value) => {
            let (volchange, minvol, maxvol) = match value {
                ValueWithOptionalLimits::Plain(vol) => (vol, -150.0, 50.0),
                ValueWithOptionalLimits::Limited(vol, min, max) => (vol, min, max),
            };
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::AdjustFaderVolume {
                    result: WsResult::InvalidFaderError,
                    value: (ctrl, volchange),
                });
            }
            let mut tempvol = shared_data_inst.processing_params.target_volume(ctrl);
            if maxvol < minvol {
                return Some(WsReply::AdjustFaderVolume {
                    result: WsResult::InvalidValueError(
                        "Max volume must be bigger than min volume".to_string(),
                    ),
                    value: (ctrl, tempvol),
                });
            }
            tempvol += volchange;
            if tempvol < minvol {
                tempvol = minvol;
                warn!("Clamped volume at {minvol} dB")
            }
            if tempvol > maxvol {
                tempvol = maxvol;
                warn!("Clamped volume at {maxvol} dB")
            }
            shared_data_inst
                .processing_params
                .set_target_volume(ctrl, tempvol);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::AdjustFaderVolume {
                result: WsResult::Ok,
                value: (ctrl, tempvol),
            })
        }
        WsCommand::GetFaderMute(ctrl) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::GetFaderMute {
                    result: WsResult::InvalidFaderError,
                    value: (ctrl, ProcessingParameters::DEFAULT_MUTE),
                });
            }
            Some(WsReply::GetFaderMute {
                result: WsResult::Ok,
                value: (ctrl, shared_data_inst.processing_params.is_mute(ctrl)),
            })
        }
        WsCommand::SetFaderMute(ctrl, mute) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::SetFaderMute {
                    result: WsResult::InvalidFaderError,
                });
            }
            shared_data_inst.processing_params.set_mute(ctrl, mute);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::SetFaderMute {
                result: WsResult::Ok,
            })
        }
        WsCommand::ToggleFaderMute(ctrl) => {
            if ctrl > ProcessingParameters::NUM_FADERS - 1 {
                return Some(WsReply::ToggleFaderMute {
                    result: WsResult::InvalidFaderError,
                    value: (ctrl, ProcessingParameters::DEFAULT_MUTE),
                });
            }
            let tempmute = shared_data_inst.processing_params.toggle_mute(ctrl);
            shared_data_inst
                .unsaved_state_change
                .store(true, Ordering::Relaxed);
            shared_data_inst
                .state_change_notify
                .try_send(())
                .unwrap_or(());
            Some(WsReply::ToggleFaderMute {
                result: WsResult::Ok,
                value: (ctrl, !tempmute),
            })
        }
        WsCommand::GetConfig => Some(WsReply::GetConfig {
            result: WsResult::Ok,
            value: yaml_serde::to_string(&*shared_data_inst.active_config.lock()).unwrap(),
        }),
        WsCommand::GetConfigValue(pointer) => {
            let conf_as_value =
                serde_json::to_value(&*shared_data_inst.active_config.lock()).unwrap();
            let value = conf_as_value.pointer(&pointer);
            match value {
                Some(v) => Some(WsReply::GetConfigValue {
                    result: WsResult::Ok,
                    value: v.clone(),
                }),
                None => Some(WsReply::GetConfigValue {
                    result: WsResult::InvalidRequestError(format!(
                        "The path '{pointer}' does not exit in the config"
                    )),
                    value: serde_json::Value::Null,
                }),
            }
        }
        WsCommand::GetConfigTitle => {
            let optional_config = shared_data_inst.active_config.lock();
            let value = if let Some(config) = &*optional_config {
                config.title.clone().unwrap_or_default()
            } else {
                String::new()
            };
            Some(WsReply::GetConfigTitle {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetConfigDescription => {
            let optional_config = shared_data_inst.active_config.lock();
            let value = if let Some(config) = &*optional_config {
                config.description.clone().unwrap_or_default()
            } else {
                String::new()
            };
            Some(WsReply::GetConfigDescription {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetPreviousConfig => Some(WsReply::GetPreviousConfig {
            result: WsResult::Ok,
            value: yaml_serde::to_string(&*shared_data_inst.previous_config.lock()).unwrap(),
        }),
        WsCommand::GetConfigJson => Some(WsReply::GetConfigJson {
            result: WsResult::Ok,
            value: serde_json::to_string(&*shared_data_inst.active_config.lock()).unwrap(),
        }),
        WsCommand::GetConfigFilePath => Some(WsReply::GetConfigFilePath {
            result: WsResult::Ok,
            value: shared_data_inst
                .active_config_path
                .lock()
                .as_ref()
                .map(|s| s.to_string()),
        }),
        WsCommand::GetStateFilePath => Some(WsReply::GetStateFilePath {
            result: WsResult::Ok,
            value: shared_data_inst.state_file_path.clone(),
        }),
        WsCommand::GetStateFileUpdated => Some(WsReply::GetStateFileUpdated {
            result: WsResult::Ok,
            value: !shared_data_inst
                .unsaved_state_change
                .load(Ordering::Relaxed),
        }),
        WsCommand::SetConfigFilePath(path) => match config::load_validate_config(&path) {
            Ok(_) => {
                *shared_data_inst.active_config_path.lock() = Some(path.clone());
                shared_data_inst
                    .unsaved_state_change
                    .store(true, Ordering::Relaxed);
                shared_data_inst
                    .state_change_notify
                    .try_send(())
                    .unwrap_or(());
                Some(WsReply::SetConfigFilePath {
                    result: WsResult::Ok,
                })
            }
            Err(error) => {
                debug!("Error setting config name: {error}");
                Some(WsReply::SetConfigFilePath {
                    result: WsResult::InvalidValueError(error.to_string()),
                })
            }
        },
        WsCommand::SetConfig(config_yml) => {
            match yaml_serde::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        match shared_data_inst
                            .command_sender
                            .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                        {
                            Ok(()) => Some(WsReply::SetConfig {
                                result: WsResult::Ok,
                            }),
                            Err(TrySendError::Full(_)) => {
                                debug!("Error sending new config, too many requests");
                                Some(WsReply::SetConfig {
                                    result: WsResult::RateLimitExceededError,
                                })
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                debug!("Error sending new config, channel was disconnected");
                                Some(WsReply::SetConfig {
                                    result: WsResult::ShutdownInProgressError,
                                })
                            }
                        }
                    }
                    Err(error) => {
                        debug!("Error validating config: {error}");
                        Some(WsReply::SetConfig {
                            result: WsResult::ConfigValidationError(error.to_string()),
                        })
                    }
                },
                Err(error) => {
                    debug!("Error parsing yaml: {error}");
                    Some(WsReply::SetConfig {
                        result: WsResult::ConfigReadError(error.to_string()),
                    })
                }
            }
        }
        WsCommand::SetConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        match shared_data_inst
                            .command_sender
                            .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                        {
                            Ok(()) => Some(WsReply::SetConfigJson {
                                result: WsResult::Ok,
                            }),
                            Err(TrySendError::Full(_)) => {
                                debug!("Error sending new config, too many requests");
                                Some(WsReply::SetConfigJson {
                                    result: WsResult::RateLimitExceededError,
                                })
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                debug!("Error sending new config, channel was disconnected");
                                Some(WsReply::SetConfigJson {
                                    result: WsResult::ShutdownInProgressError,
                                })
                            }
                        }
                    }
                    Err(error) => {
                        debug!("Error validating config: {error}");
                        Some(WsReply::SetConfigJson {
                            result: WsResult::ConfigValidationError(error.to_string()),
                        })
                    }
                },
                Err(error) => {
                    debug!("Error parsing json: {error}");
                    Some(WsReply::SetConfigJson {
                        result: WsResult::ConfigReadError(error.to_string()),
                    })
                }
            }
        }
        WsCommand::PatchConfig(value) => {
            let mut conf_as_value =
                serde_json::to_value(&*shared_data_inst.active_config.lock()).unwrap();
            if conf_as_value.is_null() {
                debug!("No active config to patch");
                return Some(WsReply::PatchConfig {
                    result: WsResult::InvalidRequestError("No active config to patch".to_string()),
                });
            }
            merge(&mut conf_as_value, &value);
            let updated_conf = serde_json::from_value::<config::Configuration>(conf_as_value);
            match updated_conf {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        match shared_data_inst
                            .command_sender
                            .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                        {
                            Ok(()) => Some(WsReply::PatchConfig {
                                result: WsResult::Ok,
                            }),
                            Err(TrySendError::Full(_)) => {
                                debug!("Error patching config, too many requests");
                                Some(WsReply::PatchConfig {
                                    result: WsResult::RateLimitExceededError,
                                })
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                debug!("Error patching config, channel was disconnected");
                                Some(WsReply::PatchConfig {
                                    result: WsResult::ShutdownInProgressError,
                                })
                            }
                        }
                    }
                    Err(error) => {
                        debug!("Error validating patched config: {error}");
                        Some(WsReply::PatchConfig {
                            result: WsResult::ConfigValidationError(error.to_string()),
                        })
                    }
                },
                Err(error) => {
                    debug!("Error parsing patched config: {error}");
                    Some(WsReply::PatchConfig {
                        result: WsResult::ConfigReadError(error.to_string()),
                    })
                }
            }
        }
        WsCommand::SetConfigValue(pointer, value) => {
            let mut conf_as_value =
                serde_json::to_value(&*shared_data_inst.active_config.lock()).unwrap();
            if conf_as_value.is_null() {
                debug!("No active config to patch");
                return Some(WsReply::SetConfigValue {
                    result: WsResult::InvalidRequestError("No active config to modify".to_string()),
                });
            }
            let maybe_config_value = conf_as_value.pointer_mut(&pointer);
            if let Some(config_value) = maybe_config_value {
                *config_value = value;
            } else {
                return Some(WsReply::SetConfigValue {
                    result: WsResult::InvalidRequestError(
                        "The active config does not contain the path '{}'".to_string(),
                    ),
                });
            }
            let updated_conf = serde_json::from_value::<config::Configuration>(conf_as_value);
            match updated_conf {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        match shared_data_inst
                            .command_sender
                            .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                        {
                            Ok(()) => Some(WsReply::SetConfigValue {
                                result: WsResult::Ok,
                            }),
                            Err(TrySendError::Full(_)) => {
                                debug!("Error patching config, too many requests");
                                Some(WsReply::SetConfigValue {
                                    result: WsResult::RateLimitExceededError,
                                })
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                debug!("Error patching config, channel was disconnected");
                                Some(WsReply::SetConfigValue {
                                    result: WsResult::ShutdownInProgressError,
                                })
                            }
                        }
                    }
                    Err(error) => {
                        debug!("Error validating patched config: {error}");
                        Some(WsReply::SetConfigValue {
                            result: WsResult::ConfigValidationError(error.to_string()),
                        })
                    }
                },
                Err(error) => {
                    debug!("Error parsing patched config: {error}");
                    Some(WsReply::SetConfigValue {
                        result: WsResult::ConfigReadError(error.to_string()),
                    })
                }
            }
        }
        WsCommand::ReadConfig(config_yml) => {
            match yaml_serde::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => Some(WsReply::ReadConfig {
                    result: WsResult::Ok,
                    value: yaml_serde::to_string(&conf).unwrap(),
                }),
                Err(error) => {
                    debug!("Error reading config: {error}");
                    Some(WsReply::ReadConfig {
                        result: WsResult::ConfigReadError(error.to_string()),
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::ReadConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(conf) => Some(WsReply::ReadConfigJson {
                    result: WsResult::Ok,
                    value: serde_json::to_string(&conf).unwrap(),
                }),
                Err(error) => {
                    error!("Error reading config: {}", error);
                    Some(WsReply::ReadConfigJson {
                        result: WsResult::ConfigReadError(error.to_string()),
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::ReadConfigFile(path) => match config::load_config(&path) {
            Ok(conf) => Some(WsReply::ReadConfigFile {
                result: WsResult::Ok,
                value: yaml_serde::to_string(&conf).unwrap(),
            }),
            Err(error) => {
                debug!("Error reading config file: {error}");
                Some(WsReply::ReadConfigFile {
                    result: WsResult::ConfigReadError(error.to_string()),
                    value: error.to_string(),
                })
            }
        },
        WsCommand::ValidateConfig(config_yml) => {
            match yaml_serde::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => Some(WsReply::ValidateConfig {
                        result: WsResult::Ok,
                        value: yaml_serde::to_string(&conf).unwrap(),
                    }),
                    Err(error) => {
                        debug!("Config error: {error}");
                        Some(WsReply::ValidateConfig {
                            result: WsResult::ConfigValidationError(error.to_string()),
                            value: error.to_string(),
                        })
                    }
                },
                Err(error) => {
                    debug!("Config error: {error}");
                    Some(WsReply::ValidateConfig {
                        result: WsResult::ConfigReadError(error.to_string()),
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::ValidateConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => Some(WsReply::ValidateConfigJson {
                        result: WsResult::Ok,
                        value: serde_json::to_string(&conf).unwrap(),
                    }),
                    Err(error) => {
                        debug!("Config error: {error}");
                        Some(WsReply::ValidateConfigJson {
                            result: WsResult::ConfigValidationError(error.to_string()),
                            value: error.to_string(),
                        })
                    }
                },
                Err(error) => {
                    debug!("Config error: {error}");
                    Some(WsReply::ValidateConfigJson {
                        result: WsResult::ConfigReadError(error.to_string()),
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::Stop => {
            match shared_data_inst
                .command_sender
                .try_send(ControllerMessage::Stop)
            {
                Ok(()) => Some(WsReply::Stop {
                    result: WsResult::Ok,
                }),
                Err(TrySendError::Full(_)) => {
                    debug!("Error sending stop message, too many requests");
                    Some(WsReply::Stop {
                        result: WsResult::RateLimitExceededError,
                    })
                }
                Err(TrySendError::Disconnected(_)) => {
                    debug!("Error sending stop message, channel was disconnected");
                    Some(WsReply::Stop {
                        result: WsResult::ShutdownInProgressError,
                    })
                }
            }
        }
        WsCommand::Exit => {
            match shared_data_inst
                .command_sender
                .try_send(ControllerMessage::Exit)
            {
                Ok(()) => Some(WsReply::Exit {
                    result: WsResult::Ok,
                }),
                Err(TrySendError::Full(_)) => {
                    debug!("Error sending exit message, too many requests");
                    Some(WsReply::Exit {
                        result: WsResult::RateLimitExceededError,
                    })
                }
                Err(TrySendError::Disconnected(_)) => {
                    debug!("Error sending exit message, channel was disconnected");
                    Some(WsReply::Exit {
                        result: WsResult::ShutdownInProgressError,
                    })
                }
            }
        }
        WsCommand::GetSupportedDeviceTypes => {
            let devs = list_supported_devices();
            Some(WsReply::GetSupportedDeviceTypes {
                result: WsResult::Ok,
                value: devs,
            })
        }
        WsCommand::GetAvailableCaptureDevices(backend) => {
            let devs = list_available_devices(&backend, true);
            Some(WsReply::GetAvailableCaptureDevices {
                result: WsResult::Ok,
                value: devs,
            })
        }
        WsCommand::GetAvailablePlaybackDevices(backend) => {
            let devs = list_available_devices(&backend, false);
            Some(WsReply::GetAvailablePlaybackDevices {
                result: WsResult::Ok,
                value: devs,
            })
        }
        WsCommand::GetProcessingLoad => {
            let load = shared_data_inst.processing_params.processing_load();
            Some(WsReply::GetProcessingLoad {
                result: WsResult::Ok,
                value: load,
            })
        }
        WsCommand::GetResamplerLoad => {
            let load = shared_data_inst.processing_params.resampler_load();
            Some(WsReply::GetResamplerLoad {
                result: WsResult::Ok,
                value: load,
            })
        }
        WsCommand::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::VuSubscriptionState;
    use super::datastructures::{VuSubscription, WsCommand, WsResult, WsSignalLevelSide};
    use super::utils::{parse_command, validate_vu_subscription};
    use crate::signal_monitor::SignalLevelSide as MonitorSignalLevelSide;
    use std::time::{Duration, Instant};
    use tungstenite::Message;

    #[test]
    fn parse_commands() {
        let cmd = Message::text("\"Reload\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("{\"SetConfigFilePath\": \"somefile\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::SetConfigFilePath("somefile".to_string()));
        let cmd = Message::text("{\"SubscribeSignalLevels\": \"playback\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(
            res,
            WsCommand::SubscribeSignalLevels(WsSignalLevelSide::Playback)
        );
        let cmd = Message::text("\"StopSubscription\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::StopSubscription);
        let cmd = Message::text("{\"SubscribeSignalLevels\": \"both\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(
            res,
            WsCommand::SubscribeSignalLevels(WsSignalLevelSide::Both)
        );
        let cmd = Message::text(
            "{\"SubscribeVuLevels\": {\"max_rate\": 30.0, \"attack\": 10.0, \"release\": 200.0}}",
        );
        let res = parse_command(cmd).unwrap();
        assert_eq!(
            res,
            WsCommand::SubscribeVuLevels(VuSubscription {
                max_rate: 30.0,
                attack: 10.0,
                release: 200.0,
            })
        );
        let cmd = Message::text("\"SubscribeState\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::SubscribeState);
    }

    #[test]
    fn vu_levels_smoothing_and_rate_limit() {
        let mut state = VuSubscriptionState::new(VuSubscription {
            max_rate: 10.0,
            attack: 100.0,
            release: 200.0,
        });
        let start = Instant::now();

        state.update_side(
            MonitorSignalLevelSide::Playback,
            vec![0.1],
            vec![0.31622776],
            start,
        );
        assert!(state.should_publish_at(start));
        let reply = state.publish_at(start);
        match reply {
            super::WsReply::VuLevelsEvent { value, .. } => {
                assert!((value.playback_rms[0] + 20.0).abs() < 0.1);
                assert!((value.playback_peak[0] + 10.0).abs() < 0.1);
            }
            _ => panic!("unexpected reply type"),
        }

        let first_update = start + Duration::from_millis(50);
        state.update_side(
            MonitorSignalLevelSide::Playback,
            vec![1.0],
            vec![1.0],
            first_update,
        );
        assert!(!state.should_publish_at(first_update));
        assert!(state.playback.rms[0] > 0.1);
        assert!(state.playback.rms[0] < 1.0);
        assert_eq!(state.playback.peak[0], 1.0);

        let second_update = start + Duration::from_millis(120);
        assert!(state.should_publish_at(second_update));
        let _ = state.publish_at(second_update);

        state.update_side(
            MonitorSignalLevelSide::Playback,
            vec![0.01],
            vec![0.031622775],
            second_update + Duration::from_millis(100),
        );
        assert!(state.playback.rms[0] > 0.01);
        assert!(state.playback.peak[0] > 0.031622775);
    }

    #[test]
    fn vu_subscription_limits_are_validated() {
        assert_eq!(
            validate_vu_subscription(VuSubscription {
                max_rate: 30.0,
                attack: -1.0,
                release: 100.0,
            }),
            Err(WsResult::InvalidValueError(
                "attack must be between 0 and 60000 ms".to_string()
            ))
        );

        assert_eq!(
            validate_vu_subscription(VuSubscription {
                max_rate: 30.0,
                attack: 100.0,
                release: 60000.1,
            }),
            Err(WsResult::InvalidValueError(
                "release must be between 0 and 60000 ms".to_string()
            ))
        );

        assert_eq!(
            validate_vu_subscription(VuSubscription {
                max_rate: 30.0,
                attack: 0.0,
                release: 60_000.0,
            }),
            Ok(VuSubscription {
                max_rate: 30.0,
                attack: 0.0,
                release: 60_000.0,
            })
        );
    }
}
