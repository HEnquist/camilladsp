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
use std::num::NonZeroUsize;
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
    /// Capture device format changed: `samplerate`, `channels` and `format`, null if unknown.
    CaptureFormatChange(SourceFormat),
    /// Playback device format changed, reported the same way as `CaptureFormatChange`.
    PlaybackFormatChange(SourceFormat),
}

/// The format a source reported. Each field is given where the backend knows it, and is
/// `None` otherwise. Most backends know at least the rate, WASAPI only knows that something
/// changed.
///
/// The sample format is the backend's own name for it, as its configs and its capability
/// probing write it ("S24_3_LE" for ALSA, "S24" for CoreAudio). Backends differ in which
/// formats they have, so a name is what stays general across them.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct SourceFormat {
    /// Sample rate in Hz.
    pub samplerate: Option<usize>,
    /// Number of channels.
    pub channels: Option<usize>,
    /// Sample format, as the backend's configs name it.
    pub format: Option<String>,
}

impl SourceFormat {
    /// A format where only the rate is known.
    pub fn rate(samplerate: usize) -> Self {
        SourceFormat {
            samplerate: Some(samplerate),
            channels: None,
            format: None,
        }
    }

    /// A format change where nothing is known about the new format.
    pub fn unknown() -> Self {
        SourceFormat {
            samplerate: None,
            channels: None,
            format: None,
        }
    }

    /// Whether two reports describe the same format. Fields that only one of them
    /// knows are not compared.
    pub fn same_as(&self, other: &SourceFormat) -> bool {
        fn agree<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
        }
        agree(&self.samplerate, &other.samplerate)
            && agree(&self.channels, &other.channels)
            && agree(&self.format, &other.format)
    }

    /// The rate, if it is known.
    pub fn known_rate(&self) -> Option<NonZeroUsize> {
        self.samplerate.and_then(NonZeroUsize::new)
    }
}

impl fmt::Display for SourceFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.samplerate {
            Some(rate) => write!(f, "{rate} Hz")?,
            None => write!(f, "unknown rate")?,
        }
        if let Some(channels) = self.channels {
            write!(f, ", {channels} channels")?;
        }
        if let Some(format) = &self.format {
            write!(f, ", {format}")?;
        }
        Ok(())
    }
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

// ── Built-in controller ────────────────────────────────────────────────────

/// The `controller` section of the statefile.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ControllerSettings {
    /// Follow the capture source format, adapting the entry config to it.
    #[serde(default)]
    pub follow_capture: Option<bool>,
    /// Retry after a device error.
    #[serde(default)]
    pub error_recovery: Option<bool>,
}

impl ControllerSettings {
    /// Whether following is enabled.
    pub fn following_enabled(&self) -> bool {
        self.follow_capture.unwrap_or_default()
    }

    /// Whether error recovery is enabled.
    pub fn recovery_enabled(&self) -> bool {
        self.error_recovery.unwrap_or_default()
    }

    /// Whether any controller feature is enabled.
    pub fn any_enabled(&self) -> bool {
        self.following_enabled() || self.recovery_enabled()
    }
}

/// The entry config that the Adapt provider starts from, and what is wrong with it, if
/// anything. The format is the one the config is written for, after the command line
/// overrides. Adapt changes the rate and the sample format, but not the channel count.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AdaptCheck {
    /// Path of the entry config file, `null` for a config that was sent over the websocket.
    pub file: Option<String>,
    /// Capture sample rate, `null` when no config is loaded.
    pub samplerate: Option<usize>,
    /// The `alternate_samplerates` of the config, the rates besides its `samplerate` that run
    /// natively. A source at any other rate is resampled to `samplerate`.
    pub alternate_samplerates: Vec<usize>,
    /// Capture channels, `null` when no config is loaded.
    pub channels: Option<usize>,
    /// Capture sample format, `null` when it is automatic or no config is loaded.
    pub format: Option<String>,
    /// What is wrong with the config, `null` if it is usable. Also lists the settings that
    /// are only right at the config's own rate, when Adapt would change its samplerate.
    pub problem: Option<String>,
    /// The per-rate variants the config is missing for an alternate rate: a coefficient file
    /// or a filter, mixer or processor named with `$samplerate$`. The config still runs at
    /// the other rates, but a source at that rate gets no config.
    pub warnings: Vec<String>,
}

