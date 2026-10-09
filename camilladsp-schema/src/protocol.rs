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

//! The messages of the CamillaDSP websocket protocol.
//!
//! The server deserializes [`WsCommand`](crate::protocol::WsCommand) and serializes
//! [`WsReply`](crate::protocol::WsReply), and a client does the opposite with the same types. All messages are UTF-8 text frames containing a JSON value.
//!
//! ## Command syntax
//!
//! Every command is a JSON object with a `"command"` field naming the command:
//! ```json
//! {"command": "GetVersion"}
//! ```
//!
//! Commands with arguments carry them in additional named fields:
//! ```json
//! {"command": "SetUpdateInterval", "value": 500}
//! ```
//!
//! ## Response format
//!
//! Every reply is a JSON object with a `"reply"` field naming the reply. Replies that do not
//! return a value carry only the `"result"` status:
//! ```json
//! {"reply": "SetUpdateInterval", "result": "Ok"}
//! ```
//!
//! Replies that return a value add a `"value"` field:
//! ```json
//! {"reply": "GetUpdateInterval", "result": "Ok", "value": 500}
//! ```
//!
//! If a command fails the `"result"` field holds the error name instead of `"Ok"`, and there is
//! no `"value"` field. Errors that carry a description add a top-level `"message"` field:
//! ```json
//! {"reply": "SetConfig", "result": "ConfigValidationError", "message": "details..."}
//! ```
//! Errors without a message have just the name: `{"reply": "SetFaderVolume", "result": "InvalidFaderError"}`.
//!
//! Unrecognised commands get a `{"reply": "Invalid", "error": "..."}` response.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

// ── State and device types that replies carry ──────────────────────────────

/// The state of the processing, as reported by [`WsCommand::GetState`].
///
/// - `Running`: processing is running normally.
/// - `Paused`: processing is paused because the input signal is silent.
/// - `Inactive`: processing is off and devices are closed, waiting for a new configuration.
/// - `Starting`: opening devices and starting up processing with a new configuration.
/// - `Stalled`: the capture device is not providing data, so processing is stalled.
// The values are described on the enum rather than on each variant, since the OpenAPI schema
// of a unit-only enum has no per-variant descriptions. The same goes for CapabilityMode.
#[derive(Clone, Debug, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum ProcessingState {
    Running,
    Paused,
    Inactive,
    Starting,
    Stalled,
}

impl fmt::Display for ProcessingState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = match self {
            ProcessingState::Running => "RUNNING",
            ProcessingState::Paused => "PAUSED",
            ProcessingState::Inactive => "INACTIVE",
            ProcessingState::Starting => "STARTING",
            ProcessingState::Stalled => "STALLED",
        };
        write!(f, "{desc}")
    }
}

/// Reason a processing run ended.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum StopReason {
    /// Processing is still running; not yet stopped.
    None,
    /// Processing completed normally (e.g. end of file input).
    Done,
    /// Capture device reported an error.
    CaptureError(String),
    /// Playback device reported an error.
    PlaybackError(String),
    /// An unexpected internal error occurred.
    UnknownError(String),
    /// Capture device sample rate changed to the given value.
    CaptureFormatChange(usize),
    /// Playback device sample rate changed to the given value.
    PlaybackFormatChange(usize),
}

/// The sample formats supported by a device at a specific sample rate.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SamplerateCapability {
    /// Sample rate in Hz.
    pub samplerate: usize,
    /// Names of the supported sample formats at this rate.
    pub formats: Vec<String>,
}

/// The sample rates (and their formats) supported by a device at a specific channel count.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ChannelCapability {
    /// Number of channels.
    pub channels: usize,
    /// Supported sample rates for this channel count.
    pub samplerates: Vec<SamplerateCapability>,
}

/// The access mode a [`DeviceCapabilitySet`] was probed under.
///
/// - `Unified`: the device uses a unified capability model (ALSA, CoreAudio, ASIO).
/// - `Shared`: WASAPI shared-mode capabilities, derived from the mix format.
/// - `Exclusive`: WASAPI exclusive-mode capabilities, probed independently.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum CapabilityMode {
    Unified,
    Shared,
    Exclusive,
}

