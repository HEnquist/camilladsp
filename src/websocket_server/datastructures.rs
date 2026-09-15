use serde::{Deserialize, Serialize};
use serde_json;

use crate::spectrum::SpectrumData;
use crate::{AudioDeviceDescriptor, ProcessingState, StopReason};

/// Side selector for [`WsCommand::SubscribeSignalLevels`] subscriptions.
///
/// Serialised as a lowercase string: `"playback"`, `"capture"`, or `"both"`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WsSignalLevelSide {
    /// Playback side only.
    Playback,
    /// Capture side only.
    Capture,
    /// Both playback and capture sides.
    Both,
}

/// Side selector for spectrum analysis commands.
///
/// Serialised as a lowercase string: `"playback"` or `"capture"`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SpectrumSide {
    /// Playback side.
    Playback,
    /// Capture side.
    Capture,
}

/// Parameters for a one-shot spectrum request ([`WsCommand::GetSpectrum`]).
///
/// The spectrum is computed from a Hann-windowed FFT.
/// Output bins are logarithmically spaced between `min_freq` and `max_freq`.
/// Magnitudes are returned in dBFS (0 dBFS = full-scale sine wave, amplitude 1.0).
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct SpectrumRequest {
    /// Which side to analyze: `"capture"` or `"playback"`.
    pub side: SpectrumSide,
    /// Channel to analyze. `null` averages all channels; an integer selects a single channel (zero-based).
    pub channel: Option<usize>,
    /// Lower edge of the frequency range in Hz. Must be > 0.
    pub min_freq: f64,
    /// Upper edge of the frequency range in Hz. Must be > `min_freq`.
    pub max_freq: f64,
    /// Number of output bins. Must be ≥ 2.
    pub n_bins: usize,
}

/// Parameters for a streaming spectrum subscription ([`WsCommand::SubscribeSpectrum`]).
///
/// Same fields as [`SpectrumRequest`] plus an optional `max_rate` cap.
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct SpectrumSubscription {
    /// Which side to analyze: `"capture"` or `"playback"`.
    pub side: SpectrumSide,
    /// Channel to analyze. `null` averages all channels; an integer selects a single channel (zero-based).
    pub channel: Option<usize>,
    /// Lower edge of the frequency range in Hz. Must be > 0.
    pub min_freq: f64,
    /// Upper edge of the frequency range in Hz. Must be > `min_freq`.
    pub max_freq: f64,
    /// Number of output bins. Must be ≥ 2.
    pub n_bins: usize,
    /// Maximum push rate in Hz. `None` = natural rate (one push per 50 % overlap hop).
    pub max_rate: Option<f32>,
}

/// Parameters for a VU-meter subscription ([`WsCommand::SubscribeVuLevels`]).
///
/// Controls smoothing and rate-limiting of pushed level events.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub(crate) struct VuSubscription {
    /// Maximum event rate in Hz. A value ≤ 0 disables rate limiting.
    ///
    /// If set higher than the natural update rate, events are sent at the natural rate.
    pub(crate) max_rate: f32,
    /// Attack time constant in ms for rising values. Valid range: 0–60000. `0` disables smoothing.
    ///
    /// A smaller value gives a faster, more responsive meter on rising signals.
    /// For peak values, upward changes are always applied immediately regardless of this setting.
    pub(crate) attack: f32,
    /// Release time constant in ms for falling values. Valid range: 0–60000. `0` disables smoothing.
    ///
    /// A smaller value makes the meter drop faster; a larger value gives a slower decay.
    /// A good starting point for an analog-feel meter is around 300 ms.
    pub(crate) release: f32,
}

/// All commands accepted by the websocket server.
///
/// Every command is a JSON object with a `"command"` field holding the command name.
/// Commands with arguments carry them in additional named fields, e.g.
/// `{"command": "SetUpdateInterval", "value": 500}`.
///
/// See the [module-level documentation](self) for the general message format.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "command")]
pub(crate) enum WsCommand {
    // ── Config management ──────────────────────────────────────────────────
    /// Change the active config file path. Not applied until [`Reload`](Self::Reload) is called.
    ///
    /// Argument: file path as a string.
    SetConfigFilePath { value: String },

