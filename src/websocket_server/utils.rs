#[cfg(feature = "secure-websocket")]
use native_tls::{Identity, TlsAcceptor, TlsStream};
use serde_json;
#[cfg(feature = "secure-websocket")]
use std::fs::File;
#[cfg(feature = "secure-websocket")]
use std::io::Read;
use std::net::TcpStream;
#[cfg(feature = "secure-websocket")]
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::accept;

use super::datastructures::{
    StateUpdate, StreamLevels, VuSubscription, WsCommand, WsReply, WsResult, WsSignalLevelSide,
};
use super::{LocalData, SharedData};
use crate::ProcessingState;
use crate::Res;
use crate::utils::decibels::linear_to_db_inplace;

const MAX_VU_TIME_CONSTANT_MS: f32 = 60_000.0;

pub(crate) fn validate_vu_subscription(config: VuSubscription) -> Result<VuSubscription, WsResult> {
    validate_vu_time_constant("attack", config.attack)?;
    validate_vu_time_constant("release", config.release)?;
    Ok(config)
}

fn validate_vu_time_constant(name: &str, value: f32) -> Result<(), WsResult> {
    if !value.is_finite() {
        return Err(WsResult::InvalidValueError(format!(
            "{name} must be a finite number"
        )));
    }

    if !(0.0..=MAX_VU_TIME_CONSTANT_MS).contains(&value) {
        return Err(WsResult::InvalidValueError(format!(
            "{name} must be between 0 and {MAX_VU_TIME_CONSTANT_MS:.0} ms"
        )));
    }

    Ok(())
}

pub(crate) fn parse_command(cmd: Message) -> Res<WsCommand> {
    match cmd {
        Message::Text(command_str) => {
            let command = serde_json::from_str::<WsCommand>(&command_str)?;
            Ok(command)
        }
        _ => Ok(WsCommand::None),
    }
}

#[cfg(feature = "secure-websocket")]
fn make_acceptor_with_cert(cert: &str, key: &str) -> Res<Arc<TlsAcceptor>> {
    let mut file = File::open(cert)?;
    let mut identity = vec![];
    file.read_to_end(&mut identity)?;
    let identity = Identity::from_pkcs12(&identity, key)?;
    let acceptor = TlsAcceptor::new(identity)?;
    Ok(Arc::new(acceptor))
}

#[cfg(feature = "secure-websocket")]
pub(crate) fn make_acceptor(
    cert_file: &Option<&str>,
    cert_key: &Option<&str>,
) -> Option<Arc<TlsAcceptor>> {
    if let (Some(cert), Some(key)) = (cert_file, cert_key) {
        let acceptor = make_acceptor_with_cert(cert, key);
        match acceptor {
            Ok(acc) => {
                debug!("Created TLS acceptor");
                return Some(acc);
            }
            Err(err) => {
                error!("Could not create TLS acceptor: {}", err);
            }
        }
    }
    debug!("Running websocket server without TLS");
    None
}