/// A set of device capabilities associated with a single access mode (e.g. exclusive vs. shared).
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct DeviceCapabilitySet {
    /// The access mode these capabilities were probed under.
    pub mode: CapabilityMode,
    /// Per-channel-count capability entries.
    pub capabilities: Vec<ChannelCapability>,
}

/// Full capability descriptor for a named audio device.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AudioDeviceDescriptor {
    /// Backend-specific device identifier (e.g. `"hw:0,0"` for ALSA).
    pub name: String,
    /// Human-readable device name.
    pub description: String,
    /// Capability sets, one per access mode supported by the backend.
    pub capability_sets: Vec<DeviceCapabilitySet>,
}

/// Log-spaced spectrum, as returned by [`WsCommand::GetSpectrum`].
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SpectrumData {
    /// Center frequency of each output bin in Hz.
    #[cfg_attr(feature = "utoipa", schema(value_type = Vec<f32>))]
    pub frequencies: Arc<[f32]>,
    /// Per-bin peak magnitude in dBFS (0 dBFS = full-scale sine wave).
    pub magnitudes: Vec<f32>,
}

// ── Commands and replies ───────────────────────────────────────────────────

/// Side selector for [`WsCommand::SubscribeSignalLevels`] subscriptions.
///
/// Serialised as a lowercase string: `"playback"`, `"capture"`, or `"both"`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum WsSignalLevelSide {
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
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum SpectrumSide {
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
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SpectrumRequest {
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
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema, utoipa::IntoParams))]
pub struct SpectrumSubscription {
    /// Which side to analyze: `"capture"` or `"playback"`.
    // As a query parameter it is inlined, since a parameter's $ref is not added to the spec.
    #[cfg_attr(feature = "utoipa", param(inline))]
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
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VuSubscription {
    /// Maximum event rate in Hz. A value ≤ 0 disables rate limiting.
    ///
    /// If set higher than the natural update rate, events are sent at the natural rate.
    pub max_rate: f32,
    /// Attack time constant in ms for rising values. Valid range: 0–60000. `0` disables smoothing.
    ///
    /// A smaller value gives a faster, more responsive meter on rising signals.
    /// For peak values, upward changes are always applied immediately regardless of this setting.
    pub attack: f32,
    /// Release time constant in ms for falling values. Valid range: 0–60000. `0` disables smoothing.
    ///
    /// A smaller value makes the meter drop faster; a larger value gives a slower decay.
    /// A good starting point for an analog-feel meter is around 300 ms.
    pub release: f32,
}

/// All commands accepted by the websocket server.
///
/// Every command is a JSON object with a `"command"` field holding the command name.
/// Commands with arguments carry them in additional named fields, e.g.
/// `{"command": "SetUpdateInterval", "value": 500}`.
///
/// See the [module-level documentation](self) for the general message format.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum WsCommand {
    // ── Config management ──────────────────────────────────────────────────
    /// Change the active config file path. Not applied until [`Reload`](Self::Reload) is called.
    SetConfigFilePath {
        /// Path of the config file.
        value: String,
    },

    /// Upload and immediately apply a new configuration as a YAML string.
    SetConfig {
        /// Config in YAML format.
        value: String,
    },

    /// Upload and immediately apply a new configuration as a JSON string.
    SetConfigJson {
        /// Config in JSON format.
        value: String,
    },

    /// Apply a partial patch to the active configuration.
    ///
    /// If the resulting config is valid it is applied immediately.
    PatchConfig {
        /// Partial config object containing only the fields to change.
        value: serde_json::Value,
    },

    /// Set a single value in the active configuration using a JSON Pointer (RFC 6901).
    SetConfigValue {
        /// JSON Pointer to the value, such as `"/devices/samplerate"`.
        pointer: String,
        /// The value to store there.
        value: serde_json::Value,
    },

    /// Reload the current config file from disk. Equivalent to sending `SIGHUP`.
    Reload,

    /// Read the active configuration.
    GetConfig,

    /// Read a single value from the active configuration using a JSON Pointer (RFC 6901).
    GetConfigValue {
        /// JSON Pointer to the value, such as `"/devices/samplerate"`.
        value: String,
    },