    /// Upload and immediately apply a new configuration as a YAML string.
    ///
    /// Argument: config in YAML format as a string.
    SetConfig { value: String },

    /// Upload and immediately apply a new configuration as a JSON string.
    ///
    /// Argument: config in JSON format as a string.
    SetConfigJson { value: String },

    /// Apply a partial patch to the active configuration.
    ///
    /// The patch is a partial config object containing only the fields to change.
    /// If the resulting config is valid it is applied immediately.
    ///
    /// Argument: partial config as a JSON value.
    PatchConfig { value: serde_json::Value },

    /// Set a single value in the active configuration using a JSON Pointer (RFC 6901).
    ///
    /// Fields: `pointer` is a JSON Pointer string such as `"/devices/samplerate"`, and `value`
    /// is the JSON value to store there.
    SetConfigValue {
        pointer: String,
        value: serde_json::Value,
    },

    /// Reload the current config file from disk. Equivalent to sending `SIGHUP`.
    Reload,

    /// Read the active configuration.
    GetConfig,

    /// Read a single value from the active configuration using a JSON Pointer (RFC 6901).
    ///
    /// Argument: JSON Pointer string, e.g. `"/devices/samplerate"`.
    GetConfigValue { value: String },

    /// Read the `title` field from the active configuration.
    GetConfigTitle,

    /// Read the `description` field from the active configuration.
    GetConfigDescription,

    /// Read the previously active configuration (before the last reload or upload).
    GetPreviousConfig,

    /// Parse and fill defaults for a YAML config string without changing the active config.
    ///
    /// Argument: config in YAML format as a string.
    ReadConfig { value: String },

    /// Parse and fill defaults for a JSON config string without changing the active config.
    ///
    /// Argument: config in JSON format as a string.
    ReadConfigJson { value: String },

    /// Parse and fill defaults for a config file without changing the active config.
    ///
    /// Argument: path to the config file as a string.
    ReadConfigFile { value: String },

    /// Like [`ReadConfig`](Self::ReadConfig) but performs more extensive validation checks.
    ///
    /// Argument: config in YAML format as a string.
    ValidateConfig { value: String },

    /// Like [`ReadConfigJson`](Self::ReadConfigJson) but performs more extensive validation checks.
    ///
    /// Argument: config in JSON format as a string.
    ValidateConfigJson { value: String },

    /// Read the active configuration as JSON.
    GetConfigJson,

    /// Get the path of the currently loaded config file.
    GetConfigFilePath,

    // ── State file ────────────────────────────────────────────────────────
    /// Get the path of the state file, if one is configured.
    GetStateFilePath,

    /// Check whether all pending changes have been saved to the state file.
    GetStateFileUpdated,

    // ── Signal levels ─────────────────────────────────────────────────────
    /// Get the peak-to-peak signal range of the last processed chunk.
    ///
    /// A value of 2.0 means full level (signal swings from −1.0 to +1.0).
    GetSignalRange,

    /// Get the RMS level of the last chunk on the capture side, per channel.
    GetCaptureSignalRms,

    /// Get the RMS level averaged over the last `n` seconds on the capture side, per channel.
    ///
    /// Argument: time window in seconds as a float.
    GetCaptureSignalRmsSince { value: f32 },

    /// Get the RMS level since the last call to this command from this client, per channel.
    ///
    /// On the first call, returns values since the client connected.
    /// If called again before new data is available, returns an empty list.
    GetCaptureSignalRmsSinceLast,

    /// Get the peak level of the last chunk on the capture side, per channel.
    GetCaptureSignalPeak,

    /// Get the peak level over the last `n` seconds on the capture side, per channel.
    ///
    /// Argument: time window in seconds as a float.
    GetCaptureSignalPeakSince { value: f32 },

    /// Get the peak level since the last call to this command from this client, per channel.
    GetCaptureSignalPeakSinceLast,

    /// Get the RMS level of the last chunk on the playback side, per channel.
    GetPlaybackSignalRms,

    /// Get the RMS level averaged over the last `n` seconds on the playback side, per channel.
    ///
    /// Argument: time window in seconds as a float.
    GetPlaybackSignalRmsSince { value: f32 },

