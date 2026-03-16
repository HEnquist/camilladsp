#![cfg(feature = "rest-api")]

use camillalib::countertimer::ValueHistory;
use camillalib::restserver::SharedData;
use camillalib::{
    CaptureStatus, PlaybackStatus, ProcessingParameters, ProcessingState, ProcessingStatus,
    StopReason,
};
use parking_lot::{Mutex, RwLock};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;

fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn make_shared_data() -> SharedData {
    let (tx_cmd, _rx_cmd) = crossbeam_channel::bounded(16);
    let (tx_state, _rx_state) = mpsc::sync_channel(1);
    SharedData {
        active_config: Arc::new(Mutex::new(None)),
        active_config_path: Arc::new(Mutex::new(None)),
        previous_config: Arc::new(Mutex::new(None)),
        command_sender: tx_cmd,
        capture_status: Arc::new(RwLock::new(CaptureStatus {
            measured_samplerate: 44100,
            update_interval: 1000,
            signal_range: 0.0,
            rate_adjust: 0.0,
            state: ProcessingState::Inactive,
            signal_rms: ValueHistory::new(1024, 2),
            signal_peak: ValueHistory::new(1024, 2),
            used_channels: Vec::new(),
        })),
        playback_status: Arc::new(RwLock::new(PlaybackStatus {
            buffer_level: 0,
            clipped_samples: 0,
            update_interval: 1000,
            signal_rms: ValueHistory::new(1024, 2),
            signal_peak: ValueHistory::new(1024, 2),
        })),
        processing_params: Arc::new(ProcessingParameters::default()),
        processing_status: Arc::new(RwLock::new(ProcessingStatus {
            stop_reason: StopReason::None,
        })),
        state_change_notify: tx_state,
        state_file_path: Some("/tmp/test_state.yml".to_string()),
        unsaved_state_change: Arc::new(AtomicBool::new(false)),
    }
}

fn start_test_server(shared_data: SharedData) -> String {
    let port = find_free_port();
    let addr = format!("127.0.0.1:{port}");
    let bind_addr = addr.clone();

    let shared_data = Arc::new(shared_data);
    let now = std::time::Instant::now();
    let local_data = Arc::new(Mutex::new(camillalib::restserver::LocalData {
        last_cap_peak_time: now,
        last_cap_rms_time: now,
        last_pb_peak_time: now,
        last_pb_rms_time: now,
    }));

    let server = rouille::Server::new(&bind_addr, move |request| {
        camillalib::restserver::handle_request(request, &shared_data, &local_data)
    })
    .expect("Failed to start test server");

    std::thread::spawn(move || server.run());
    // Give server time to bind
    std::thread::sleep(std::time::Duration::from_millis(50));

    format!("http://{addr}")
}

fn get_json(base: &str, path: &str) -> serde_json::Value {
    let url = format!("{base}{path}");
    let resp = ureq::get(&url).call().unwrap();
    let body = resp.into_body().read_to_string().unwrap();
    serde_json::from_str(&body).unwrap()
}

fn put_json(base: &str, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let url = format!("{base}{path}");
    let payload = serde_json::to_vec(body).unwrap();
    let resp = ureq::put(&url)
        .header("Content-Type", "application/json")
        .send(&payload[..])
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap();
    (status, serde_json::from_str(&text).unwrap())
}

fn post_json(base: &str, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let url = format!("{base}{path}");
    let payload = serde_json::to_vec(body).unwrap();
    let resp = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send(&payload[..])
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap();
    (status, serde_json::from_str(&text).unwrap())
}

fn post_empty(base: &str, path: &str) -> (u16, serde_json::Value) {
    let url = format!("{base}{path}");
    let resp = ureq::post(&url).send_empty().unwrap();
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap();
    (status, serde_json::from_str(&text).unwrap())
}

// --- Integration Tests ---

#[test]
fn test_version_endpoint() {
    let sd = make_shared_data();
    let base = start_test_server(sd);
    let body = get_json(&base, "/api/v1/version");
    assert_eq!(body["result"], "Ok");
    assert!(body["value"].is_string());
}

#[test]
fn test_state_endpoint() {
    let sd = make_shared_data();
    let base = start_test_server(sd);
    let body = get_json(&base, "/api/v1/state");
    assert_eq!(body["result"], "Ok");
    assert_eq!(body["value"], "Inactive");
}

#[test]
fn test_volume_roundtrip() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    // Get default volume
    let body = get_json(&base, "/api/v1/volume");
    assert_eq!(body["value"], 0.0);

    // Set volume
    let (status, _) = put_json(
        &base,
        "/api/v1/volume",
        &serde_json::json!({"value": -12.5}),
    );
    assert_eq!(status, 200);

    // Verify
    let body = get_json(&base, "/api/v1/volume");
    assert_eq!(body["value"], -12.5);
}