    /// Read the `title` field from the active configuration.
    GetConfigTitle,

    /// Read the `description` field from the active configuration.
    GetConfigDescription,

    /// Read the previously active configuration (before the last reload or upload).
    GetPreviousConfig,

    /// Parse and fill defaults for a YAML config string without changing the active config.
    ReadConfig {
        /// Config in YAML format.
        value: String,
    },

    /// Parse and fill defaults for a JSON config string without changing the active config.
    ReadConfigJson {
        /// Config in JSON format.
        value: String,
    },

    /// Parse and fill defaults for a config file without changing the active config.
    ReadConfigFile {
        /// Path of the config file.
        value: String,
    },

    /// Like [`ReadConfig`](Self::ReadConfig) but performs more extensive validation checks.
    ValidateConfig {
        /// Config in YAML format.
        value: String,
    },

    /// Like [`ReadConfigJson`](Self::ReadConfigJson) but performs more extensive validation checks.
    ValidateConfigJson {
        /// Config in JSON format.
        value: String,
    },

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

    /// Get the RMS level averaged over the last `value` seconds on the capture side, per channel.
    GetCaptureSignalRmsSince {
        /// Time window in seconds.
        value: f32,
    },

    /// Get the RMS level since the last call to this command from this client, per channel.
    ///
    /// On the first call, returns values since the client connected.
    /// If called again before new data is available, returns an empty list.
    GetCaptureSignalRmsSinceLast,

    /// Get the peak level of the last chunk on the capture side, per channel.
    GetCaptureSignalPeak,

    /// Get the peak level over the last `value` seconds on the capture side, per channel.
    GetCaptureSignalPeakSince {
        /// Time window in seconds.
        value: f32,
    },

    /// Get the peak level since the last call to this command from this client, per channel.
    GetCaptureSignalPeakSinceLast,

    /// Get the RMS level of the last chunk on the playback side, per channel.
    GetPlaybackSignalRms,

    /// Get the RMS level averaged over the last `value` seconds on the playback side, per channel.
    GetPlaybackSignalRmsSince {
        /// Time window in seconds.
        value: f32,
    },

    /// Get the RMS level since the last call to this command from this client, per channel.
    GetPlaybackSignalRmsSinceLast,

    /// Get the peak level of the last chunk on the playback side, per channel.
    GetPlaybackSignalPeak,

    /// Get the peak level over the last `value` seconds on the playback side, per channel.
    GetPlaybackSignalPeakSince {
        /// Time window in seconds.
        value: f32,
    },

    /// Get the peak level since the last call to this command from this client, per channel.
    GetPlaybackSignalPeakSinceLast,

    /// Get RMS and peak levels for both sides in a single request.
    GetSignalLevels,

    /// Get RMS and peak levels over the last `value` seconds for both sides.
    GetSignalLevelsSince {
        /// Time window in seconds.
        value: f32,
    },

    /// Get RMS and peak levels since the last call to this command from this client, for both sides.
    GetSignalLevelsSinceLast,