    /// Get the RMS level since the last call to this command from this client, per channel.
    GetPlaybackSignalRmsSinceLast,

    /// Get the peak level of the last chunk on the playback side, per channel.
    GetPlaybackSignalPeak,

    /// Get the peak level over the last `n` seconds on the playback side, per channel.
    ///
    /// Argument: time window in seconds as a float.
    GetPlaybackSignalPeakSince { value: f32 },

    /// Get the peak level since the last call to this command from this client, per channel.
    GetPlaybackSignalPeakSinceLast,

    /// Get RMS and peak levels for both sides in a single request.
    GetSignalLevels,

    /// Get RMS and peak levels over the last `n` seconds for both sides.
    ///
    /// Argument: time window in seconds as a float.
    GetSignalLevelsSince { value: f32 },

    /// Get RMS and peak levels since the last call to this command from this client, for both sides.
    GetSignalLevelsSinceLast,

    /// Subscribe to pushed signal level events.
    ///
    /// Argument: which side to receive events for — `"playback"`, `"capture"`, or `"both"`.
    ///
    /// While subscribed, CamillaDSP sends a [`WsReply::SignalLevelsEvent`] message each time a
    /// new chunk is analyzed. The event rate therefore depends on the configured chunk size and
    /// sample rate. Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeSignalLevels { value: WsSignalLevelSide },

    /// Subscribe to smoothed, rate-capped VU-meter level events.
    ///
    /// If `attack` or `release` is out of range the command returns [`WsResult::InvalidValueError`]
    /// and no subscription is started.
    ///
    /// While subscribed, CamillaDSP sends [`WsReply::VuLevelsEvent`] messages containing
    /// smoothed `playback_rms`, `playback_peak`, `capture_rms`, and `capture_peak` vectors.
    /// Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeVuLevels { value: VuSubscription },

    /// Stop an active subscription (signal levels, VU levels, state, or spectrum).
    ///
    /// Returns [`WsResult::InvalidRequestError`] if no subscription is active.
    StopSubscription,

    // ── Processing status ─────────────────────────────────────────────────
    /// Subscribe to pushed processing state change events.
    ///
    /// While subscribed, CamillaDSP sends a [`WsReply::StateEvent`] message whenever the
    /// processing state changes. The event payload always contains `state`. When the state is
    /// `"Inactive"` it also contains `stop_reason`.
    ///
    /// Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeState,

    /// Get the peak capture and playback levels measured since processing started.
    GetSignalPeaksSinceStart,

    /// Reset the peak-since-start counters. Affects all connected clients.
    ResetSignalPeaksSinceStart,

    /// Get the optional display labels for capture and playback channels.
    GetChannelLabels,

    /// Get the measured sample rate of the capture device.
    GetCaptureRate,

    /// Get the update interval for capture rate and signal range polling.
    GetUpdateInterval,

    /// Set the update interval for capture rate and signal range polling.
    ///
    /// Argument: interval in milliseconds as an integer.
    SetUpdateInterval { value: usize },

    // ── Volume control (Main fader) ───────────────────────────────────────
    /// Get the current volume of the Main fader.
    GetVolume,

    /// Set the volume of the Main fader. Clamped to −150 to +50 dB.
    ///
    /// Argument: volume in dB as a float.
    SetVolume { value: f32 },

    /// Adjust the volume of the Main fader by `value` dB.
    ///
    /// The optional `min` and `max` fields clamp the resulting volume; when omitted they default
    /// to the global −150 to +50 dB range.
    AdjustVolume {
        value: f32,
        #[serde(default)]
        min: Option<f32>,
        #[serde(default)]
        max: Option<f32>,
    },

    /// Get the mute state of the Main fader.
    GetMute,

    /// Set the mute state of the Main fader.
    ///
    /// Argument: `true` to mute, `false` to unmute.
    SetMute { value: bool },

    /// Toggle the mute state of the Main fader.
    ToggleMute,

    // ── Volume control (faders) ───────────────────────────────────────────
    /// Get the volume and mute state of all faders in a single request.
    GetFaders,

    /// Get the volume of a specific fader.
    ///
    /// Field: `fader` index — 0 for Main, 1–4 for Aux1–Aux4.
    GetFaderVolume { fader: usize },