/// Value of the `CheckControllerFiles` reply.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ControllerFileChecks {
    /// The entry config Adapt starts from. `null` when following is off.
    pub adapt: Option<AdaptCheck>,
}

/// Where the config the controller runs comes from.
///
/// - `Files`: the controller loads it from the entry config file, through a provider.
/// - `Websocket`: it was sent over the websocket, and runs exactly as sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum ControllerSource {
    #[default]
    Files,
    Websocket,
}

/// The provider that supplied a config of the files source.
///
/// - `Single`: the entry config file as is.
/// - `Adapt`: the entry config file adapted to the source format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum ConfigProvider {
    Single,
    Adapt,
}

/// The state of the built-in controller, returned by the `GetControllerStatus` command.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ControllerStatusReport {
    /// Where the config comes from. It stays until the next switch, also while nothing runs.
    pub source: ControllerSource,
    /// The provider of the running config. `null` for the websocket source, and while
    /// nothing runs.
    pub provider: Option<ConfigProvider>,
    /// The entry config file that runs, `null` when `provider` is.
    pub config_file: Option<String>,
    /// The entry config file, the one `GetConfigFilePath` gives, and the one `Reload`
    /// loads.
    pub entry_config_file: Option<String>,
    /// Waiting to retry after a device error.
    pub recovering: bool,
    /// Retries since the last session that ran long enough.
    pub attempts: usize,
    /// Seconds until the next retry, while waiting for one.
    pub next_retry_s: Option<f32>,
    /// The source format no config was found for, while waiting for the source to change.
    pub waiting_for_source: Option<SourceFormat>,
    /// Whether following is on, enabled and in wait mode. `false` for the websocket source.
    pub following: bool,
    /// Whether error recovery is on, enabled and in wait mode.
    pub error_recovery: bool,
    /// Why processing last stopped.
    pub stop_reason: StopReason,
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    /// Maximum push rate in Hz. Leave out or set `null` for the natural rate (one push per 50 %
    /// overlap hop).
    pub max_rate: Option<f32>,
}

/// Parameters for a VU-meter subscription ([`WsCommand::SubscribeVuLevels`]).
///
/// Controls smoothing and rate-limiting of pushed level events.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