    /// Subscribe to pushed signal level events.
    ///
    /// While subscribed, CamillaDSP sends a [`WsReply::SignalLevelsEvent`] message each time a
    /// new chunk is analyzed. The event rate therefore depends on the configured chunk size and
    /// sample rate. Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeSignalLevels {
        /// Which side to receive events for.
        value: WsSignalLevelSide,
    },

    /// Subscribe to smoothed, rate-capped VU-meter level events.
    ///
    /// If `attack` or `release` is out of range the command returns [`WsResult::InvalidValueError`]
    /// and no subscription is started.
    ///
    /// While subscribed, CamillaDSP sends [`WsReply::VuLevelsEvent`] messages containing
    /// smoothed `playback_rms`, `playback_peak`, `capture_rms`, and `capture_peak` vectors.
    /// Send [`StopSubscription`](Self::StopSubscription) to end the stream.
    SubscribeVuLevels {
        /// Rate limit and smoothing settings.
        value: VuSubscription,
    },

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
    SetUpdateInterval {
        /// Interval in milliseconds.
        value: usize,
    },

    // ── Volume control (Main fader) ───────────────────────────────────────
    /// Get the current volume of the Main fader.
    GetVolume,

    /// Set the volume of the Main fader. Clamped to −150 to +50 dB.
    SetVolume {
        /// Volume in dB.
        value: f32,
    },

    /// Adjust the volume of the Main fader by `value` dB.
    AdjustVolume {
        /// Volume change in dB.
        value: f32,
        /// Lower limit for the resulting volume in dB. Defaults to −150 dB.
        #[serde(default)]
        min: Option<f32>,
        /// Upper limit for the resulting volume in dB. Defaults to +50 dB.
        #[serde(default)]
        max: Option<f32>,
    },

    /// Get the mute state of the Main fader.
    GetMute,

    /// Set the mute state of the Main fader.
    SetMute {
        /// `true` to mute, `false` to unmute.
        value: bool,
    },

    /// Toggle the mute state of the Main fader.
    ToggleMute,

    // ── Volume control (faders) ───────────────────────────────────────────
    /// Get the volume and mute state of all faders in a single request.
    GetFaders,

    /// Get the volume of a specific fader.
    GetFaderVolume {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
    },

    /// Set the volume of a specific fader. Clamped to −150 to +50 dB.
    SetFaderVolume {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
        /// Volume in dB.
        value: f32,
    },

    /// Special volume setter for use with a Loudness filter and an external volume control
    /// (without a Volume filter). Clamped to −150 to +50 dB.
    SetFaderExternalVolume {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
        /// Volume in dB.
        value: f32,
    },

    /// Adjust the volume of a specific fader by `value` dB.
    AdjustFaderVolume {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
        /// Volume change in dB.
        value: f32,
        /// Lower limit for the resulting volume in dB. Defaults to −150 dB.
        #[serde(default)]
        min: Option<f32>,
        /// Upper limit for the resulting volume in dB. Defaults to +50 dB.
        #[serde(default)]
        max: Option<f32>,
    },

    /// Get the mute state of a specific fader.
    GetFaderMute {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
    },

    /// Set the mute state of a specific fader.
    SetFaderMute {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
        /// `true` to mute, `false` to unmute.
        value: bool,
    },

    /// Toggle the mute state of a specific fader.
    ToggleFaderMute {
        /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
        fader: usize,
    },

    // ── General ───────────────────────────────────────────────────────────
    /// Get the CamillaDSP version string.
    GetVersion,

    /// Get the current processing state.
    GetState,

    /// Get the reason processing last stopped.
    GetStopReason,

    /// Get the current adjustment factor applied to the asynchronous resampler.
    GetRateAdjust,

    /// Get the number of samples that have been clipped since start, or since the
    /// counter was last reset. Loading a new config does not reset it.
    GetClippedSamples,

    /// Reset the clipped-samples counter to zero.
    ResetClippedSamples,

    /// Get the current playback device buffer level when rate adjust is enabled.
    GetBufferLevel,

    /// Get the list of supported playback and capture device types.
    GetSupportedDeviceTypes,

    // ── Audio device listing ──────────────────────────────────────────────
    /// List available capture devices for a given backend.
    GetAvailableCaptureDevices {
        /// Backend name, one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
        backend: String,
    },

    /// List available playback devices for a given backend.
    GetAvailablePlaybackDevices {
        /// Backend name, one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
        backend: String,
    },

    /// Get the capabilities of a specific capture device.
    ///
    /// Errors: [`WsResult::DeviceNotFoundError`], [`WsResult::DeviceBusyError`], [`WsResult::DeviceError`].
    GetCaptureDeviceCapabilities {
        /// Backend name.
        backend: String,
        /// Device identifier, as listed by
        /// [`GetAvailableCaptureDevices`](Self::GetAvailableCaptureDevices).
        device: String,
    },

    /// Get the capabilities of a specific playback device.
    ///
    /// Errors: [`WsResult::DeviceNotFoundError`], [`WsResult::DeviceBusyError`], [`WsResult::DeviceError`].
    GetPlaybackDeviceCapabilities {
        /// Backend name.
        backend: String,
        /// Device identifier, as listed by
        /// [`GetAvailablePlaybackDevices`](Self::GetAvailablePlaybackDevices).
        device: String,
    },

    // ── Performance ───────────────────────────────────────────────────────
    /// Get the current pipeline processing load.
    GetProcessingLoad,

    /// Get the current resampler processing load.
    GetResamplerLoad,

    // ── Spectrum analysis ─────────────────────────────────────────────────
    /// Compute a one-shot frequency spectrum from the audio currently passing through the pipeline.
    GetSpectrum {
        /// The spectrum to compute.
        value: SpectrumRequest,
    },

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
    SubscribeSpectrum {
        /// The spectrum to compute, and the push rate.
        value: SpectrumSubscription,
    },

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
#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "result")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum WsResult {
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ChannelLabels {
    /// Labels for playback channels. `null` if no labels are configured. Each entry is a label
    /// string, or `null` if that specific channel has no label.
    #[cfg_attr(feature = "utoipa", schema(required))]
    pub playback: Option<Vec<Option<String>>>,
    /// Labels for capture channels. Same structure as `playback`.
    #[cfg_attr(feature = "utoipa", schema(required))]
    pub capture: Option<Vec<Option<String>>>,
}