    /// Set the volume of a specific fader. Clamped to −150 to +50 dB.
    ///
    /// Fields: `fader` index and `value` (volume in dB).
    SetFaderVolume { fader: usize, value: f32 },

    /// Special volume setter for use with a Loudness filter and an external volume control
    /// (without a Volume filter). Clamped to −150 to +50 dB.
    ///
    /// Fields: `fader` index and `value` (volume in dB).
    SetFaderExternalVolume { fader: usize, value: f32 },

    /// Adjust the volume of a specific fader by `value` dB.
    ///
    /// Fields: `fader` index and `value` (delta in dB). The optional `min` and `max` fields clamp
    /// the resulting volume; when omitted they default to the global −150 to +50 dB range.
    AdjustFaderVolume {
        fader: usize,
        value: f32,
        #[serde(default)]
        min: Option<f32>,
        #[serde(default)]
        max: Option<f32>,
    },

    /// Get the mute state of a specific fader.
    ///
    /// Field: `fader` index.
    GetFaderMute { fader: usize },

    /// Set the mute state of a specific fader.
    ///
    /// Fields: `fader` index and `value` (`true` to mute).
    SetFaderMute { fader: usize, value: bool },

    /// Toggle the mute state of a specific fader.
    ///
    /// Field: `fader` index.
    ToggleFaderMute { fader: usize },

    // ── General ───────────────────────────────────────────────────────────
    /// Get the CamillaDSP version string.
    GetVersion,

    /// Get the current processing state.
    GetState,

    /// Get the reason processing last stopped.
    GetStopReason,

    /// Get the current adjustment factor applied to the asynchronous resampler.
    GetRateAdjust,

    /// Get the number of samples that have been clipped since the config was loaded.
    GetClippedSamples,

    /// Reset the clipped-samples counter to zero.
    ResetClippedSamples,

    /// Get the current playback device buffer level when rate adjust is enabled.
    GetBufferLevel,

    /// Get the list of supported playback and capture device types.
    GetSupportedDeviceTypes,

    // ── Audio device listing ──────────────────────────────────────────────
    /// List available capture devices for a given backend.
    ///
    /// Field: `backend` name — one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
    GetAvailableCaptureDevices { backend: String },

    /// List available playback devices for a given backend.
    ///
    /// Field: `backend` name — one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
    GetAvailablePlaybackDevices { backend: String },

    /// Get the capabilities of a specific capture device.
    ///
    /// Fields: `backend` and `device` names.
    ///
    /// Errors: [`WsResult::DeviceNotFoundError`], [`WsResult::DeviceBusyError`], [`WsResult::DeviceError`].
    GetCaptureDeviceCapabilities { backend: String, device: String },

    /// Get the capabilities of a specific playback device.
    ///
    /// Fields: `backend` and `device` names.
    ///
    /// Errors: [`WsResult::DeviceNotFoundError`], [`WsResult::DeviceBusyError`], [`WsResult::DeviceError`].
    GetPlaybackDeviceCapabilities { backend: String, device: String },

    // ── Performance ───────────────────────────────────────────────────────
    /// Get the current pipeline processing load.
    GetProcessingLoad,

    /// Get the current resampler processing load.
    GetResamplerLoad,

    // ── Spectrum analysis ─────────────────────────────────────────────────
    /// Compute a one-shot frequency spectrum from the audio currently passing through the pipeline.
    GetSpectrum { value: SpectrumRequest },

    /// Subscribe to pushed spectrum events.
    ///
    /// If processing is not running when this is sent, the result is
    /// [`WsResult::ProcessingNotRunningError`] and no subscription is started.
    ///
    /// While subscribed, CamillaDSP sends [`WsReply::SpectrumEvent`] each time a new spectrum is
    /// ready. If processing stops, a final event with [`WsResult::ProcessingStopped`] is sent and
    /// the subscription is cancelled. Resubscribe once processing has resumed.
    ///
    /// Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeSpectrum { value: SpectrumSubscription },

    // ── Shutdown ──────────────────────────────────────────────────────────
    /// Stop processing and exit CamillaDSP.
    Exit,