/// Defines the websocket protocol: [`WsCommand`], [`WsReply`] and the command groups.
///
/// Each command is written together with its reply, inside a group:
///
/// ```text
/// /// Group description, shown in websocket.md.
/// group "Group name" {
///     /// Command description.
///     CommandName { arguments } -> { reply fields },
/// }
/// ```
///
/// - `{ arguments }` are the named fields of the command. Leave it out for a command without
///   arguments.
/// - `-> { reply fields }` are the fields of the reply, normally just a documented `value`.
///   Leave it out for a reply that only reports the result. Every reply gets a flattened
///   `result: WsResult` field first, so it is never written here.
/// - `internal { ... }` holds commands that are not part of the protocol. They get no reply
///   and are left out of websocket.md.
/// - `other_replies { ... }` holds replies that do not answer a command, written out in full:
///   the pushed events and `Invalid`.
///
/// The groups and the commands within them are listed in the order they appear in
/// websocket.md. Doc comments on commands, arguments and reply fields become the descriptions
/// in websocket.md and in the OpenAPI schemas. rustfmt does not format the macro body, so keep
/// the layout of the existing entries.
macro_rules! ws_protocol {
    (
        $(
            $(#[doc = $gdoc:literal])*
            group $gname:literal {
                $(
                    $(#[$cmeta:meta])*
                    $cmd:ident
                    $({ $($args:tt)* })?
                    $(-> { $($rfields:tt)* })?
                ),* $(,)?
            }
        )*
        internal {
            $(
                $(#[$imeta:meta])*
                $iname:ident
            ),* $(,)?
        }
        other_replies {
            $(
                $(#[$ometa:meta])*
                $oname:ident { $($ofields:tt)* }
            ),* $(,)?
        }
    ) => {
        /// All commands accepted by the websocket server.
        ///
        /// Every command is a JSON object with a `"command"` field holding the command name.
        /// Commands with arguments carry them in additional named fields, e.g.
        /// `{"command": "SetUpdateInterval", "value": 500}`.
        ///
        /// A command with a field that it does not take is rejected, so that a misspelled field
        /// name gives an error instead of being ignored.
        ///
        /// See the [module-level documentation](self) for the general message format.
        // The commands without arguments are empty struct variants, since serde does not check
        // the fields of a unit variant.
        #[derive(Debug, PartialEq, Deserialize, Serialize)]
        #[serde(tag = "command", deny_unknown_fields)]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        pub enum WsCommand {
            $($(
                $(#[$cmeta])*
                $cmd { $($($args)*)? },
            )*)*
            $(
                $(#[$imeta])*
                $iname,
            )*
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
            $($(
                $cmd {
                    #[serde(flatten)]
                    result: WsResult,
                    $($($rfields)*)?
                },
            )*)*
            $(
                $(#[$ometa])*
                $oname { $($ofields)* },
            )*
        }

        impl WsCommand {
            /// The variant name, which is also the value of the `"command"` field in the JSON
            /// message.
            pub fn name(&self) -> &'static str {
                match self {
                    $($(Self::$cmd { .. } => stringify!($cmd),)*)*
                    $(Self::$iname { .. } => stringify!($iname),)*
                }
            }

            #[cfg(test)]
            const NAMES: &[&str] = &[
                $($(stringify!($cmd),)*)*
                $(stringify!($iname),)*
            ];
        }

        impl WsReply {
            /// The variant name, which is also the value of the `"reply"` field in the JSON
            /// message.
            pub fn name(&self) -> &'static str {
                match self {
                    $($(Self::$cmd { .. } => stringify!($cmd),)*)*
                    $(Self::$oname { .. } => stringify!($oname),)*
                }
            }

            #[cfg(test)]
            const NAMES: &[&str] = &[
                $($(stringify!($cmd),)*)*
                $(stringify!($oname),)*
            ];
        }

        /// The command groups in the order of websocket.md, as
        /// `(group name, group description, command names)`.
        pub const WS_COMMAND_GROUPS: &[(&str, &str, &[&str])] = &[
            $((
                $gname,
                concat!($($gdoc, "\n"),*),
                &[$(stringify!($cmd)),*],
            ),)*
        ];
    };
}

ws_protocol! {
    /// Basic commands for querying the CamillaDSP version and the list of supported
    /// device types, and for stopping or exiting the process.
    group "General" {
        /// Get the CamillaDSP version string.
        GetVersion -> {
            /// Version string, e.g. `"2.0.0"`.
            value: String,
        },

        /// Get the list of supported playback and capture device types.
        GetSupportedDeviceTypes -> {
            /// `[list_of_playback_types, list_of_capture_types]`.
            value: (Vec<String>, Vec<String>),
        },

        /// Stop processing and wait for a new configuration to be uploaded
        /// via [`SetConfig`](Self::SetConfig) or [`SetConfigFilePath`](Self::SetConfigFilePath) +
        /// [`Reload`](Self::Reload).
        Stop,

        /// Stop processing and exit CamillaDSP.
        Exit,
    }

    /// Controls the polling interval used for background status sampling.
    /// The update interval determines how often the capture rate and signal range
    /// snapshots are refreshed internally.
    group "Websocket server settings" {
        /// Get the update interval for capture rate and signal range polling.
        GetUpdateInterval -> {
            /// Update interval in milliseconds.
            value: usize,
        },

        /// Set the update interval for capture rate and signal range polling.
        SetUpdateInterval {
            /// Interval in milliseconds.
            value: usize,
        },
    }

    /// Commands for monitoring the running pipeline: processing state, capture rate,
    /// signal range, async-resampler adjustment, playback buffer level, clipped
    /// samples, and CPU load. Also covers the optional state file and
    /// a push subscription for state-change events.
    group "Processing status" {
        /// Get the current processing state.
        GetState -> {
            /// Current processing state.
            value: ProcessingState,
        },

        /// Get the reason processing last stopped.
        GetStopReason -> {
            /// Reason the processing last stopped.
            value: StopReason,
        },

        /// Subscribe to pushed processing state change events.
        ///
        /// While subscribed, CamillaDSP sends a [`WsReply::StateEvent`] message whenever the
        /// processing state changes. The event payload always contains `state`. When the state is
        /// `"Inactive"` it also contains `stop_reason`.
        ///
        /// Send [`StopSubscription`](Self::StopSubscription) to end the stream.
        /// No other commands are accepted on the connection until then.
        SubscribeState,

        /// Stop an active subscription (signal levels, VU levels, state, or spectrum).
        ///
        /// If no subscription is active, the reply is [`WsReply::Invalid`]
        /// with the error `"No active subscription"`.
        StopSubscription,

        /// Get the measured sample rate of the capture device.
        GetCaptureRate -> {
            /// Measured capture sample rate in Hz.
            value: usize,
        },

        /// Get the peak-to-peak signal range of the last processed chunk.
        ///
        /// A value of 2.0 means full level (signal swings from −1.0 to +1.0).
        GetSignalRange -> {
            /// Peak-to-peak amplitude range of the last chunk (2.0 = full level).
            value: f32,
        },

        /// Get the current adjustment factor applied to the asynchronous resampler.
        GetRateAdjust -> {
            /// Rate adjustment factor applied to the async resampler (1.0 = no adjustment).
            value: f32,
        },

        /// Get the current playback device buffer level when rate adjust is enabled.
        GetBufferLevel -> {
            /// Playback device buffer fill level in frames; 0 if rate adjust is not enabled.
            value: usize,
        },

        /// Get the number of samples that have been clipped since start, or since the
        /// counter was last reset. Loading a new config does not reset it.
        GetClippedSamples -> {
            /// Number of clipped samples since start or the last reset.
            value: usize,
        },

        /// Reset the clipped-samples counter to zero.
        ResetClippedSamples,

        /// Get the current pipeline processing load.
        GetProcessingLoad -> {
            /// Pipeline processing load in percent.
            value: f32,
        },

        /// Get the current resampler processing load.
        GetResamplerLoad -> {
            /// Resampler processing load in percent.
            value: f32,
        },

        /// Get the path of the state file, if one is configured.
        GetStateFilePath -> {
            /// File path of the state file, or `null` if no state file is used.
            value: Option<String>,
        },

        /// Check whether all pending changes have been saved to the state file.
        GetStateFileUpdated -> {
            /// `true` if all changes have been saved to the state file.
            value: bool,
        },
    }

    /// Read RMS and peak levels for capture and playback channels.
    /// Levels are available as an instantaneous snapshot, averaged over a time window,
    /// accumulated since the last call, or pushed continuously via a subscription.
    /// Also includes peak-since-start tracking, per-channel display labels,
    /// and a smoothed VU-meter subscription.
    group "Signal levels" {
        /// Get the RMS level of the last chunk on the capture side, per channel.
        GetCaptureSignalRms -> {
            /// RMS level per capture channel in dB (0 dB = full level).
            value: Vec<f32>,
        },

        /// Get the RMS level averaged over the last `value` seconds on the capture side, per channel.
        GetCaptureSignalRmsSince {
            /// Time window in seconds.
            value: f32,
        } -> {
            /// RMS level per capture channel in dB, averaged over the requested window.
            value: Vec<f32>,
        },

        /// Get the RMS level since the last call to this command from this client, per channel.
        ///
        /// On the first call, returns values since the client connected.
        /// If called again before new data is available, returns an empty list.
        GetCaptureSignalRmsSinceLast -> {
            /// RMS level per capture channel in dB since the last call; empty if no new data.
            value: Vec<f32>,
        },

        /// Get the peak level of the last chunk on the capture side, per channel.
        GetCaptureSignalPeak -> {
            /// Peak level per capture channel in dB (0 dB = full level).
            value: Vec<f32>,
        },

        /// Get the peak level over the last `value` seconds on the capture side, per channel.
        GetCaptureSignalPeakSince {
            /// Time window in seconds.
            value: f32,
        } -> {
            /// Peak level per capture channel in dB over the requested window.
            value: Vec<f32>,
        },

        /// Get the peak level since the last call to this command from this client, per channel.
        GetCaptureSignalPeakSinceLast -> {
            /// Peak level per capture channel in dB since the last call; empty if no new data.
            value: Vec<f32>,
        },

        /// Get the RMS level of the last chunk on the playback side, per channel.
        GetPlaybackSignalRms -> {
            /// RMS level per playback channel in dB (0 dB = full level).
            value: Vec<f32>,
        },

        /// Get the RMS level averaged over the last `value` seconds on the playback side, per channel.
        GetPlaybackSignalRmsSince {
            /// Time window in seconds.
            value: f32,
        } -> {
            /// RMS level per playback channel in dB, averaged over the requested window.
            value: Vec<f32>,
        },

        /// Get the RMS level since the last call to this command from this client, per channel.
        GetPlaybackSignalRmsSinceLast -> {
            /// RMS level per playback channel in dB since the last call; empty if no new data.
            value: Vec<f32>,
        },

        /// Get the peak level of the last chunk on the playback side, per channel.
        GetPlaybackSignalPeak -> {
            /// Peak level per playback channel in dB (0 dB = full level).
            value: Vec<f32>,
        },

        /// Get the peak level over the last `value` seconds on the playback side, per channel.
        GetPlaybackSignalPeakSince {
            /// Time window in seconds.
            value: f32,
        } -> {
            /// Peak level per playback channel in dB over the requested window.
            value: Vec<f32>,
        },

        /// Get the peak level since the last call to this command from this client, per channel.
        GetPlaybackSignalPeakSinceLast -> {
            /// Peak level per playback channel in dB since the last call; empty if no new data.
            value: Vec<f32>,
        },

        /// Get RMS and peak levels for both sides in a single request.
        GetSignalLevels -> {
            /// RMS and peak levels for both sides.
            value: AllLevels,
        },

        /// Get RMS and peak levels over the last `value` seconds for both sides.
        GetSignalLevelsSince {
            /// Time window in seconds.
            value: f32,
        } -> {
            /// RMS and peak levels for both sides, averaged over the requested window.
            value: AllLevels,
        },

        /// Get RMS and peak levels since the last call to this command from this client, for both sides.
        GetSignalLevelsSinceLast -> {
            /// RMS and peak levels for both sides since the last call; empty if no new data.
            value: AllLevels,
        },

        /// Subscribe to pushed signal level events.
        ///
        /// While subscribed, CamillaDSP sends a [`WsReply::SignalLevelsEvent`] message each time a
        /// new chunk is analyzed. The event rate therefore depends on the configured chunk size and
        /// sample rate. Send [`StopSubscription`](Self::StopSubscription) to end the stream.
        /// No other commands are accepted on the connection until then.
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
        /// No other commands are accepted on the connection until then.
        SubscribeVuLevels {
            /// Rate limit and smoothing settings.
            value: VuSubscription,
        },

        /// Get the peak capture and playback levels measured since processing started.
        GetSignalPeaksSinceStart -> {
            /// Peak levels since processing started, for both sides.
            value: PbCapLevels,
        },

        /// Reset the peak-since-start counters. Affects all connected clients.
        ResetSignalPeaksSinceStart,

        /// Get the optional display labels for capture and playback channels.
        GetChannelLabels -> {
            /// Display labels for capture and playback channels.
            value: ChannelLabels,
        },
    }

    /// Compute an FFT-based frequency spectrum from the audio passing through
    /// the pipeline. Available as a one-shot request or a continuous push
    /// subscription.
    group "Spectrum analysis" {
        /// Compute a one-shot frequency spectrum from the audio currently passing through the pipeline.
        GetSpectrum {
            /// The spectrum to compute.
            value: SpectrumRequest,
        } -> {
            /// Computed spectrum with frequency and magnitude arrays.
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<SpectrumData>,
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
        /// No other commands are accepted on the connection until then.
        SubscribeSpectrum {
            /// The spectrum to compute, and the push rate.
            value: SpectrumSubscription,
        },
    }

    /// Read and adjust the volume and mute state of the faders.
    /// The Main fader (index 0) maps to fader index 0; Aux1–Aux4 map to indices 1–4.
    /// Volume adjustments can optionally specify a clamping range so that the
    /// result stays within caller-defined limits.
    group "Volume control" {
        /// Get the current volume of the Main fader.
        GetVolume -> {
            /// Current volume in dB.
            value: f32,
        },

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
        } -> {
            /// New volume in dB after the adjustment.
            value: f32,
        },

        /// Get the mute state of the Main fader.
        GetMute -> {
            /// `true` if muted.
            value: bool,
        },

        /// Set the mute state of the Main fader.
        SetMute {
            /// `true` to mute, `false` to unmute.
            value: bool,
        },

        /// Toggle the mute state of the Main fader.
        ToggleMute -> {
            /// New mute state after the toggle.
            value: bool,
        },

        /// Get the volume and mute state of all faders in a single request.
        GetFaders -> {
            /// List of faders: Main (index 0) followed by Aux1–Aux4 (indices 1–4).
            value: Vec<Fader>,
        },

        /// Get the volume of a specific fader.
        GetFaderVolume {
            /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
            fader: usize,
        } -> {
            /// `[fader_index, volume_dB]`.
            value: (usize, f32),
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
        } -> {
            /// `[fader_index, new_volume_dB]` after the adjustment.
            value: (usize, f32),
        },

        /// Get the mute state of a specific fader.
        GetFaderMute {
            /// Fader index, 0 for Main and 1–4 for Aux1–Aux4.
            fader: usize,
        } -> {
            /// `[fader_index, is_muted]`.
            value: (usize, bool),
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
        } -> {
            /// `[fader_index, new_mute_state]` after the toggle.
            value: (usize, bool),
        },
    }

    /// Read and modify the active configuration.
    /// Changes applied via `SetConfig`, `SetConfigJson`, or `PatchConfig` take
    /// effect immediately. Changes via `SetConfigFilePath` require a subsequent
    /// `Reload` to be applied.
    /// While the built-in controller follows the capture source, `GetConfig` returns
    /// the config that runs, which is the loaded config adapted to the source format.
    /// `SetConfig`, `SetConfigJson`, `PatchConfig` and `SetConfigValue` switch the
    /// controller to the websocket source, where the config runs exactly as sent and
    /// nothing is followed, until a config is loaded from a file again with `Reload`.
    /// `PatchConfig` and `SetConfigValue` patch the config that runs, the one
    /// `GetConfig` returns.
    group "Config management" {
        /// Read the active configuration.
        GetConfig -> {
            /// Active config in YAML format.
            value: String,
        },

        /// Read the active configuration as JSON.
        GetConfigJson -> {
            /// Active config in JSON format.
            value: String,
        },

        /// Read the `title` field from the active configuration.
        GetConfigTitle -> {
            /// Title string from the active config.
            value: String,
        },

        /// Read the `description` field from the active configuration.
        GetConfigDescription -> {
            /// Description string from the active config.
            value: String,
        },

        /// Get the path of the currently loaded config file.
        GetConfigFilePath -> {
            /// File path of the active config, or `null` if no file is loaded.
            value: Option<String>,
        },

        /// Read the previously active configuration (before the last reload or upload).
        GetPreviousConfig -> {
            /// Previously active config in YAML format.
            value: String,
        },

        /// Change the active config file path. Not applied until [`Reload`](Self::Reload) is called.
        SetConfigFilePath {
            /// Path of the config file.
            value: String,
        },

        /// Upload and immediately apply a new configuration as a YAML string.
        /// Switches the built-in controller to the websocket source, see the group
        /// description.
        SetConfig {
            /// Config in YAML format.
            value: String,
        },

        /// Upload and immediately apply a new configuration as a JSON string.
        /// Switches the built-in controller to the websocket source, see the group
        /// description.
        SetConfigJson {
            /// Config in JSON format.
            value: String,
        },

        /// Apply a partial patch to the active configuration.
        ///
        /// If the resulting config is valid it is applied immediately, and the built-in
        /// controller switches to the websocket source, see the group description.
        PatchConfig {
            /// Partial config object containing only the fields to change.
            value: serde_json::Value,
        },

        /// Read a single value from the active configuration using a JSON Pointer (RFC 6901).
        GetConfigValue {
            /// JSON Pointer to the value, such as `"/devices/samplerate"`.
            value: String,
        } -> {
            /// Value at the specified JSON Pointer path.
            value: serde_json::Value,
        },

        /// Set a single value in the active configuration using a JSON Pointer (RFC 6901).
        /// Switches the built-in controller to the websocket source, like `PatchConfig`.
        SetConfigValue {
            /// JSON Pointer to the value, such as `"/devices/samplerate"`.
            pointer: String,
            /// The value to store there.
            value: serde_json::Value,
        },

        /// Reload the current config file from disk. Equivalent to sending `SIGHUP`.
        /// Switches the built-in controller back to the files source, and following
        /// resumes if it is enabled.
        Reload,
    }

    /// The built-in controller follows the format of the capture source and recovers
    /// from device errors. Its settings are the `controller` section of the statefile,
    /// and both features only work in wait mode.
    ///
    /// The config comes from one of two sources. From the files source, the controller
    /// loads the entry config file through a provider: `Single` runs it as is, and `Adapt`
    /// adapts it to the source format.
    /// A config sent with `SetConfig` or `SetConfigJson`, or patched with `PatchConfig`
    /// or `SetConfigValue`, switches to the websocket source. It then runs exactly as
    /// sent: it is not adapted, and a source format change
    /// stops processing as it does without following. Error recovery works the same for
    /// both sources, it restarts the config that ran. A config loaded from a file switches
    /// back to the files source, with `Reload`, `SetConfigFilePath` followed by `Reload`,
    /// `SIGHUP`, or a restart. Following then resumes from that file.
    group "Controller" {
        /// Get the settings of the built-in controller, the `controller` section of the
        /// statefile.
        GetControllerSettings -> {
            /// The `controller` section, or `null` if there is none.
            value: Option<ControllerSettings>,
        },

        /// Replace the settings of the built-in controller. They take effect at the next
        /// session start, so send a [`Reload`](Self::Reload) to apply them at once.
        /// This doesn't switch back to the files source, the `Reload` does.
        SetControllerSettings {
            /// The whole `controller` section, or `null` to remove it.
            value: Option<ControllerSettings>,
        },

        /// Get the state of the built-in controller: the source, the provider and file of
        /// the running config, error recovery, and whether it is waiting for the source to
        /// change format.
        GetControllerStatus -> {
            /// The controller state.
            value: ControllerStatusReport,
        },

        /// Check the entry config that Adapt starts from. Loads the config and its
        /// coefficient files, so it is meant to be run on demand rather than polled.
        CheckControllerFiles -> {
            /// The check, with `adapt` `null` when following is off.
            value: ControllerFileChecks,
        },
    }

    /// Parse and validate a configuration string or file without affecting the
    /// running pipeline. Useful for checking a config before applying it.
    /// The `Validate` variants perform more thorough cross-field checks than
    /// the `Read` variants.
    group "Config reading and checking" {
        /// Parse and fill defaults for a YAML config string without changing the active config.
        ReadConfig {
            /// Config in YAML format.
            value: String,
        } -> {
            /// Config with all optional fields filled with defaults, or an error message.
            value: String,
        },

        /// Parse and fill defaults for a JSON config string without changing the active config.
        ReadConfigJson {
            /// Config in JSON format.
            value: String,
        } -> {
            /// Config with all optional fields filled with defaults, or an error message.
            value: String,
        },

        /// Parse and fill defaults for a config file without changing the active config.
        ReadConfigFile {
            /// Path of the config file.
            value: String,
        } -> {
            /// Config with all optional fields filled with defaults, or an error message.
            value: String,
        },

        /// Like [`ReadConfig`](Self::ReadConfig) but performs more extensive validation checks.
        ValidateConfig {
            /// Config in YAML format.
            value: String,
        } -> {
            /// Validated config with defaults, or an error message.
            value: String,
        },

        /// Like [`ReadConfigJson`](Self::ReadConfigJson) but performs more extensive validation checks.
        ValidateConfigJson {
            /// Config in JSON format.
            value: String,
        } -> {
            /// Validated config with defaults, or an error message.
            value: String,
        },
    }

    /// Enumerate available audio devices for a given backend and query their
    /// supported sample rates, formats, and channel counts.
    /// See the section below for the response format.
    group "Audio device listing" {
        /// List available capture devices for a given backend.
        GetAvailableCaptureDevices {
            /// Backend name, one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
            backend: String,
        } -> {
            /// List of `[identifier, name]` pairs. Some backends use the identifier as the name.
            value: Vec<(String, String)>,
        },

        /// List available playback devices for a given backend.
        GetAvailablePlaybackDevices {
            /// Backend name, one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.
            backend: String,
        } -> {
            /// List of `[identifier, name]` pairs. Some backends use the identifier as the name.
            value: Vec<(String, String)>,
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
        } -> {
            /// Capabilities of the requested capture device.
            value: AudioDeviceDescriptor,
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
        } -> {
            /// Capabilities of the requested playback device.
            value: AudioDeviceDescriptor,
        },
    }

    internal {
        /// Internal sentinel, used for messages that are not text. Never accepted from clients.
        #[serde(skip_deserializing)]
        None,
    }

    other_replies {
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

        /// Pushed to subscribed clients each time a new spectrum is ready.
        SpectrumEvent {
            #[serde(flatten)]
            result: WsResult,
            /// Computed spectrum, or absent if processing has stopped.
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<SpectrumData>,
        },

        /// Sent when the server cannot parse or dispatch the incoming command.
        Invalid { error: String },
    }
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
        round_trip_command(WsCommand::GetVersion {});
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
            WsCommand::GetVersion {},
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
    fn internal_command_is_not_accepted_from_clients() {
        assert!(serde_json::from_str::<WsCommand>(r#"{"command": "None"}"#).is_err());
    }

    #[test]
    fn unknown_command_fields_are_rejected() {
        for json in [
            r#"{"command": "GetVersion", "value": 1}"#,
            r#"{"command": "SetUpdateInterval", "valeu": 500}"#,
            r#"{"command": "SetUpdateInterval", "value": 500, "extra": 1}"#,
            r#"{"command": "AdjustVolume", "value": -3.0, "minimum": -50.0}"#,
            r#"{"command": "SubscribeVuLevels",
                "value": {"max_rate": 10.0, "attack": 0.0, "relase": 0.0}}"#,
            r#"{"command": "SubscribeVuLevels",
                "value": {"max_rate": 10.0, "attack": 0.0, "release": 0.0, "extra": 1}}"#,
            r#"{"command": "GetSpectrum", "value": {"side": "capture", "channel": null,
                "min_freq": 20.0, "max_freq": 20000.0, "n_bins": 10, "extra": 1}}"#,
            r#"{"command": "SubscribeSpectrum", "value": {"side": "capture", "channel": null,
                "min_freq": 20.0, "max_freq": 20000.0, "n_binz": 10}}"#,
        ] {
            assert!(serde_json::from_str::<WsCommand>(json).is_err(), "{json}");
        }
        for json in [
            r#"{"command": "GetVersion"}"#,
            r#"{"command": "AdjustVolume", "value": -3.0, "min": -50.0}"#,
            r#"{"command": "SubscribeVuLevels",
                "value": {"max_rate": 10.0, "attack": 0.0, "release": 0.0}}"#,
            r#"{"command": "SubscribeSpectrum", "value": {"side": "capture", "channel": null,
                "min_freq": 20.0, "max_freq": 20000.0, "n_bins": 10}}"#,
        ] {
            assert!(serde_json::from_str::<WsCommand>(json).is_ok(), "{json}");
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
        // The internal sentinel is never deserialized, so serde does not list it.
        let mut names: Vec<&str> = WsCommand::NAMES
            .iter()
            .copied()
            .filter(|n| *n != "None")
            .collect();
        names.sort();
        assert_eq!(names, serde_tags::<WsCommand>("command"));
        let mut names: Vec<&str> = WsReply::NAMES.to_vec();
        names.sort();
        assert_eq!(names, serde_tags::<WsReply>("reply"));
    }
}