/// Combined RMS and peak levels for both sides, returned by the `GetSignalLevels*` commands.
///
/// All values are in dB (0 dB = full level), one entry per channel.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AllLevels {
    /// RMS level per playback channel in dB.
    pub playback_rms: Vec<f32>,
    /// Peak level per playback channel in dB.
    pub playback_peak: Vec<f32>,
    /// RMS level per capture channel in dB.
    pub capture_rms: Vec<f32>,
    /// Peak level per capture channel in dB.
    pub capture_peak: Vec<f32>,
}

/// Peak levels for playback and capture sides, returned by [`WsCommand::GetSignalPeaksSinceStart`].
///
/// All values are in dB, one entry per channel.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct PbCapLevels {
    /// Peak level per playback channel in dB, measured since processing started.
    pub playback: Vec<f32>,
    /// Peak level per capture channel in dB, measured since processing started.
    pub capture: Vec<f32>,
}

/// Volume and mute state for one fader, as returned by [`WsCommand::GetFaders`].
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Fader {
    /// Current volume in dB.
    pub volume: f32,
    /// Whether the fader is muted.
    pub mute: bool,
}

/// Payload of a [`WsReply::SignalLevelsEvent`] pushed by [`WsCommand::SubscribeSignalLevels`].
///
/// All dB values are per-channel, 0 dB = full level.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct StreamLevels {
    /// Which side these levels belong to.
    pub side: WsSignalLevelSide,
    /// RMS level per channel in dB.
    pub rms: Vec<f32>,
    /// Peak level per channel in dB.
    pub peak: Vec<f32>,
}

/// Payload of a [`WsReply::VuLevelsEvent`] pushed by [`WsCommand::SubscribeVuLevels`].
///
/// All values are smoothed dB levels, per channel.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct VuLevels {
    /// Smoothed RMS level per playback channel in dB.
    pub playback_rms: Vec<f32>,
    /// Smoothed peak level per playback channel in dB.
    pub playback_peak: Vec<f32>,
    /// Smoothed RMS level per capture channel in dB.
    pub capture_rms: Vec<f32>,
    /// Smoothed peak level per capture channel in dB.
    pub capture_peak: Vec<f32>,
}

/// Payload of a [`WsReply::StateEvent`] pushed by [`WsCommand::SubscribeState`].
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct StateUpdate {
    /// The new processing state.
    pub state: ProcessingState,
    /// Present only when `state` is `Inactive`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
}