    /// Stop processing and wait for a new configuration to be uploaded
    /// via [`SetConfig`](Self::SetConfig) or [`SetConfigFilePath`](Self::SetConfigFilePath) +
    /// [`Reload`](Self::Reload).
    Stop,

    /// Internal sentinel. Not a valid command from clients.
    #[doc(hidden)]
    None,
}

/// Result status returned in every websocket response.
///
/// Flattened into each [`WsReply`] variant, so its tag becomes the reply's `"result"` field
/// (always a plain string) and any message rides alongside as a top-level `"message"` field.
/// See the [module-level documentation](self) for the full response format.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result")]
pub(crate) enum WsResult {
    /// The command succeeded.
    Ok,
    /// CamillaDSP is shutting down and cannot handle the request.
    ShutdownInProgressError,
    /// Too many requests were sent in a short time.
    RateLimitExceededError,
    /// The request referred to a fader index that does not exist.
    InvalidFaderError,
    /// The configuration could be parsed but contains a logical error.
    ///
    /// Includes a message describing the problem.
    ConfigValidationError { message: String },
    /// The configuration could not be read (file missing, YAML/JSON syntax error, etc.).
    ///
    /// Includes a message describing the problem.
    ConfigReadError { message: String },
    /// A parameter value was outside the accepted range.
    ///
    /// Includes a message describing the problem.
    InvalidValueError { message: String },
    /// The request itself was malformed or not valid in the current state.
    ///
    /// Includes a message describing the problem.
    InvalidRequestError { message: String },
    /// The named audio device does not exist.
    ///
    /// The `message` contains the device name.
    DeviceNotFoundError { message: String },
    /// The audio device is currently in use and cannot be probed.
    ///
    /// The `message` contains the device name.
    DeviceBusyError { message: String },
    /// The device probe failed for another reason.
    ///
    /// The `message` contains a description.
    DeviceError { message: String },
    /// Processing stopped while a subscription was active.
    ///
    /// Sent as the final event of a spectrum subscription when processing stops.
    ProcessingStopped,
    /// Processing is not currently running.
    ///
    /// Returned by [`WsCommand::SubscribeSpectrum`] when processing is inactive.
    ProcessingNotRunningError,
}

/// Channel display labels returned by [`WsCommand::GetChannelLabels`].
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct ChannelLabels {
    /// Labels for playback channels. `null` if no labels are configured. Each entry is a label
    /// string, or `null` if that specific channel has no label.
    pub(crate) playback: Option<Vec<Option<String>>>,
    /// Labels for capture channels. Same structure as `playback`.
    pub(crate) capture: Option<Vec<Option<String>>>,
}

/// Combined RMS and peak levels for both sides, returned by the `GetSignalLevels*` commands.
///
/// All values are in dB (0 dB = full level), one entry per channel.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct AllLevels {
    /// RMS level per playback channel in dB.
    pub(crate) playback_rms: Vec<f32>,
    /// Peak level per playback channel in dB.
    pub(crate) playback_peak: Vec<f32>,
    /// RMS level per capture channel in dB.
    pub(crate) capture_rms: Vec<f32>,
    /// Peak level per capture channel in dB.
    pub(crate) capture_peak: Vec<f32>,
}

/// Peak levels for playback and capture sides, returned by [`WsCommand::GetSignalPeaksSinceStart`].
///
/// All values are in dB, one entry per channel.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct PbCapLevels {
    /// Peak level per playback channel in dB, measured since processing started.
    pub(crate) playback: Vec<f32>,
    /// Peak level per capture channel in dB, measured since processing started.
    pub(crate) capture: Vec<f32>,
}

/// Volume and mute state for one fader, as returned by [`WsCommand::GetFaders`].
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Fader {
    /// Current volume in dB.
    pub(crate) volume: f32,
    /// Whether the fader is muted.
    pub(crate) mute: bool,
}

/// Payload of a [`WsReply::SignalLevelsEvent`] pushed by [`WsCommand::SubscribeSignalLevels`].
///
/// All dB values are per-channel, 0 dB = full level.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct StreamLevels {
    /// Which side these levels belong to.
    pub(crate) side: WsSignalLevelSide,
    pub(crate) rms: Vec<f32>,
    pub(crate) peak: Vec<f32>,
}