#[test]
fn test_mute_toggle() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/mute");
    assert_eq!(body["value"], false);

    let (status, body) = post_empty(&base, "/api/v1/mute/toggle");
    assert_eq!(status, 200);
    assert_eq!(body["value"], true);

    let body = get_json(&base, "/api/v1/mute");
    assert_eq!(body["value"], true);
}

#[test]
fn test_faders_list() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/faders");
    let faders = body["value"].as_array().unwrap();
    assert_eq!(faders.len(), ProcessingParameters::NUM_FADERS);
}

#[test]
fn test_fader_volume_set_get() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let (status, _) = put_json(
        &base,
        "/api/v1/faders/0/volume",
        &serde_json::json!({"value": -8.0}),
    );
    assert_eq!(status, 200);

    let body = get_json(&base, "/api/v1/faders/0/volume");
    assert_eq!(body["value"]["index"], 0);
    assert_eq!(body["value"]["volume"], -8.0);
}

#[test]
fn test_signal_levels() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/signal/levels");
    assert_eq!(body["result"], "Ok");
    assert!(body["value"]["playback_rms"].is_array());
    assert!(body["value"]["playback_peak"].is_array());
    assert!(body["value"]["capture_rms"].is_array());
    assert!(body["value"]["capture_peak"].is_array());
}

#[test]
fn test_signal_levels_with_since() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/signal/levels?since=5.0");
    assert_eq!(body["result"], "Ok");
}

#[test]
fn test_processing_load() {
    let sd = make_shared_data();
    sd.processing_params.set_processing_load(0.75);
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/processing/load");
    assert!((body["value"].as_f64().unwrap() - 0.75).abs() < 0.001);
}

#[test]
fn test_supported_device_types() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let body = get_json(&base, "/api/v1/devices/supportedtypes");
    assert!(body["value"]["playback"].is_array());
    assert!(body["value"]["capture"].is_array());
}

// --- Error case tests ---

#[test]
fn test_invalid_fader_index_returns_422() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let url = format!("{base}/api/v1/faders/99/volume");
    let resp = ureq::get(&url).call();
    match resp {
        Err(ureq::Error::StatusCode(code)) => assert_eq!(code, 422),
        Ok(r) => panic!("Expected 422, got {}", r.status()),
        Err(e) => panic!("Unexpected error: {e}"),
    }
}

#[test]
fn test_invalid_json_body_returns_400() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let url = format!("{base}/api/v1/volume");
    let resp = ureq::put(&url)
        .header("Content-Type", "application/json")
        .send(b"not json" as &[u8]);
    match resp {
        Err(ureq::Error::StatusCode(code)) => assert_eq!(code, 400),
        Ok(r) => panic!("Expected 400, got {}", r.status()),
        Err(e) => panic!("Unexpected error: {e}"),
    }
}

#[test]
fn test_unknown_endpoint_returns_404() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let url = format!("{base}/api/v1/does_not_exist");
    let resp = ureq::get(&url).call();
    match resp {
        Err(ureq::Error::StatusCode(code)) => assert_eq!(code, 404),
        Ok(r) => panic!("Expected 404, got {}", r.status()),
        Err(e) => panic!("Unexpected error: {e}"),
    }
}

#[test]
fn test_put_missing_required_field_returns_400() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let url = format!("{base}/api/v1/volume");
    let resp = ureq::put(&url)
        .header("Content-Type", "application/json")
        .send(br#"{"wrong": 1.0}"# as &[u8]);
    match resp {
        Err(ureq::Error::StatusCode(code)) => assert_eq!(code, 400),
        Ok(r) => panic!("Expected 400, got {}", r.status()),
        Err(e) => panic!("Unexpected error: {e}"),
    }
}

#[test]
fn test_volume_adjust_endpoint() {
    let sd = make_shared_data();
    sd.processing_params.set_target_volume(0, -20.0);
    let base = start_test_server(sd);

    let (status, body) = post_json(
        &base,
        "/api/v1/volume/adjust",
        &serde_json::json!({"value": 5.0, "min": -50.0, "max": 0.0}),
    );
    assert_eq!(status, 200);
    assert_eq!(body["value"], -15.0);
}

#[test]
fn test_fader_mute_toggle_endpoint() {
    let sd = make_shared_data();
    let base = start_test_server(sd);

    let (status, body) = post_empty(&base, "/api/v1/faders/1/mute/toggle");
    assert_eq!(status, 200);
    assert_eq!(body["value"]["index"], 1);
    assert_eq!(body["value"]["mute"], true);
}