/// All possible reply messages sent by the websocket server.
///
/// Each variant mirrors the corresponding [`WsCommand`]. Every reply is a JSON object with a
/// `"reply"` field holding the reply name, e.g. `{"reply": "GetVersion", "result": "Ok",
/// "value": "2.0.0"}`.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "reply")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum WsReply {
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
        /// Number of clipped samples since start or the last reset.
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
        /// List of `[identifier, name]` pairs. Some backends use the identifier as the name.
        value: Vec<(String, String)>,
    },
    GetAvailablePlaybackDevices {
        #[serde(flatten)]
        result: WsResult,
        /// List of `[identifier, name]` pairs. Some backends use the identifier as the name.
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

/// Implements `name()` for a protocol enum. The match is exhaustive, so a new variant does not
/// compile until it is listed here.
macro_rules! variant_names {
    ($enum:ident { $($variant:ident),* $(,)? }) => {
        impl $enum {
            /// The variant name, which is also the value of the tag field
            /// (`"command"` or `"reply"`) in the JSON message.
            pub fn name(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => stringify!($variant),)*
                }
            }

            #[cfg(test)]
            const NAMES: &[&str] = &[$(stringify!($variant)),*];
        }
    };
}

variant_names!(WsCommand {
    SetConfigFilePath,
    SetConfig,
    SetConfigJson,
    PatchConfig,
    SetConfigValue,
    Reload,
    GetConfig,
    GetConfigValue,
    GetConfigTitle,
    GetConfigDescription,
    GetPreviousConfig,
    ReadConfig,
    ReadConfigJson,
    ReadConfigFile,
    ValidateConfig,
    ValidateConfigJson,
    GetConfigJson,
    GetConfigFilePath,
    GetStateFilePath,
    GetStateFileUpdated,
    GetSignalRange,
    GetCaptureSignalRms,
    GetCaptureSignalRmsSince,
    GetCaptureSignalRmsSinceLast,
    GetCaptureSignalPeak,
    GetCaptureSignalPeakSince,
    GetCaptureSignalPeakSinceLast,
    GetPlaybackSignalRms,
    GetPlaybackSignalRmsSince,
    GetPlaybackSignalRmsSinceLast,
    GetPlaybackSignalPeak,
    GetPlaybackSignalPeakSince,
    GetPlaybackSignalPeakSinceLast,
    GetSignalLevels,
    GetSignalLevelsSince,
    GetSignalLevelsSinceLast,
    SubscribeSignalLevels,
    SubscribeVuLevels,
    StopSubscription,
    SubscribeState,
    GetSignalPeaksSinceStart,
    ResetSignalPeaksSinceStart,
    GetChannelLabels,
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval,
    GetVolume,
    SetVolume,
    AdjustVolume,
    GetMute,
    SetMute,
    ToggleMute,
    GetFaders,
    GetFaderVolume,
    SetFaderVolume,
    SetFaderExternalVolume,
    AdjustFaderVolume,
    GetFaderMute,
    SetFaderMute,
    ToggleFaderMute,
    GetVersion,
    GetState,
    GetStopReason,
    GetRateAdjust,
    GetClippedSamples,
    ResetClippedSamples,
    GetBufferLevel,
    GetSupportedDeviceTypes,
    GetAvailableCaptureDevices,
    GetAvailablePlaybackDevices,
    GetCaptureDeviceCapabilities,
    GetPlaybackDeviceCapabilities,
    GetProcessingLoad,
    GetResamplerLoad,
    GetSpectrum,
    SubscribeSpectrum,
    Exit,
    Stop,
    None,
});