/// Payload of a [`WsReply::VuLevelsEvent`] pushed by [`WsCommand::SubscribeVuLevels`].
///
/// All values are smoothed dB levels, per channel.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct VuLevels {
    pub(crate) playback_rms: Vec<f32>,
    pub(crate) playback_peak: Vec<f32>,
    pub(crate) capture_rms: Vec<f32>,
    pub(crate) capture_peak: Vec<f32>,
}

/// Payload of a [`WsReply::StateEvent`] pushed by [`WsCommand::SubscribeState`].
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct StateUpdate {
    pub(crate) state: ProcessingState,
    /// Present only when `state` is `Inactive`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stop_reason: Option<StopReason>,
}

/// All possible reply messages sent by the websocket server.
///
/// Each variant mirrors the corresponding [`WsCommand`]. Every reply is a JSON object with a
/// `"reply"` field holding the reply name, e.g. `{"reply": "GetVersion", "result": "Ok",
/// "value": "2.0.0"}`.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "reply")]
pub(crate) enum WsReply {
    SetConfigFilePath {
        #[serde(flatten)]
        result: WsResult,
    },
    SetConfig {
        #[serde(flatten)]
        result: WsResult,
    },
    SetConfigJson {
        #[serde(flatten)]
        result: WsResult,
    },
    PatchConfig {
        #[serde(flatten)]
        result: WsResult,
    },
    SetConfigValue {
        #[serde(flatten)]
        result: WsResult,
    },
    Reload {
        #[serde(flatten)]
        result: WsResult,
    },
    GetConfig {
        #[serde(flatten)]
        result: WsResult,
        /// Active config in YAML format.
        value: String,
    },
    GetConfigJson {
        #[serde(flatten)]
        result: WsResult,
        /// Active config in JSON format.
        value: String,
    },
    GetConfigValue {
        #[serde(flatten)]
        result: WsResult,
        /// Value at the specified JSON Pointer path.
        value: serde_json::Value,
    },
    GetConfigTitle {
        #[serde(flatten)]
        result: WsResult,
        /// Title string from the active config.
        value: String,
    },
    GetConfigDescription {
        #[serde(flatten)]
        result: WsResult,
        /// Description string from the active config.
        value: String,
    },
    GetPreviousConfig {
        #[serde(flatten)]
        result: WsResult,
        /// Previously active config in YAML format.
        value: String,
    },
    ReadConfig {
        #[serde(flatten)]
        result: WsResult,
        /// Config with all optional fields filled with defaults, or an error message.
        value: String,
    },
    ReadConfigJson {
        #[serde(flatten)]
        result: WsResult,
        /// Config with all optional fields filled with defaults, or an error message.
        value: String,
    },
    ReadConfigFile {
        #[serde(flatten)]
        result: WsResult,
        /// Config with all optional fields filled with defaults, or an error message.
        value: String,
    },
    ValidateConfig {
        #[serde(flatten)]
        result: WsResult,
        /// Validated config with defaults, or an error message.
        value: String,
    },
    ValidateConfigJson {
        #[serde(flatten)]
        result: WsResult,
        /// Validated config with defaults, or an error message.
        value: String,
    },
    GetConfigFilePath {
        #[serde(flatten)]
        result: WsResult,
        /// File path of the active config, or `null` if no file is loaded.
        value: Option<String>,
    },
    GetStateFilePath {
        #[serde(flatten)]
        result: WsResult,
        /// File path of the state file, or `null` if no state file is used.
        value: Option<String>,
    },
    GetStateFileUpdated {
        #[serde(flatten)]
        result: WsResult,
        /// `true` if all changes have been saved to the state file.
        value: bool,
    },
    GetSignalRange {
        #[serde(flatten)]
        result: WsResult,
        /// Peak-to-peak amplitude range of the last chunk (2.0 = full level).
        value: f32,
    },
    GetPlaybackSignalRms {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per playback channel in dB (0 dB = full level).
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSince {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per playback channel in dB, averaged over the requested window.
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSinceLast {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per playback channel in dB since the last call; empty if no new data.
        value: Vec<f32>,
    },
    GetPlaybackSignalPeak {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per playback channel in dB (0 dB = full level).
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSince {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per playback channel in dB over the requested window.
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSinceLast {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per playback channel in dB since the last call; empty if no new data.
        value: Vec<f32>,
    },
    GetCaptureSignalRms {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per capture channel in dB (0 dB = full level).
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSince {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per capture channel in dB, averaged over the requested window.
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSinceLast {
        #[serde(flatten)]
        result: WsResult,
        /// RMS level per capture channel in dB since the last call; empty if no new data.
        value: Vec<f32>,
    },
    GetCaptureSignalPeak {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per capture channel in dB (0 dB = full level).
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSince {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per capture channel in dB over the requested window.
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSinceLast {
        #[serde(flatten)]
        result: WsResult,
        /// Peak level per capture channel in dB since the last call; empty if no new data.
        value: Vec<f32>,
    },
    GetSignalLevels {
        #[serde(flatten)]
        result: WsResult,
        /// RMS and peak levels for both sides.
        value: AllLevels,
    },
    GetSignalLevelsSince {
        #[serde(flatten)]
        result: WsResult,
        /// RMS and peak levels for both sides, averaged over the requested window.
        value: AllLevels,
    },
    GetSignalLevelsSinceLast {
        #[serde(flatten)]
        result: WsResult,
        /// RMS and peak levels for both sides since the last call; empty if no new data.
        value: AllLevels,
    },
    SubscribeSignalLevels {
        #[serde(flatten)]
        result: WsResult,
    },
    SubscribeVuLevels {
        #[serde(flatten)]
        result: WsResult,
    },
    SubscribeState {
        #[serde(flatten)]
        result: WsResult,
    },
    StopSubscription {
        #[serde(flatten)]
        result: WsResult,
    },
    /// Pushed to subscribed clients each time the signal levels are updated.
    SignalLevelsEvent {
        #[serde(flatten)]
        result: WsResult,
        /// Levels for the subscribed side.
        value: StreamLevels,
    },
    /// Pushed to subscribed clients each time smoothed VU levels are updated.
    VuLevelsEvent {
        #[serde(flatten)]
        result: WsResult,
        /// Smoothed RMS and peak levels for both sides.
        value: VuLevels,
    },
    /// Pushed to subscribed clients each time the processing state changes.
    StateEvent {
        #[serde(flatten)]
        result: WsResult,
        /// New processing state, with stop reason if the state is `Inactive`.
        value: StateUpdate,
    },
    GetSignalPeaksSinceStart {
        #[serde(flatten)]
        result: WsResult,
        /// Peak levels since processing started, for both sides.
        value: PbCapLevels,
    },
    ResetSignalPeaksSinceStart {
        #[serde(flatten)]
        result: WsResult,
    },
    GetChannelLabels {
        #[serde(flatten)]
        result: WsResult,
        /// Display labels for capture and playback channels.
        value: ChannelLabels,
    },
    GetCaptureRate {
        #[serde(flatten)]
        result: WsResult,
        /// Measured capture sample rate in Hz.
        value: usize,
    },
    GetUpdateInterval {
        #[serde(flatten)]
        result: WsResult,
        /// Update interval in milliseconds.
        value: usize,
    },
    SetUpdateInterval {
        #[serde(flatten)]
        result: WsResult,
    },
    SetVolume {
        #[serde(flatten)]
        result: WsResult,
    },
    GetVolume {
        #[serde(flatten)]
        result: WsResult,
        /// Current volume in dB.
        value: f32,
    },
    AdjustVolume {
        #[serde(flatten)]
        result: WsResult,
        /// New volume in dB after the adjustment.
        value: f32,
    },
    SetMute {
        #[serde(flatten)]
        result: WsResult,
    },
    GetMute {
        #[serde(flatten)]
        result: WsResult,
        /// `true` if muted.
        value: bool,
    },
    ToggleMute {
        #[serde(flatten)]
        result: WsResult,
        /// New mute state after the toggle.
        value: bool,
    },
    SetFaderVolume {
        #[serde(flatten)]
        result: WsResult,
    },
    SetFaderExternalVolume {
        #[serde(flatten)]
        result: WsResult,
    },
    GetFaders {
        #[serde(flatten)]
        result: WsResult,
        /// List of faders: Main (index 0) followed by Aux1–Aux4 (indices 1–4).
        value: Vec<Fader>,
    },
    GetFaderVolume {
        #[serde(flatten)]
        result: WsResult,
        /// `[fader_index, volume_dB]`.
        value: (usize, f32),
    },
    AdjustFaderVolume {
        #[serde(flatten)]
        result: WsResult,
        /// `[fader_index, new_volume_dB]` after the adjustment.
        value: (usize, f32),
    },
    SetFaderMute {
        #[serde(flatten)]
        result: WsResult,
    },
    GetFaderMute {
        #[serde(flatten)]
        result: WsResult,
        /// `[fader_index, is_muted]`.
        value: (usize, bool),
    },
    ToggleFaderMute {
        #[serde(flatten)]
        result: WsResult,
        /// `[fader_index, new_mute_state]` after the toggle.
        value: (usize, bool),
    },
    GetVersion {
        #[serde(flatten)]
        result: WsResult,
        /// Version string, e.g. `"2.0.0"`.
        value: String,
    },
    GetState {
        #[serde(flatten)]
        result: WsResult,
        /// Current processing state.
        value: ProcessingState,
    },
    GetStopReason {
        #[serde(flatten)]
        result: WsResult,
        /// Reason the processing last stopped.
        value: StopReason,
    },
    GetRateAdjust {
        #[serde(flatten)]
        result: WsResult,
        /// Rate adjustment factor applied to the async resampler (1.0 = no adjustment).
        value: f32,
    },
    GetBufferLevel {
        #[serde(flatten)]
        result: WsResult,
        /// Playback device buffer fill level in frames; 0 if rate adjust is not enabled.
        value: usize,
    },
    GetClippedSamples {
        #[serde(flatten)]
        result: WsResult,
        /// Number of clipped samples since the config was loaded.
        value: usize,
    },
    ResetClippedSamples {
        #[serde(flatten)]
        result: WsResult,
    },
    GetSupportedDeviceTypes {
        #[serde(flatten)]
        result: WsResult,
        /// `[list_of_playback_types, list_of_capture_types]`.
        value: (Vec<String>, Vec<String>),
    },
    GetAvailableCaptureDevices {
        #[serde(flatten)]
        result: WsResult,
        /// List of `[identifier, name_or_null]` pairs.
        value: Vec<(String, String)>,
    },
    GetAvailablePlaybackDevices {
        #[serde(flatten)]
        result: WsResult,
        /// List of `[identifier, name_or_null]` pairs.
        value: Vec<(String, String)>,
    },
    GetCaptureDeviceCapabilities {
        #[serde(flatten)]
        result: WsResult,
        /// Capabilities of the requested capture device.
        value: AudioDeviceDescriptor,
    },
    GetPlaybackDeviceCapabilities {
        #[serde(flatten)]
        result: WsResult,
        /// Capabilities of the requested playback device.
        value: AudioDeviceDescriptor,
    },
    GetProcessingLoad {
        #[serde(flatten)]
        result: WsResult,
        /// Pipeline processing load in percent.
        value: f32,
    },
    GetResamplerLoad {
        #[serde(flatten)]
        result: WsResult,
        /// Resampler processing load in percent.
        value: f32,
    },
    GetSpectrum {
        #[serde(flatten)]
        result: WsResult,
        /// Computed spectrum with frequency and magnitude arrays.
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<SpectrumData>,
    },
    SubscribeSpectrum {
        #[serde(flatten)]
        result: WsResult,
    },
    /// Pushed to subscribed clients each time a new spectrum is ready.
    SpectrumEvent {
        #[serde(flatten)]
        result: WsResult,
        /// Computed spectrum, or absent if processing has stopped.
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<SpectrumData>,
    },
    Exit {
        #[serde(flatten)]
        result: WsResult,
    },
    Stop {
        #[serde(flatten)]
        result: WsResult,
    },
    /// Sent when the server cannot parse or dispatch the incoming command.
    Invalid { error: String },
}
