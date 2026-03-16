use clap::crate_version;
use crossbeam_channel::TrySendError;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::helpers::linear_to_db;
use crate::{config, ControllerMessage};
use crate::{
    list_available_devices, list_supported_devices, CaptureStatus, PlaybackStatus,
    ProcessingParameters, ProcessingStatus,
};

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
    pub state_change_notify: mpsc::SyncSender<()>,
    pub state_file_path: Option<String>,
    pub unsaved_state_change: Arc<AtomicBool>,
}

#[derive(Debug, Serialize)]
struct ApiResponse<T: Serialize> {
    result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn ok_response<T: Serialize>(value: T) -> ApiResponse<T> {
    ApiResponse {
        result: "Ok",
        value: Some(value),
        message: None,
    }
}

fn ok_empty() -> ApiResponse<()> {
    ApiResponse {
        result: "Ok",
        value: None,
        message: None,
    }
}

fn error_response(msg: String) -> ApiResponse<()> {
    ApiResponse {
        result: "Error",
        value: None,
        message: Some(msg),
    }
}

fn json_ok<T: Serialize>(value: T) -> rouille::Response {
    rouille::Response::json(&ok_response(value))
}

fn json_ok_empty() -> rouille::Response {
    rouille::Response::json(&ok_empty())
}

fn json_error(status: u16, msg: String) -> rouille::Response {
    rouille::Response::json(&error_response(msg)).with_status_code(status)
}

fn read_json_body<T: for<'de> Deserialize<'de>>(request: &rouille::Request) -> Result<T, String> {
    let mut body = String::new();
    request
        .data()
        .ok_or_else(|| "Request body already read".to_string())?
        .read_to_string(&mut body)
        .map_err(|e| format!("Failed to read body: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("Invalid JSON: {e}"))
}

#[derive(Deserialize)]
struct SetValueFloat {
    value: f32,
}

#[derive(Deserialize)]
struct SetValueBool {
    value: bool,
}

#[derive(Deserialize)]
struct SetValueInt {
    value: usize,
}

#[derive(Deserialize)]
struct SetValueString {
    value: String,
}

#[derive(Deserialize)]
struct AdjustVolumeRequest {
    value: f32,
    min: Option<f32>,
    max: Option<f32>,
}

#[derive(Deserialize)]
struct ReadConfigFileRequest {
    path: String,
}

#[derive(Serialize)]
struct AllLevels {
    playback_rms: Vec<f32>,
    playback_peak: Vec<f32>,
    capture_rms: Vec<f32>,
    capture_peak: Vec<f32>,
}

#[derive(Serialize)]
struct PbCapLevels {
    playback: Vec<f32>,
    capture: Vec<f32>,
}

#[derive(Serialize)]
struct Fader {
    volume: f32,
    mute: bool,
}

#[derive(Serialize)]
struct FaderVolumeValue {
    index: usize,
    volume: f32,
}

#[derive(Serialize)]
struct FaderMuteValue {
    index: usize,
    mute: bool,
}

#[derive(Serialize)]
struct SupportedDeviceTypes {
    playback: Vec<String>,
    capture: Vec<String>,
}

fn send_controller_message(
    shared_data: &SharedData,
    msg: ControllerMessage,
) -> rouille::Response {
    match shared_data.command_sender.try_send(msg) {
        Ok(()) => json_ok_empty(),
        Err(TrySendError::Full(_)) => {
            json_error(503, "Too many requests".to_string())
        }
        Err(TrySendError::Disconnected(_)) => {
            json_error(500, "Channel disconnected".to_string())
        }
    }
}

fn clamped_volume(vol: f32) -> f32 {
    vol.clamp(-150.0, 50.0)
}

fn notify_state_change(shared_data: &SharedData) {
    shared_data
        .unsaved_state_change
        .store(true, Ordering::Relaxed);
    shared_data
        .state_change_notify
        .try_send(())
        .unwrap_or(());
}

fn parse_since_param(request: &rouille::Request) -> Option<SinceParam> {
    request.get_param("since").map(|s| {
        if s == "last" {
            SinceParam::Last
        } else {
            match s.parse::<f32>() {
                Ok(secs) => SinceParam::Seconds(secs),
                Err(_) => SinceParam::Invalid,
            }
        }
    })
}

enum SinceParam {
    Seconds(f32),
    Last,
    Invalid,
}

// Workaround to safely subtract from an Instant on all operating systems
fn get_subtracted_instant(seconds: f32) -> Instant {
    let now = Instant::now();
    let mut clamped_seconds = seconds.clamp(0.0, 600.0);
    let mut maybe_instant = None;
    while maybe_instant.is_none() && clamped_seconds > 0.1 {
        maybe_instant = now.checked_sub(Duration::from_secs_f32(clamped_seconds));
        clamped_seconds /= 2.0;
    }
    maybe_instant.unwrap_or(now)
}

fn playback_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.playback_status.read().signal_peak.last();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .playback_status
        .read()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.playback_status.read().signal_rms.last_sqrt();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .playback_status
        .read()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.capture_status.read().signal_peak.last();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .capture_status
        .read()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.capture_status.read().signal_rms.last_sqrt();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .capture_status
        .read()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data.playback_status.read().signal_peak.global_max()
}

fn capture_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data.capture_status.read().signal_peak.global_max()
}

pub struct LocalData {
    pub last_cap_rms_time: Instant,
    pub last_cap_peak_time: Instant,
    pub last_pb_rms_time: Instant,
    pub last_pb_peak_time: Instant,
}

fn playback_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
) -> Vec<f32> {
    let last = local_data.lock().last_pb_peak_time;
    let res = shared_data
        .playback_status
        .read()
        .signal_peak
        .max_since(last);
    match res {
        Some(mut record) => {
            local_data.lock().last_pb_peak_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_rms_since_last(
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
) -> Vec<f32> {
    let last = local_data.lock().last_pb_rms_time;
    let res = shared_data
        .playback_status
        .read()
        .signal_rms
        .average_sqrt_since(last);
    match res {
        Some(mut record) => {
            local_data.lock().last_pb_rms_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
) -> Vec<f32> {
    let last = local_data.lock().last_cap_peak_time;
    let res = shared_data
        .capture_status
        .read()
        .signal_peak
        .max_since(last);
    match res {
        Some(mut record) => {
            local_data.lock().last_cap_peak_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_rms_since_last(
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
) -> Vec<f32> {
    let last = local_data.lock().last_cap_rms_time;
    let res = shared_data
        .capture_status
        .read()
        .signal_rms
        .average_sqrt_since(last);
    match res {
        Some(mut record) => {
            local_data.lock().last_cap_rms_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn handle_signal_with_since(
    request: &rouille::Request,
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
    current_fn: fn(&SharedData) -> Vec<f32>,
    since_fn: fn(&SharedData, f32) -> Vec<f32>,
    since_last_fn: fn(&SharedData, &Mutex<LocalData>) -> Vec<f32>,
) -> rouille::Response {
    match parse_since_param(request) {
        None => json_ok(current_fn(shared_data)),
        Some(SinceParam::Seconds(secs)) => json_ok(since_fn(shared_data, secs)),
        Some(SinceParam::Last) => json_ok(since_last_fn(shared_data, local_data)),
        Some(SinceParam::Invalid) => {
            json_error(400, "Invalid 'since' parameter".to_string())
        }
    }
}

fn parse_fader_index(index_str: &str) -> Result<usize, rouille::Response> {
    let index: usize = index_str
        .parse()
        .map_err(|_| json_error(400, "Invalid fader index".to_string()))?;
    if index >= ProcessingParameters::NUM_FADERS {
        return Err(json_error(
            422,
            format!("Fader index {index} out of range (0-{})", ProcessingParameters::NUM_FADERS - 1),
        ));
    }
    Ok(index)
}

pub fn start_server(address: &str, port: usize, shared_data: SharedData) {
    let bind_addr = format!("{address}:{port}");
    debug!("Start REST API server on {}", bind_addr);

    let shared_data = Arc::new(shared_data);
    let now = Instant::now();
    let local_data = Arc::new(Mutex::new(LocalData {
        last_cap_peak_time: now,
        last_cap_rms_time: now,
        last_pb_peak_time: now,
        last_pb_rms_time: now,
    }));

    let server = rouille::Server::new(&bind_addr, move |request| {
        handle_request(request, &shared_data, &local_data)
    });

    match server {
        Ok(server) => {
            std::thread::spawn(move || {
                server.run();
            });
        }
        Err(err) => {
            error!("Failed to start REST API server: {}", err);
        }
    }
}

pub fn handle_request(
    request: &rouille::Request,
    shared_data: &SharedData,
    local_data: &Mutex<LocalData>,
) -> rouille::Response {
    let url = request.url();
    let method: &str = request.method();

    // Serve OpenAPI spec
    if method == "GET" && (url == "/api/v1/openapi.yaml" || url == "/api/v1/openapi.yml") {
        return rouille::Response::text(include_str!("../docs/openapi.yaml"))
            .with_unique_header("Content-Type", "text/yaml; charset=utf-8");
    }

    // Strip the /api/v1 prefix
    let path = match url.strip_prefix("/api/v1") {
        Some("") => "/",
        Some(p) => p,
        None => {
            return json_error(404, format!("Not found: {url}"));
        }
    };

    match (method, path) {
        // ── System / Lifecycle ──
        ("GET", "/version") => json_ok(crate_version!().to_string()),

        ("GET", "/state") => {
            let capstat = shared_data.capture_status.read();
            json_ok(capstat.state)
        }

        ("GET", "/stopreason") => {
            let stat = shared_data.processing_status.read();
            json_ok(stat.stop_reason.clone())
        }

        ("POST", "/reload") => {
            let cfg_path = shared_data.active_config_path.lock().clone();
            match cfg_path {
                Some(path) => match config::load_config(path.as_str()) {
                    Ok(mut conf) => {
                        match config::validate_config(&mut conf, Some(path.as_str())) {
                            Ok(()) => send_controller_message(
                                shared_data,
                                ControllerMessage::ConfigChanged(Box::new(conf)),
                            ),
                            Err(err) => json_error(422, format!("Invalid config: {err}")),
                        }
                    }
                    Err(err) => json_error(422, format!("Config file error: {err}")),
                },
                None => json_error(422, "Config path not set, cannot reload".to_string()),
            }
        }

        ("POST", "/stop") => send_controller_message(shared_data, ControllerMessage::Stop),

        ("POST", "/exit") => send_controller_message(shared_data, ControllerMessage::Exit),

        // ── Configuration ──
        ("GET", "/config") => {
            json_ok(serde_yaml::to_string(&*shared_data.active_config.lock()).unwrap())
        }

        ("GET", "/config/json") => {
            json_ok(serde_json::to_string(&*shared_data.active_config.lock()).unwrap())
        }

        ("GET", "/config/title") => {
            let optional_config = shared_data.active_config.lock();
            let value = if let Some(cfg) = &*optional_config {
                cfg.title.clone().unwrap_or_default()
            } else {
                String::new()
            };
            json_ok(value)
        }

        ("GET", "/config/description") => {
            let optional_config = shared_data.active_config.lock();
            let value = if let Some(cfg) = &*optional_config {
                cfg.description.clone().unwrap_or_default()
            } else {
                String::new()
            };
            json_ok(value)
        }

        ("GET", "/config/previous") => {
            json_ok(serde_yaml::to_string(&*shared_data.previous_config.lock()).unwrap())
        }

        ("GET", "/config/filepath") => {
            let value = shared_data
                .active_config_path
                .lock()
                .as_ref()
                .map(|s| s.to_string());
            json_ok(value)
        }

        ("PUT", "/config/filepath") => {
            match read_json_body::<SetValueString>(request) {
                Ok(body) => match config::load_validate_config(&body.value) {
                    Ok(_) => {
                        *shared_data.active_config_path.lock() = Some(body.value);
                        notify_state_change(shared_data);
                        json_ok_empty()
                    }
                    Err(err) => json_error(422, format!("Error setting config path: {err}")),
                },
                Err(e) => json_error(400, e),
            }
        }

        ("PUT", "/config") => match read_json_body::<SetValueString>(request) {
            Ok(body) => {
                match serde_yaml::from_str::<config::Configuration>(&body.value) {
                    Ok(mut conf) => match config::validate_config(&mut conf, None) {
                        Ok(()) => send_controller_message(
                            shared_data,
                            ControllerMessage::ConfigChanged(Box::new(conf)),
                        ),
                        Err(err) => json_error(422, format!("Config validation error: {err}")),
                    },
                    Err(err) => json_error(400, format!("YAML parse error: {err}")),
                }
            }
            Err(e) => json_error(400, e),
        },

        ("PUT", "/config/json") => match read_json_body::<SetValueString>(request) {
            Ok(body) => {
                match serde_json::from_str::<config::Configuration>(&body.value) {
                    Ok(mut conf) => match config::validate_config(&mut conf, None) {
                        Ok(()) => send_controller_message(
                            shared_data,
                            ControllerMessage::ConfigChanged(Box::new(conf)),
                        ),
                        Err(err) => json_error(422, format!("Config validation error: {err}")),
                    },
                    Err(err) => json_error(400, format!("JSON parse error: {err}")),
                }
            }
            Err(e) => json_error(400, e),
        },

        ("POST", "/config/read") => match read_json_body::<SetValueString>(request) {
            Ok(body) => match serde_yaml::from_str::<config::Configuration>(&body.value) {
                Ok(conf) => json_ok(serde_yaml::to_string(&conf).unwrap()),
                Err(err) => json_error(422, format!("Error reading config: {err}")),
            },
            Err(e) => json_error(400, e),
        },

        ("POST", "/config/readfile") => match read_json_body::<ReadConfigFileRequest>(request) {
            Ok(body) => match config::load_config(&body.path) {
                Ok(conf) => json_ok(serde_yaml::to_string(&conf).unwrap()),
                Err(err) => json_error(422, format!("Error reading config file: {err}")),
            },
            Err(e) => json_error(400, e),
        },

        ("POST", "/config/validate") => match read_json_body::<SetValueString>(request) {
            Ok(body) => match serde_yaml::from_str::<config::Configuration>(&body.value) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => json_ok(serde_yaml::to_string(&conf).unwrap()),
                    Err(err) => json_error(422, format!("Config error: {err}")),
                },
                Err(err) => json_error(422, format!("Config error: {err}")),
            },
            Err(e) => json_error(400, e),
        },

        // ── State File ──
        ("GET", "/state/filepath") => json_ok(shared_data.state_file_path.clone()),

        ("GET", "/state/fileupdated") => {
            json_ok(!shared_data.unsaved_state_change.load(Ordering::Relaxed))
        }

        // ── Volume & Mute (Main / Fader 0) ──
        ("GET", "/volume") => {
            json_ok(shared_data.processing_params.target_volume(0))
        }

        ("PUT", "/volume") => match read_json_body::<SetValueFloat>(request) {
            Ok(body) => {
                let new_vol = clamped_volume(body.value);
                shared_data.processing_params.set_target_volume(0, new_vol);
                notify_state_change(shared_data);
                json_ok_empty()
            }
            Err(e) => json_error(400, e),
        },

        ("POST", "/volume/adjust") => match read_json_body::<AdjustVolumeRequest>(request) {
            Ok(body) => {
                let minvol = body.min.unwrap_or(-150.0);
                let maxvol = body.max.unwrap_or(50.0);
                if maxvol < minvol {
                    return json_error(422, "max must be >= min".to_string());
                }
                let mut tempvol = shared_data.processing_params.target_volume(0);
                tempvol += body.value;
                tempvol = tempvol.clamp(minvol, maxvol);
                shared_data.processing_params.set_target_volume(0, tempvol);
                notify_state_change(shared_data);
                json_ok(tempvol)
            }
            Err(e) => json_error(400, e),
        },

        ("GET", "/mute") => {
            json_ok(shared_data.processing_params.is_mute(0))
        }

        ("PUT", "/mute") => match read_json_body::<SetValueBool>(request) {
            Ok(body) => {
                shared_data.processing_params.set_mute(0, body.value);
                notify_state_change(shared_data);
                json_ok_empty()
            }
            Err(e) => json_error(400, e),
        },

        ("POST", "/mute/toggle") => {
            let old_mute = shared_data.processing_params.toggle_mute(0);
            notify_state_change(shared_data);
            json_ok(!old_mute)
        }

        // ── Faders ──
        ("GET", "/faders") => {
            let volumes = shared_data.processing_params.volumes();
            let mutes = shared_data.processing_params.mutes();
            let faders: Vec<Fader> = volumes
                .iter()
                .zip(mutes)
                .map(|(v, m)| Fader { volume: *v, mute: m })
                .collect();
            json_ok(faders)
        }

        // ── Signal Levels ──
        ("GET", "/signal/range") => {
            let capstat = shared_data.capture_status.read();
            json_ok(capstat.signal_range)
        }

        ("GET", "/signal/levels") => {
            match parse_since_param(request) {
                None => {
                    let levels = AllLevels {
                        playback_rms: playback_signal_rms(shared_data),
                        playback_peak: playback_signal_peak(shared_data),
                        capture_rms: capture_signal_rms(shared_data),
                        capture_peak: capture_signal_peak(shared_data),
                    };
                    json_ok(levels)
                }
                Some(SinceParam::Seconds(secs)) => {
                    let levels = AllLevels {
                        playback_rms: playback_signal_rms_since(shared_data, secs),
                        playback_peak: playback_signal_peak_since(shared_data, secs),
                        capture_rms: capture_signal_rms_since(shared_data, secs),
                        capture_peak: capture_signal_peak_since(shared_data, secs),
                    };
                    json_ok(levels)
                }
                Some(SinceParam::Last) => {
                    let levels = AllLevels {
                        playback_rms: playback_signal_rms_since_last(shared_data, local_data),
                        playback_peak: playback_signal_peak_since_last(shared_data, local_data),
                        capture_rms: capture_signal_rms_since_last(shared_data, local_data),
                        capture_peak: capture_signal_peak_since_last(shared_data, local_data),
                    };
                    json_ok(levels)
                }
                Some(SinceParam::Invalid) => {
                    json_error(400, "Invalid 'since' parameter".to_string())
                }
            }
        }

        ("GET", "/signal/peaks/sincestart") => {
            let levels = PbCapLevels {
                playback: playback_signal_global_peak(shared_data),
                capture: capture_signal_global_peak(shared_data),
            };
            json_ok(levels)
        }

        ("POST", "/signal/peaks/sincestart/reset") => {
            shared_data
                .playback_status
                .write()
                .signal_peak
                .reset_global_max();
            shared_data
                .capture_status
                .write()
                .signal_peak
                .reset_global_max();
            json_ok_empty()
        }

        ("GET", "/signal/capture/rms") => handle_signal_with_since(
            request,
            shared_data,
            local_data,
            capture_signal_rms,
            capture_signal_rms_since,
            capture_signal_rms_since_last,
        ),

        ("GET", "/signal/capture/peak") => handle_signal_with_since(
            request,
            shared_data,
            local_data,
            capture_signal_peak,
            capture_signal_peak_since,
            capture_signal_peak_since_last,
        ),

        ("GET", "/signal/playback/rms") => handle_signal_with_since(
            request,
            shared_data,
            local_data,
            playback_signal_rms,
            playback_signal_rms_since,
            playback_signal_rms_since_last,
        ),

        ("GET", "/signal/playback/peak") => handle_signal_with_since(
            request,
            shared_data,
            local_data,
            playback_signal_peak,
            playback_signal_peak_since,
            playback_signal_peak_since_last,
        ),

        // ── Processing ──
        ("GET", "/processing/capturerate") => {
            let capstat = shared_data.capture_status.read();
            json_ok(capstat.measured_samplerate)
        }

        ("GET", "/processing/updateinterval") => {
            let capstat = shared_data.capture_status.read();
            json_ok(capstat.update_interval)
        }

        ("PUT", "/processing/updateinterval") => {
            match read_json_body::<SetValueInt>(request) {
                Ok(body) => {
                    let mut captstat = shared_data.capture_status.write();
                    let mut playstat = shared_data.playback_status.write();
                    captstat.update_interval = body.value;
                    playstat.update_interval = body.value;
                    drop(captstat);
                    drop(playstat);
                    json_ok_empty()
                }
                Err(e) => json_error(400, e),
            }
        }

        ("GET", "/processing/rateadjust") => {
            let capstat = shared_data.capture_status.read();
            json_ok(capstat.rate_adjust)
        }

        ("GET", "/processing/bufferlevel") => {
            let pbstat = shared_data.playback_status.read();
            json_ok(pbstat.buffer_level)
        }

        ("GET", "/processing/clippedsamples") => {
            let pbstat = shared_data.playback_status.read();
            json_ok(pbstat.clipped_samples)
        }

        ("POST", "/processing/clippedsamples/reset") => {
            let mut pbstat = shared_data.playback_status.write();
            pbstat.clipped_samples = 0;
            drop(pbstat);
            json_ok_empty()
        }

        ("GET", "/processing/load") => {
            json_ok(shared_data.processing_params.processing_load())
        }

        // ── Devices ──
        ("GET", "/devices/supportedtypes") => {
            let (playback, capture) = list_supported_devices();
            json_ok(SupportedDeviceTypes { playback, capture })
        }

        _ => {
            // Handle dynamic path segments: faders/{index}/... and devices/{type}/{backend}
            handle_dynamic_routes(method, path, request, shared_data, local_data)
        }
    }
}

fn handle_dynamic_routes(
    method: &str,
    path: &str,
    request: &rouille::Request,
    shared_data: &SharedData,
    _local_data: &Mutex<LocalData>,
) -> rouille::Response {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match (method, segments.as_slice()) {
        // GET /faders/{index}/volume
        ("GET", ["faders", index, "volume"]) => {
            match parse_fader_index(index) {
                Ok(idx) => json_ok(FaderVolumeValue {
                    index: idx,
                    volume: shared_data.processing_params.target_volume(idx),
                }),
                Err(resp) => resp,
            }
        }

        // PUT /faders/{index}/volume
        ("PUT", ["faders", index, "volume"]) => {
            match parse_fader_index(index) {
                Ok(idx) => match read_json_body::<SetValueFloat>(request) {
                    Ok(body) => {
                        let new_vol = clamped_volume(body.value);
                        shared_data.processing_params.set_target_volume(idx, new_vol);
                        notify_state_change(shared_data);
                        json_ok_empty()
                    }
                    Err(e) => json_error(400, e),
                },
                Err(resp) => resp,
            }
        }

        // PUT /faders/{index}/volume/external
        ("PUT", ["faders", index, "volume", "external"]) => {
            match parse_fader_index(index) {
                Ok(idx) => match read_json_body::<SetValueFloat>(request) {
                    Ok(body) => {
                        let new_vol = clamped_volume(body.value);
                        shared_data.processing_params.set_target_volume(idx, new_vol);
                        shared_data.processing_params.set_current_volume(idx, new_vol);
                        notify_state_change(shared_data);
                        json_ok_empty()
                    }
                    Err(e) => json_error(400, e),
                },
                Err(resp) => resp,
            }
        }

        // POST /faders/{index}/volume/adjust
        ("POST", ["faders", index, "volume", "adjust"]) => {
            match parse_fader_index(index) {
                Ok(idx) => match read_json_body::<AdjustVolumeRequest>(request) {
                    Ok(body) => {
                        let minvol = body.min.unwrap_or(-150.0);
                        let maxvol = body.max.unwrap_or(50.0);
                        if maxvol < minvol {
                            return json_error(422, "max must be >= min".to_string());
                        }
                        let mut tempvol = shared_data.processing_params.target_volume(idx);
                        tempvol += body.value;
                        tempvol = tempvol.clamp(minvol, maxvol);
                        shared_data.processing_params.set_target_volume(idx, tempvol);
                        notify_state_change(shared_data);
                        json_ok(FaderVolumeValue {
                            index: idx,
                            volume: tempvol,
                        })
                    }
                    Err(e) => json_error(400, e),
                },
                Err(resp) => resp,
            }
        }

        // GET /faders/{index}/mute
        ("GET", ["faders", index, "mute"]) => {
            match parse_fader_index(index) {
                Ok(idx) => json_ok(FaderMuteValue {
                    index: idx,
                    mute: shared_data.processing_params.is_mute(idx),
                }),
                Err(resp) => resp,
            }
        }

        // PUT /faders/{index}/mute
        ("PUT", ["faders", index, "mute"]) => {
            match parse_fader_index(index) {
                Ok(idx) => match read_json_body::<SetValueBool>(request) {
                    Ok(body) => {
                        shared_data.processing_params.set_mute(idx, body.value);
                        notify_state_change(shared_data);
                        json_ok_empty()
                    }
                    Err(e) => json_error(400, e),
                },
                Err(resp) => resp,
            }
        }

        // POST /faders/{index}/mute/toggle
        ("POST", ["faders", index, "mute", "toggle"]) => {
            match parse_fader_index(index) {
                Ok(idx) => {
                    let old_mute = shared_data.processing_params.toggle_mute(idx);
                    notify_state_change(shared_data);
                    json_ok(FaderMuteValue {
                        index: idx,
                        mute: !old_mute,
                    })
                }
                Err(resp) => resp,
            }
        }

        // GET /devices/capture/{backend}
        ("GET", ["devices", "capture", backend]) => {
            let devs = list_available_devices(backend, true);
            json_ok(devs)
        }

        // GET /devices/playback/{backend}
        ("GET", ["devices", "playback", backend]) => {
            let devs = list_available_devices(backend, false);
            json_ok(devs)
        }

        _ => json_error(404, format!("Not found: {method} /api/v1{path}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Instant;

    fn make_test_shared_data() -> SharedData {
        let (tx_cmd, _rx_cmd) = crossbeam_channel::bounded(16);
        let (tx_state, _rx_state) = mpsc::sync_channel(1);
        SharedData {
            active_config: Arc::new(parking_lot::Mutex::new(None)),
            active_config_path: Arc::new(parking_lot::Mutex::new(None)),
            previous_config: Arc::new(parking_lot::Mutex::new(None)),
            command_sender: tx_cmd,
            capture_status: Arc::new(parking_lot::RwLock::new(CaptureStatus {
                measured_samplerate: 44100,
                update_interval: 1000,
                signal_range: 0.0,
                rate_adjust: 0.0,
                state: crate::ProcessingState::Inactive,
                signal_rms: crate::countertimer::ValueHistory::new(1024, 2),
                signal_peak: crate::countertimer::ValueHistory::new(1024, 2),
                used_channels: Vec::new(),
            })),
            playback_status: Arc::new(parking_lot::RwLock::new(PlaybackStatus {
                buffer_level: 0,
                clipped_samples: 0,
                update_interval: 1000,
                signal_rms: crate::countertimer::ValueHistory::new(1024, 2),
                signal_peak: crate::countertimer::ValueHistory::new(1024, 2),
            })),
            processing_params: Arc::new(ProcessingParameters::default()),
            processing_status: Arc::new(parking_lot::RwLock::new(crate::ProcessingStatus {
                stop_reason: crate::StopReason::None,
            })),
            state_change_notify: tx_state,
            state_file_path: Some("/tmp/test_state.yml".to_string()),
            unsaved_state_change: Arc::new(AtomicBool::new(false)),
        }
    }

    fn make_local_data() -> Mutex<LocalData> {
        let now = Instant::now();
        Mutex::new(LocalData {
            last_cap_rms_time: now,
            last_cap_peak_time: now,
            last_pb_rms_time: now,
            last_pb_peak_time: now,
        })
    }

    fn response_body(resp: rouille::Response) -> serde_json::Value {
        let (mut reader, _size) = resp.data.into_reader_and_size();
        let mut body = String::new();
        std::io::Read::read_to_string(&mut reader, &mut body).unwrap();
        serde_json::from_str(&body).unwrap()
    }

    // --- JSON envelope tests ---

    #[test]
    fn test_ok_response_serialization() {
        let resp = ok_response("hello");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], "Ok");
        assert_eq!(json["value"], "hello");
        assert!(json.get("message").is_none());
    }

    #[test]
    fn test_ok_empty_serialization() {
        let resp = ok_empty();
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], "Ok");
        assert!(json.get("value").is_none());
        assert!(json.get("message").is_none());
    }

    #[test]
    fn test_error_response_serialization() {
        let resp = error_response("something broke".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"], "Error");
        assert!(json.get("value").is_none());
        assert_eq!(json["message"], "something broke");
    }

    // --- SinceParam parsing tests ---

    #[test]
    fn test_parse_since_absent() {
        let req = rouille::Request::fake_http("GET", "/api/v1/signal/levels", vec![], vec![]);
        assert!(parse_since_param(&req).is_none());
    }

    #[test]
    fn test_parse_since_numeric() {
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/signal/levels?since=5.0",
            vec![],
            vec![],
        );
        match parse_since_param(&req) {
            Some(SinceParam::Seconds(s)) => assert!((s - 5.0).abs() < f32::EPSILON),
            other => panic!("Expected Seconds(5.0), got {:?}", other.is_some()),
        }
    }

    #[test]
    fn test_parse_since_last() {
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/signal/levels?since=last",
            vec![],
            vec![],
        );
        assert!(matches!(parse_since_param(&req), Some(SinceParam::Last)));
    }

    #[test]
    fn test_parse_since_invalid() {
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/signal/levels?since=xyz",
            vec![],
            vec![],
        );
        assert!(matches!(
            parse_since_param(&req),
            Some(SinceParam::Invalid)
        ));
    }

    // --- Fader index parsing tests ---

    #[test]
    fn test_parse_fader_index_valid() {
        assert_eq!(parse_fader_index("0").unwrap(), 0);
        assert_eq!(
            parse_fader_index(&(ProcessingParameters::NUM_FADERS - 1).to_string()).unwrap(),
            ProcessingParameters::NUM_FADERS - 1
        );
    }

    #[test]
    fn test_parse_fader_index_out_of_range() {
        let resp = parse_fader_index(&ProcessingParameters::NUM_FADERS.to_string()).unwrap_err();
        assert_eq!(resp.status_code, 422);
    }

    #[test]
    fn test_parse_fader_index_non_numeric() {
        let resp = parse_fader_index("abc").unwrap_err();
        assert_eq!(resp.status_code, 400);
    }

    // --- clamped_volume tests ---

    #[test]
    fn test_clamped_volume() {
        assert_eq!(clamped_volume(-200.0), -150.0);
        assert_eq!(clamped_volume(100.0), 50.0);
        assert_eq!(clamped_volume(-10.0), -10.0);
    }

    // --- Endpoint handler tests via handle_request ---

    #[test]
    fn test_get_version() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/version", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["result"], "Ok");
        assert!(body["value"].is_string());
    }

    #[test]
    fn test_get_state() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/state", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["result"], "Ok");
        assert_eq!(body["value"], "Inactive");
    }

    #[test]
    fn test_get_volume_default() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/volume", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["result"], "Ok");
        assert_eq!(body["value"], 0.0);
    }

    #[test]
    fn test_put_volume() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/volume",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": -10.5}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);

        // Verify volume was set
        let req2 = rouille::Request::fake_http("GET", "/api/v1/volume", vec![], vec![]);
        let resp2 = handle_request(&req2, &sd, &ld);
        let body = response_body(resp2);
        assert_eq!(body["value"], -10.5);
    }

    #[test]
    fn test_put_volume_clamped() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/volume",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 999.0}"#.to_vec(),
        );
        handle_request(&req, &sd, &ld);
        assert_eq!(sd.processing_params.target_volume(0), 50.0);
    }

    #[test]
    fn test_put_volume_invalid_json() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/volume",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            b"not json".to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 400);
        let body = response_body(resp);
        assert_eq!(body["result"], "Error");
    }

    #[test]
    fn test_get_mute_default() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/mute", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        let body = response_body(resp);
        assert_eq!(body["value"], false);
    }

    #[test]
    fn test_toggle_mute() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("POST", "/api/v1/mute/toggle", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["value"], true);

        // Toggle again
        let req2 = rouille::Request::fake_http("POST", "/api/v1/mute/toggle", vec![], vec![]);
        let resp2 = handle_request(&req2, &sd, &ld);
        let body2 = response_body(resp2);
        assert_eq!(body2["value"], false);
    }

    #[test]
    fn test_get_faders() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/faders", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        let faders = body["value"].as_array().unwrap();
        assert_eq!(faders.len(), ProcessingParameters::NUM_FADERS);
        for f in faders {
            assert_eq!(f["volume"], 0.0);
            assert_eq!(f["mute"], false);
        }
    }

    #[test]
    fn test_fader_volume_get_set() {
        let sd = make_test_shared_data();
        let ld = make_local_data();

        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/faders/1/volume",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": -5.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);

        let req2 = rouille::Request::fake_http("GET", "/api/v1/faders/1/volume", vec![], vec![]);
        let resp2 = handle_request(&req2, &sd, &ld);
        let body = response_body(resp2);
        assert_eq!(body["value"]["index"], 1);
        assert_eq!(body["value"]["volume"], -5.0);
    }

    #[test]
    fn test_fader_index_out_of_range() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/faders/99/volume", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 422);
    }

    #[test]
    fn test_fader_mute_toggle() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/faders/2/mute/toggle",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["value"]["index"], 2);
        assert_eq!(body["value"]["mute"], true);
    }

    #[test]
    fn test_get_signal_levels() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/signal/levels", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["result"], "Ok");
        assert!(body["value"]["playback_rms"].is_array());
        assert!(body["value"]["capture_rms"].is_array());
    }

    #[test]
    fn test_get_signal_levels_with_since() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/signal/levels?since=5.0",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
    }

    #[test]
    fn test_get_signal_levels_invalid_since() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/signal/levels?since=bogus",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 400);
    }

    #[test]
    fn test_get_processing_load() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        sd.processing_params.set_processing_load(0.42);
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/processing/load",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        let body = response_body(resp);
        assert!((body["value"].as_f64().unwrap() - 0.42).abs() < 0.001);
    }

    #[test]
    fn test_get_supported_device_types() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/devices/supportedtypes",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert!(body["value"]["playback"].is_array());
        assert!(body["value"]["capture"].is_array());
    }

    #[test]
    fn test_not_found() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "GET",
            "/api/v1/nonexistent",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 404);
    }

    #[test]
    fn test_no_api_prefix_returns_404() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/version", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 404);
    }

    #[test]
    fn test_get_state_filepath() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/state/filepath", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["value"], "/tmp/test_state.yml");
    }

    #[test]
    fn test_get_config_filepath_none() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/config/filepath", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert!(body["value"].is_null());
    }

    #[test]
    fn test_volume_adjust() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        sd.processing_params.set_target_volume(0, -10.0);

        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/volume/adjust",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 3.0, "min": -50.0, "max": 0.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["value"], -7.0);
    }

    #[test]
    fn test_volume_adjust_clamp_to_max() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        sd.processing_params.set_target_volume(0, -1.0);

        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/volume/adjust",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 10.0, "max": 0.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        let body = response_body(resp);
        assert_eq!(body["value"], 0.0);
    }

    #[test]
    fn test_volume_adjust_max_less_than_min() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/volume/adjust",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 1.0, "min": 0.0, "max": -10.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 422);
    }

    #[test]
    fn test_put_mute() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/mute",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": true}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        assert!(sd.processing_params.is_mute(0));
    }

    #[test]
    fn test_fader_volume_adjust() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        sd.processing_params.set_target_volume(1, -20.0);

        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/faders/1/volume/adjust",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 5.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["value"]["index"], 1);
        assert_eq!(body["value"]["volume"], -15.0);
    }

    #[test]
    fn test_clipped_samples_reset() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        sd.playback_status.write().clipped_samples = 42;

        let req = rouille::Request::fake_http(
            "POST",
            "/api/v1/processing/clippedsamples/reset",
            vec![],
            vec![],
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        assert_eq!(sd.playback_status.read().clipped_samples, 0);
    }

    #[test]
    fn test_get_stopreason() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http("GET", "/api/v1/stopreason", vec![], vec![]);
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        let body = response_body(resp);
        assert_eq!(body["result"], "Ok");
    }

    #[test]
    fn test_put_missing_body_field() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/volume",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"wrong_field": 1.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 400);
    }

    #[test]
    fn test_update_interval_get_set() {
        let sd = make_test_shared_data();
        let ld = make_local_data();

        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/processing/updateinterval",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": 500}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);

        let req2 = rouille::Request::fake_http(
            "GET",
            "/api/v1/processing/updateinterval",
            vec![],
            vec![],
        );
        let resp2 = handle_request(&req2, &sd, &ld);
        let body = response_body(resp2);
        assert_eq!(body["value"], 500);
    }

    #[test]
    fn test_fader_external_volume() {
        let sd = make_test_shared_data();
        let ld = make_local_data();
        let req = rouille::Request::fake_http(
            "PUT",
            "/api/v1/faders/0/volume/external",
            vec![("Content-Type".to_string(), "application/json".to_string())],
            br#"{"value": -3.0}"#.to_vec(),
        );
        let resp = handle_request(&req, &sd, &ld);
        assert_eq!(resp.status_code, 200);
        assert_eq!(sd.processing_params.target_volume(0), -3.0);
        assert_eq!(sd.processing_params.current_volume(0), -3.0);
    }
}