variant_names!(WsReply {
    SetConfigFilePath,
    SetConfig,
    SetConfigJson,
    PatchConfig,
    SetConfigValue,
    Reload,
    GetConfig,
    GetConfigJson,
    GetConfigValue,
    GetConfigTitle,
    GetConfigDescription,
    GetPreviousConfig,
    ReadConfig,
    ReadConfigJson,
    ReadConfigFile,
    ValidateConfig,
    ValidateConfigJson,
    GetConfigFilePath,
    GetStateFilePath,
    GetStateFileUpdated,
    GetSignalRange,
    GetPlaybackSignalRms,
    GetPlaybackSignalRmsSince,
    GetPlaybackSignalRmsSinceLast,
    GetPlaybackSignalPeak,
    GetPlaybackSignalPeakSince,
    GetPlaybackSignalPeakSinceLast,
    GetCaptureSignalRms,
    GetCaptureSignalRmsSince,
    GetCaptureSignalRmsSinceLast,
    GetCaptureSignalPeak,
    GetCaptureSignalPeakSince,
    GetCaptureSignalPeakSinceLast,
    GetSignalLevels,
    GetSignalLevelsSince,
    GetSignalLevelsSinceLast,
    SubscribeSignalLevels,
    SubscribeVuLevels,
    SubscribeState,
    StopSubscription,
    SignalLevelsEvent,
    VuLevelsEvent,
    StateEvent,
    GetSignalPeaksSinceStart,
    ResetSignalPeaksSinceStart,
    GetChannelLabels,
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval,
    SetVolume,
    GetVolume,
    AdjustVolume,
    SetMute,
    GetMute,
    ToggleMute,
    SetFaderVolume,
    SetFaderExternalVolume,
    GetFaders,
    GetFaderVolume,
    AdjustFaderVolume,
    SetFaderMute,
    GetFaderMute,
    ToggleFaderMute,
    GetVersion,
    GetState,
    GetStopReason,
    GetRateAdjust,
    GetBufferLevel,
    GetClippedSamples,
    ResetClippedSamples,
    GetSupportedDeviceTypes,
    GetAvailableCaptureDevices,
    GetAvailablePlaybackDevices,
    GetCaptureDeviceCapabilities,
    GetPlaybackDeviceCapabilities,
    GetProcessingLoad,
    GetResamplerLoad,
    GetSpectrum,
    SubscribeSpectrum,
    SpectrumEvent,
    Exit,
    Stop,
    Invalid,
});

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_command(command: WsCommand) {
        let json = serde_json::to_string(&command).unwrap();
        let parsed: WsCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, command, "{json}");
    }

    fn round_trip_reply(reply: WsReply) {
        let json = serde_json::to_string(&reply).unwrap();
        let parsed: WsReply = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reply, "{json}");
    }

    #[test]
    fn commands_round_trip() {
        round_trip_command(WsCommand::GetVersion);
        round_trip_command(WsCommand::SetUpdateInterval { value: 500 });
        round_trip_command(WsCommand::SetFaderVolume {
            fader: 2,
            value: -12.5,
        });
        round_trip_command(WsCommand::SetConfigValue {
            pointer: "/devices/samplerate".to_string(),
            value: serde_json::json!(48000),
        });
        round_trip_command(WsCommand::SubscribeSignalLevels {
            value: WsSignalLevelSide::Both,
        });
        round_trip_command(WsCommand::SubscribeVuLevels {
            value: VuSubscription {
                max_rate: 10.0,
                attack: 10.0,
                release: 300.0,
            },
        });
        round_trip_command(WsCommand::GetSpectrum {
            value: SpectrumRequest {
                side: SpectrumSide::Capture,
                channel: None,
                min_freq: 20.0,
                max_freq: 20000.0,
                n_bins: 100,
            },
        });
        round_trip_command(WsCommand::GetCaptureDeviceCapabilities {
            backend: "Alsa".to_string(),
            device: "hw:0".to_string(),
        });
    }

    #[test]
    fn replies_round_trip() {
        round_trip_reply(WsReply::SetConfig {
            result: WsResult::Ok,
        });
        round_trip_reply(WsReply::SetConfig {
            result: WsResult::ConfigValidationError {
                message: "filters.lp.parameters.freq: Frequency must be > 0".to_string(),
            },
        });
        round_trip_reply(WsReply::GetConfigFilePath {
            result: WsResult::Ok,
            value: None,
        });
        round_trip_reply(WsReply::GetState {
            result: WsResult::Ok,
            value: ProcessingState::Running,
        });
        round_trip_reply(WsReply::StateEvent {
            result: WsResult::Ok,
            value: StateUpdate {
                state: ProcessingState::Inactive,
                stop_reason: Some(StopReason::CaptureError("gone".to_string())),
            },
        });
        round_trip_reply(WsReply::StateEvent {
            result: WsResult::Ok,
            value: StateUpdate {
                state: ProcessingState::Running,
                stop_reason: None,
            },
        });
        round_trip_reply(WsReply::GetFaders {
            result: WsResult::Ok,
            value: vec![Fader {
                volume: -10.0,
                mute: false,
            }],
        });
        round_trip_reply(WsReply::GetSupportedDeviceTypes {
            result: WsResult::Ok,
            value: (vec!["File".to_string()], vec!["RawFile".to_string()]),
        });
        round_trip_reply(WsReply::GetCaptureDeviceCapabilities {
            result: WsResult::Ok,
            value: AudioDeviceDescriptor {
                name: "hw:0".to_string(),
                description: "Card".to_string(),
                capability_sets: vec![DeviceCapabilitySet {
                    mode: CapabilityMode::Unified,
                    capabilities: vec![ChannelCapability {
                        channels: 2,
                        samplerates: vec![SamplerateCapability {
                            samplerate: 48000,
                            formats: vec!["S32_LE".to_string()],
                        }],
                    }],
                }],
            },
        });
        round_trip_reply(WsReply::GetSpectrum {
            result: WsResult::Ok,
            value: Some(SpectrumData {
                frequencies: Arc::from(vec![100.0, 1000.0]),
                magnitudes: vec![-20.0, -30.0],
            }),
        });
        round_trip_reply(WsReply::SpectrumEvent {
            result: WsResult::ProcessingStopped,
            value: None,
        });
        round_trip_reply(WsReply::Invalid {
            error: "bad".to_string(),
        });
    }

    /// Deriving `Deserialize` must not change what goes over the wire.
    #[test]
    fn reply_format_is_unchanged() {
        let reply = WsReply::SetConfig {
            result: WsResult::ConfigReadError {
                message: "x".to_string(),
            },
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"reply":"SetConfig","result":"ConfigReadError","message":"x"}"#
        );
        let reply = WsReply::GetUpdateInterval {
            result: WsResult::Ok,
            value: 500,
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"reply":"GetUpdateInterval","result":"Ok","value":500}"#
        );
    }

    fn tag_of(message: &impl Serialize, field: &str) -> String {
        serde_json::to_value(message).unwrap()[field]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn command_name_is_the_tag() {
        for command in [
            WsCommand::GetVersion,
            WsCommand::SetConfigJson {
                value: "{}".to_string(),
            },
            WsCommand::AdjustFaderVolume {
                fader: 1,
                value: -3.0,
                min: None,
                max: Some(0.0),
            },
            WsCommand::None,
        ] {
            assert_eq!(command.name(), tag_of(&command, "command"));
        }
    }

    #[test]
    fn reply_name_is_the_tag() {
        for reply in [
            WsReply::GetConfigJson {
                result: WsResult::Ok,
                value: "{}".to_string(),
            },
            WsReply::SetConfig {
                result: WsResult::ConfigValidationError {
                    message: "x".to_string(),
                },
            },
            WsReply::StateEvent {
                result: WsResult::Ok,
                value: StateUpdate {
                    state: ProcessingState::Running,
                    stop_reason: None,
                },
            },
            WsReply::Invalid {
                error: "bad".to_string(),
            },
        ] {
            assert_eq!(reply.name(), tag_of(&reply, "reply"));
        }
    }

    /// The tag names serde accepts, read from the error for an unknown tag. It lists them as
    /// "expected one of `A`, `B`, ...".
    fn serde_tags<T: serde::de::DeserializeOwned + fmt::Debug>(field: &str) -> Vec<String> {
        let json = format!(r#"{{"{field}": "NoSuchVariant"}}"#);
        let error = serde_json::from_str::<T>(&json).unwrap_err().to_string();
        let list = error.split("expected one of").nth(1).expect(&error);
        let mut tags: Vec<String> = list
            .split('`')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect();
        tags.sort();
        tags
    }

    /// `name()` returns the variant identifier, so it matches the tag for every variant as long
    /// as serde uses the same set of names, i.e. no variant is renamed.
    #[test]
    fn every_name_is_a_tag() {
        let mut names: Vec<&str> = WsCommand::NAMES.to_vec();
        names.sort();
        assert_eq!(names, serde_tags::<WsCommand>("command"));
        let mut names: Vec<&str> = WsReply::NAMES.to_vec();
        names.sort();
        assert_eq!(names, serde_tags::<WsReply>("reply"));
    }
}
