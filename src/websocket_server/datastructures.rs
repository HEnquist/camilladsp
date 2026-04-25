use serde::{Deserialize, Serialize};
use serde_json;

use crate::spectrum::SpectrumData;
use crate::{AudioDeviceDescriptor, ProcessingState, StopReason};

#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub(crate) enum ValueWithOptionalLimits {
    Plain(f32),
    Limited(f32, f32, f32),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WsSignalLevelSide {
    Playback,
    Capture,
    Both,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SpectrumSide {
    Playback,
    Capture,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct SpectrumRequest {
    pub side: SpectrumSide,
    pub channel: Option<usize>,
    pub min_freq: f64,
    pub max_freq: f64,
    pub n_bins: usize,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct SpectrumSubscription {
    pub side: SpectrumSide,
    pub channel: Option<usize>,
    pub min_freq: f64,
    pub max_freq: f64,
    pub n_bins: usize,
    /// Maximum push rate in Hz. `None` = natural rate (one push per 50 % overlap hop).
    pub max_rate: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub(crate) struct VuSubscription {
    pub(crate) max_rate: f32,
    pub(crate) attack: f32,
    pub(crate) release: f32,
}

#[derive(Debug, PartialEq, Deserialize)]
pub(crate) enum WsCommand {
    SetConfigFilePath(String),
    SetConfig(String),
    SetConfigJson(String),
    PatchConfig(serde_json::Value),
    SetConfigValue(String, serde_json::Value),
    Reload,
    GetConfig,
    GetConfigValue(String),
    GetConfigTitle,
    GetConfigDescription,
    GetPreviousConfig,
    ReadConfig(String),
    ReadConfigJson(String),
    ReadConfigFile(String),
    ValidateConfig(String),
    ValidateConfigJson(String),
    GetConfigJson,
    GetConfigFilePath,
    GetStateFilePath,
    GetStateFileUpdated,
    GetSignalRange,
    GetCaptureSignalRms,
    GetCaptureSignalRmsSince(f32),
    GetCaptureSignalRmsSinceLast,
    GetCaptureSignalPeak,
    GetCaptureSignalPeakSince(f32),
    GetCaptureSignalPeakSinceLast,
    GetPlaybackSignalRms,
    GetPlaybackSignalRmsSince(f32),
    GetPlaybackSignalRmsSinceLast,
    GetPlaybackSignalPeak,
    GetPlaybackSignalPeakSince(f32),
    GetPlaybackSignalPeakSinceLast,
    GetSignalLevels,
    GetSignalLevelsSince(f32),
    GetSignalLevelsSinceLast,
    SubscribeSignalLevels(WsSignalLevelSide),
    SubscribeVuLevels(VuSubscription),
    StopSubscription,
    SubscribeState,
    GetSignalPeaksSinceStart,
    ResetSignalPeaksSinceStart,
    GetChannelLabels,
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval(usize),
    GetVolume,
    SetVolume(f32),
    AdjustVolume(ValueWithOptionalLimits),
    GetMute,
    SetMute(bool),
    ToggleMute,
    GetFaders,
    GetFaderVolume(usize),
    SetFaderVolume(usize, f32),
    SetFaderExternalVolume(usize, f32),
    AdjustFaderVolume(usize, ValueWithOptionalLimits),
    GetFaderMute(usize),
    SetFaderMute(usize, bool),
    ToggleFaderMute(usize),
    GetVersion,
    GetState,
    GetStopReason,
    GetRateAdjust,
    GetClippedSamples,
    ResetClippedSamples,
    GetBufferLevel,
    GetSupportedDeviceTypes,
    GetAvailableCaptureDevices(String),
    GetAvailablePlaybackDevices(String),
    GetCaptureDeviceCapabilities(String, String),
    GetPlaybackDeviceCapabilities(String, String),
    GetProcessingLoad,
    GetResamplerLoad,
    GetSpectrum(SpectrumRequest),
    SubscribeSpectrum(SpectrumSubscription),
    Exit,
    Stop,
    None,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(crate) enum WsResult {
    Ok,
    ShutdownInProgressError,
    RateLimitExceededError,
    InvalidFaderError,
    ConfigValidationError(String),
    ConfigReadError(String),
    InvalidValueError(String),
    InvalidRequestError(String),
    DeviceNotFoundError(String),
    DeviceBusyError(String),
    DeviceError(String),
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct ChannelLabels {
    pub(crate) playback: Option<Vec<Option<String>>>,
    pub(crate) capture: Option<Vec<Option<String>>>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct AllLevels {
    pub(crate) playback_rms: Vec<f32>,
    pub(crate) playback_peak: Vec<f32>,
    pub(crate) capture_rms: Vec<f32>,
    pub(crate) capture_peak: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct PbCapLevels {
    pub(crate) playback: Vec<f32>,
    pub(crate) capture: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Fader {
    pub(crate) volume: f32,
    pub(crate) mute: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct StreamLevels {
    pub(crate) side: WsSignalLevelSide,
    pub(crate) rms: Vec<f32>,
    pub(crate) peak: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct VuLevels {
    pub(crate) playback_rms: Vec<f32>,
    pub(crate) playback_peak: Vec<f32>,
    pub(crate) capture_rms: Vec<f32>,
    pub(crate) capture_peak: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct StateUpdate {
    pub(crate) state: ProcessingState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stop_reason: Option<StopReason>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) enum WsReply {
    SetConfigFilePath {
        result: WsResult,
    },
    SetConfig {
        result: WsResult,
    },
    SetConfigJson {
        result: WsResult,
    },
    PatchConfig {
        result: WsResult,
    },
    SetConfigValue {
        result: WsResult,
    },
    Reload {
        result: WsResult,
    },
    GetConfig {
        result: WsResult,
        value: String,
    },
    GetConfigJson {
        result: WsResult,
        value: String,
    },
    GetConfigValue {
        result: WsResult,
        value: serde_json::Value,
    },
    GetConfigTitle {
        result: WsResult,
        value: String,
    },
    GetConfigDescription {
        result: WsResult,
        value: String,
    },
    GetPreviousConfig {
        result: WsResult,
        value: String,
    },
    ReadConfig {
        result: WsResult,
        value: String,
    },
    ReadConfigJson {
        result: WsResult,
        value: String,
    },
    ReadConfigFile {
        result: WsResult,
        value: String,
    },
    ValidateConfig {
        result: WsResult,
        value: String,
    },
    ValidateConfigJson {
        result: WsResult,
        value: String,
    },
    GetConfigFilePath {
        result: WsResult,
        value: Option<String>,
    },
    GetStateFilePath {
        result: WsResult,
        value: Option<String>,
    },
    GetStateFileUpdated {
        result: WsResult,
        value: bool,
    },
    GetSignalRange {
        result: WsResult,
        value: f32,
    },
    GetPlaybackSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetSignalLevels {
        result: WsResult,
        value: AllLevels,
    },
    GetSignalLevelsSince {
        result: WsResult,
        value: AllLevels,
    },
    GetSignalLevelsSinceLast {
        result: WsResult,
        value: AllLevels,
    },
    SubscribeSignalLevels {
        result: WsResult,
    },
    SubscribeVuLevels {
        result: WsResult,
    },
    SubscribeState {
        result: WsResult,
    },
    StopSubscription {
        result: WsResult,
    },
    SignalLevelsEvent {
        result: WsResult,
        value: StreamLevels,
    },
    VuLevelsEvent {
        result: WsResult,
        value: VuLevels,
    },
    StateEvent {
        result: WsResult,
        value: StateUpdate,
    },
    GetSignalPeaksSinceStart {
        result: WsResult,
        value: PbCapLevels,
    },
    ResetSignalPeaksSinceStart {
        result: WsResult,
    },
    GetChannelLabels {
        result: WsResult,
        value: ChannelLabels,
    },
    GetCaptureRate {
        result: WsResult,
        value: usize,
    },
    GetUpdateInterval {
        result: WsResult,
        value: usize,
    },
    SetUpdateInterval {
        result: WsResult,
    },
    SetVolume {
        result: WsResult,
    },
    GetVolume {
        result: WsResult,
        value: f32,
    },
    AdjustVolume {
        result: WsResult,
        value: f32,
    },
    SetMute {
        result: WsResult,
    },
    GetMute {
        result: WsResult,
        value: bool,
    },
    ToggleMute {
        result: WsResult,
        value: bool,
    },
    SetFaderVolume {
        result: WsResult,
    },
    SetFaderExternalVolume {
        result: WsResult,
    },
    GetFaders {
        result: WsResult,
        value: Vec<Fader>,
    },
    GetFaderVolume {
        result: WsResult,
        value: (usize, f32),
    },
    AdjustFaderVolume {
        result: WsResult,
        value: (usize, f32),
    },
    SetFaderMute {
        result: WsResult,
    },
    GetFaderMute {
        result: WsResult,
        value: (usize, bool),
    },
    ToggleFaderMute {
        result: WsResult,
        value: (usize, bool),
    },
    GetVersion {
        result: WsResult,
        value: String,
    },
    GetState {
        result: WsResult,
        value: ProcessingState,
    },
    GetStopReason {
        result: WsResult,
        value: StopReason,
    },
    GetRateAdjust {
        result: WsResult,
        value: f32,
    },
    GetBufferLevel {
        result: WsResult,
        value: usize,
    },
    GetClippedSamples {
        result: WsResult,
        value: usize,
    },
    ResetClippedSamples {
        result: WsResult,
    },
    GetSupportedDeviceTypes {
        result: WsResult,
        value: (Vec<String>, Vec<String>),
    },
    GetAvailableCaptureDevices {
        result: WsResult,
        value: Vec<(String, String)>,
    },
    GetAvailablePlaybackDevices {
        result: WsResult,
        value: Vec<(String, String)>,
    },
    GetCaptureDeviceCapabilities {
        result: WsResult,
        value: AudioDeviceDescriptor,
    },
    GetPlaybackDeviceCapabilities {
        result: WsResult,
        value: AudioDeviceDescriptor,
    },
    GetProcessingLoad {
        result: WsResult,
        value: f32,
    },
    GetResamplerLoad {
        result: WsResult,
        value: f32,
    },
    GetSpectrum {
        result: WsResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<SpectrumData>,
    },
    SubscribeSpectrum {
        result: WsResult,
    },
    SpectrumEvent {
        result: WsResult,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<SpectrumData>,
    },
    Exit {
        result: WsResult,
    },
    Stop {
        result: WsResult,
    },
    Invalid {
        error: String,
    },
}