#[cfg(feature = "secure-websocket")]
pub(crate) fn accept_secure_stream(
    acceptor: Arc<TlsAcceptor>,
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TlsStream<TcpStream>>> {
    let ws = accept(acceptor.accept(stream?)?)?;
    Ok(ws)
}

pub(crate) fn accept_plain_stream(
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TcpStream>> {
    let ws = accept(stream?)?;
    Ok(ws)
}

pub(crate) trait StreamTimeoutExt {
    fn set_stream_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl StreamTimeoutExt for WebSocket<TcpStream> {
    fn set_stream_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.get_mut().set_read_timeout(timeout)
    }
}

#[cfg(feature = "secure-websocket")]
impl StreamTimeoutExt for WebSocket<TlsStream<TcpStream>> {
    fn set_stream_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.get_mut().get_mut().set_read_timeout(timeout)
    }
}

pub(crate) fn set_stream_timeout<T>(websocket: &mut T, timeout: Option<Duration>)
where
    T: StreamTimeoutExt,
{
    if let Err(err) = websocket.set_stream_timeout(timeout) {
        warn!("Failed to set websocket read timeout: {err}");
    }
}

pub(crate) fn is_timeout_error(err: &tungstenite::error::Error) -> bool {
    matches!(
        err,
        tungstenite::error::Error::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::WouldBlock
                || io_err.kind() == std::io::ErrorKind::TimedOut
    )
}

pub(crate) fn smoothing_alpha(delta: Duration, time_constant_ms: f32) -> f32 {
    if time_constant_ms <= 0.0 {
        return 1.0;
    }

    let delta_seconds = delta.as_secs_f32();
    if delta_seconds <= 0.0 {
        return 0.0;
    }

    let time_constant_seconds = time_constant_ms / 1000.0;
    1.0 - (-delta_seconds / time_constant_seconds).exp()
}

pub(crate) fn smooth_levels(
    previous: &[f32],
    current: Vec<f32>,
    attack: f32,
    release: f32,
) -> Vec<f32> {
    if previous.len() != current.len() {
        return current;
    }

    if attack >= 1.0 && release >= 1.0 {
        return current;
    }

    if attack <= 0.0 && release <= 0.0 {
        return previous.to_vec();
    }

    previous
        .iter()
        .zip(current)
        .map(|(previous, current)| {
            if current > *previous {
                previous + attack * (current - previous)
            } else if current < *previous {
                previous + release * (current - previous)
            } else {
                *previous
            }
        })
        .collect()
}

pub(crate) fn get_signal_levels_values(
    side: WsSignalLevelSide,
    shared_data_inst: &SharedData,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let (rms, peak) = match side {
        WsSignalLevelSide::Playback => (
            playback_signal_rms(shared_data_inst),
            playback_signal_peak(shared_data_inst),
        ),
        WsSignalLevelSide::Capture => (
            capture_signal_rms(shared_data_inst),
            capture_signal_peak(shared_data_inst),
        ),
        WsSignalLevelSide::Both => return None,
    };

    if rms.is_empty() && peak.is_empty() {
        None
    } else {
        Some((rms, peak))
    }
}

pub(crate) fn get_signal_levels_values_linear(
    side: WsSignalLevelSide,
    shared_data_inst: &SharedData,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let (rms, peak) = match side {
        WsSignalLevelSide::Playback => (
            playback_signal_rms_linear(shared_data_inst),
            playback_signal_peak_linear(shared_data_inst),
        ),
        WsSignalLevelSide::Capture => (
            capture_signal_rms_linear(shared_data_inst),
            capture_signal_peak_linear(shared_data_inst),
        ),
        WsSignalLevelSide::Both => return None,
    };

    if rms.is_empty() && peak.is_empty() {
        None
    } else {
        Some((rms, peak))
    }
}

pub(crate) fn get_stream_levels_event(
    side: WsSignalLevelSide,
    shared_data_inst: &SharedData,
) -> Option<WsReply> {
    get_signal_levels_values(side, shared_data_inst).map(|(rms, peak)| WsReply::SignalLevelsEvent {
        result: WsResult::Ok,
        value: StreamLevels { side, rms, peak },
    })
}

pub(crate) fn stream_invalid_reply() -> WsReply {
    WsReply::Invalid {
        error: "Only StopSubscription is accepted while streaming is active".to_string(),
    }
}

pub(crate) fn current_processing_state(shared_data: &SharedData) -> ProcessingState {
    shared_data.capture_status.read().state
}

pub(crate) fn get_state_event(state: ProcessingState, shared_data: &SharedData) -> WsReply {
    let stop_reason = if state == ProcessingState::Inactive {
        Some(shared_data.processing_status.read().stop_reason.clone())
    } else {
        None
    };

    WsReply::StateEvent {
        result: WsResult::Ok,
        value: StateUpdate { state, stop_reason },
    }
}

pub(crate) fn clamped_volume(vol: f32) -> f32 {
    let mut new_vol = vol;
    if new_vol < -150.0 {
        new_vol = -150.0;
        warn!("Clamped volume at -150 dB")
    } else if new_vol > 50.0 {
        new_vol = 50.0;
        warn!("Clamped volume at +50 dB")
    }
    new_vol
}

pub(crate) fn get_subtracted_instant(seconds: f32) -> Instant {
    let now = Instant::now();
    let mut clamped_seconds = seconds.clamp(0.0, 600.0);
    let mut maybe_instant = None;
    while maybe_instant.is_none() && clamped_seconds > 0.1 {
        maybe_instant = now.checked_sub(Duration::from_secs_f32(clamped_seconds));
        clamped_seconds /= 2.0;
    }
    maybe_instant.unwrap_or(now)
}

pub(crate) fn playback_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .playback_status
        .read()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn playback_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .playback_status
        .read()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn capture_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .capture_status
        .read()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn capture_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = get_subtracted_instant(time);
    let res = shared_data
        .capture_status
        .read()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn playback_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .read()
        .signal_peak
        .max_since(local_data.last_pb_peak_time);
    match res {
        Some(mut record) => {
            local_data.last_pb_peak_time = record.time;
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn playback_signal_rms_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .read()
        .signal_rms
        .average_sqrt_since(local_data.last_pb_rms_time);
    match res {
        Some(mut record) => {
            local_data.last_pb_rms_time = record.time;
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn capture_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .read()
        .signal_peak
        .max_since(local_data.last_cap_peak_time);
    match res {
        Some(mut record) => {
            local_data.last_cap_peak_time = record.time;
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn capture_signal_rms_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .read()
        .signal_rms
        .average_sqrt_since(local_data.last_cap_rms_time);
    match res {
        Some(mut record) => {
            local_data.last_cap_rms_time = record.time;
            linear_to_db_inplace(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

pub(crate) fn playback_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let mut values = playback_signal_peak_linear(shared_data);
    linear_to_db_inplace(&mut values);
    values
}

pub(crate) fn playback_signal_peak_linear(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.playback_status.read().signal_peak.last();
    match res {
        Some(record) => record.values,
        None => vec![],
    }
}

pub(crate) fn playback_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data.playback_status.read().signal_peak.global_max()
}

pub(crate) fn reset_playback_signal_global_peak(shared_data: &SharedData) {
    shared_data
        .playback_status
        .write()
        .signal_peak
        .reset_global_max();
}

pub(crate) fn playback_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let mut values = playback_signal_rms_linear(shared_data);
    linear_to_db_inplace(&mut values);
    values
}

pub(crate) fn playback_signal_rms_linear(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.playback_status.read().signal_rms.last_sqrt();
    match res {
        Some(record) => record.values,
        None => vec![],
    }
}

pub(crate) fn capture_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let mut values = capture_signal_peak_linear(shared_data);
    linear_to_db_inplace(&mut values);
    values
}

pub(crate) fn capture_signal_peak_linear(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.capture_status.read().signal_peak.last();
    match res {
        Some(record) => record.values,
        None => vec![],
    }
}

pub(crate) fn capture_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data.capture_status.read().signal_peak.global_max()
}

pub(crate) fn reset_capture_signal_global_peak(shared_data: &SharedData) {
    shared_data
        .capture_status
        .write()
        .signal_peak
        .reset_global_max();
}

pub(crate) fn capture_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let mut values = capture_signal_rms_linear(shared_data);
    linear_to_db_inplace(&mut values);
    values
}

pub(crate) fn capture_signal_rms_linear(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data.capture_status.read().signal_rms.last_sqrt();
    match res {
        Some(record) => record.values,
        None => vec![],
    }
}
