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

#![doc = include_str!("../README.crates.md")]
//!
//! # API cross-references
//!
//! Key modules: [`filters`], [`mixer`], [`processors`], [`pipeline`],
//! [`config`], [`engine`], [`processing`].
//!
//! Key types in this crate root: [`StatusMessage`], [`CommandMessage`],
//! [`CaptureStatus`], [`PlaybackStatus`], [`PrcFmt`].

#[cfg(target_os = "linux")]
extern crate alsa;
#[cfg(target_os = "linux")]
extern crate alsa_sys;
extern crate clap;
#[cfg(target_os = "macos")]
extern crate coreaudio;
#[cfg(feature = "cpal-backend")]
extern crate cpal;
extern crate crossbeam_channel;
#[cfg(target_os = "macos")]
extern crate dispatch;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_binding as pulse;
#[cfg(feature = "pulse-backend")]
extern crate libpulse_simple_binding as psimple;
#[cfg(feature = "secure-websocket")]
extern crate native_tls;
#[cfg(target_os = "linux")]
extern crate nix;
extern crate num_complex;
extern crate num_traits;
#[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
extern crate pipewire;
extern crate rand;
extern crate rand_distr;
extern crate realfft;
extern crate rubato;
extern crate serde;
extern crate serde_with;
extern crate signal_hook;
#[cfg(feature = "websocket")]
extern crate tungstenite;
//#[cfg(target_os = "windows")]
//extern crate winapi;

#[macro_use]
extern crate log;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::error;
use std::fmt;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};
use std::time::Instant;

/// Global flag set to `true` when a graceful shutdown has been requested (e.g. by SIGTERM).
pub static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

// Logging macros to give extra logs
// when the "debug" feature is enabled.
#[allow(unused)]
macro_rules! xtrace { ($($x:tt)*) => (
    #[cfg(feature = "debug")] {
        log::trace!($($x)*)
    }
) }
#[allow(unused)]
macro_rules! xdebug { ($($x:tt)*) => (
    #[cfg(feature = "debug")] {
        log::debug!($($x)*)
    }
) }
#[allow(unused)]
macro_rules! xinfo { ($($x:tt)*) => (
    #[cfg(feature = "debug")] {
        log::info!($($x)*)
    }
) }
#[allow(unused)]
macro_rules! xwarn { ($($x:tt)*) => (
    #[cfg(feature = "debug")] {
        log::warn!($($x)*)
    }
) }
#[allow(unused)]
macro_rules! xerror { ($($x:tt)*) => (
    #[cfg(feature = "debug")] {
        log::error!($($x)*)
    }
) }

/// Internal floating-point sample type: `f32` with the `32bit` feature, `f64` otherwise.
#[cfg(feature = "32bit")]
pub type PrcFmt = f32;
/// Internal floating-point sample type: `f32` with the `32bit` feature, `f64` otherwise.
#[cfg(not(feature = "32bit"))]
pub type PrcFmt = f64;

/// Helper trait for lossless type coercion used internally when converting between `f32` and `f64`.
pub trait NewValue<T> {
    fn coerce(val: T) -> Self;
}

impl<PrcFmt> NewValue<PrcFmt> for PrcFmt {
    fn coerce(val: PrcFmt) -> PrcFmt {
        val
    }
}

/// Convenience `Result` type used throughout CamillaDSP.
pub type Res<T> = Result<T, Box<dyn error::Error>>;

/// ALSA audio backend (Linux only).
#[cfg(target_os = "linux")]
pub mod alsa_backend;
/// ASIO audio backend (Windows only, requires `asio-backend` feature).
#[cfg(all(target_os = "windows", feature = "asio-backend"))]
pub mod asio_backend;
/// Audio chunk types and per-chunk statistics.
pub mod audiochunk;
/// Audio device abstraction and cross-backend message types.
pub mod audiodevice;
/// Configuration parsing, validation, and type definitions.
pub mod config;
/// CoreAudio backend (macOS only).
#[cfg(target_os = "macos")]
pub mod coreaudio_backend;
/// CPAL/JACK audio backend (requires `cpal-backend` feature).
#[cfg(feature = "cpal-backend")]
pub mod cpal_backend;
/// Top-level engine: device startup, supervisor loop, and restart logic.
pub mod engine;
/// File, stdin/stdout, and WAV audio backends.
pub mod file_backend;
/// Audio filter implementations and the [`filters::Filter`] trait.
pub mod filters;
/// Signal-generator capture device.
pub mod generatordevice;
/// Mixer: channel routing and gain with the [`mixer::Mixer`] runtime type.
pub mod mixer;
/// Processing pipeline: ordered mixer, filter, and processor steps.
pub mod pipeline;
/// PipeWire audio backend (Linux only, requires `pipewire-backend` feature).
#[cfg(all(target_os = "linux", feature = "pipewire-backend"))]
pub mod pipewire_backend;
/// Processing thread: pipeline loop and config-update handling.
pub mod processing;
/// Audio processor implementations and the [`processors::Processor`] trait.
pub mod processors;
/// PulseAudio backend (Linux only, requires `pulse-backend` feature).
#[cfg(all(target_os = "linux", feature = "pulse-backend"))]
pub mod pulse_backend;
/// Signal-level event notifications for WebSocket subscribers.
pub mod signal_monitor;
/// FFT-based spectrum analysis and the audio ring buffer.
pub mod spectrum;
/// Persistent state file (volume, mute, config path).
pub mod statefile;
/// Shared utilities: resampling, conversions, timing, dB helpers, and buffer stash.
pub mod utils;
/// WASAPI audio backend (Windows only).
#[cfg(target_os = "windows")]
pub mod wasapi_backend;
/// WebSocket control server (requires `websocket` feature).
#[cfg(feature = "websocket")]
pub mod websocket_server;

/// Messages sent from audio device threads to the processing supervisor.
pub enum StatusMessage {
    /// Playback device is open and ready.
    PlaybackReady,
    /// Capture device is open and ready.
    CaptureReady,
    /// Playback device encountered an unrecoverable error.
    PlaybackError(String),
    /// Capture device encountered an unrecoverable error.
    CaptureError(String),
    /// Playback device detected a sample-rate change to the given value.
    PlaybackFormatChange(usize),
    /// Capture device detected a sample-rate change to the given value.
    CaptureFormatChange(usize),
    /// Playback device thread has finished normally.
    PlaybackDone,
    /// Capture device thread has finished normally.
    CaptureDone,
    /// Request to change the resampling speed ratio (async resampling).
    SetSpeed(f64),
    /// Request to set the master volume (dB).
    SetVolume(f32),
    /// Request to set the master mute state.
    SetMute(bool),
}

/// Commands sent from the supervisor to an audio device thread.
pub enum CommandMessage {
    /// Change the resampling speed ratio (async resampling only).
    SetSpeed { speed: f64 },
    /// Tell the device thread to stop gracefully.
    Exit,
}

/// Outcome returned by the engine after the processing loop ends.
#[derive(Debug)]
pub enum ExitState {
    /// The engine should restart with a (potentially new) configuration.
    Restart,
    /// The engine should shut down completely.
    Exit,
}

/// Messages sent to the engine controller (WebSocket server or external caller).
pub enum ControllerMessage {
    /// A new configuration has been loaded and should replace the active one.
    // Config must be boxed, to prevent "large size difference between variants" warning
    ConfigChanged(Box<config::Configuration>),
    /// Stop processing but remain ready for a new configuration.
    Stop,
    /// Shut down the engine entirely.
    Exit,
}

#[derive(Clone, Debug, Copy, Serialize, Eq, PartialEq)]
pub enum ProcessingState {
    /// Processing is running normally.
    Running,
    /// Processing is paused because the input signal is silent.
    Paused,
    /// Processing is off and devices are closed, waiting for a new configuration.
    Inactive,
    /// Opening devices and starting up processing with a new configuration.
    Starting,
    /// Capture device is not providing data; processing is stalled.
    Stalled,
}

/// Live status of the capture device, updated each processing chunk.
#[derive(Clone, Debug)]
pub struct CaptureStatus {
    /// How often (in milliseconds) the WebSocket server pushes status updates.
    pub update_interval: usize,
    /// Most recently measured capture sample rate in Hz.
    pub measured_samplerate: usize,
    /// Peak amplitude of the most recent capture chunk (linear, 0..1).
    pub signal_range: f32,
    /// Rolling history of per-channel RMS levels (squared values).
    pub signal_rms: utils::countertimer::ValueHistory,
    /// Rolling history of per-channel peak levels.
    pub signal_peak: utils::countertimer::ValueHistory,
    /// Current processing state (running, paused, stalled, …).
    pub state: ProcessingState,
    /// Current sample-rate adjustment ratio applied by the async resampler.
    pub rate_adjust: f32,
    /// Which input channels are active (non-empty waveform).
    pub used_channels: Vec<bool>,
    /// Ring buffer holding recent capture audio for spectrum analysis.
    pub audio_buffer: spectrum::AudioRingBuffer,
}

/// Live status of the playback device, updated each processing chunk.
#[derive(Clone, Debug)]
pub struct PlaybackStatus {
    /// How often (in milliseconds) the WebSocket server pushes status updates.
    pub update_interval: usize,
    /// Cumulative number of clipped samples since the last config load.
    pub clipped_samples: usize,
    /// Current playback device buffer fill level in frames.
    pub buffer_level: usize,
    /// Rolling history of per-channel RMS levels (squared values).
    pub signal_rms: utils::countertimer::ValueHistory,
    /// Rolling history of per-channel peak levels.
    pub signal_peak: utils::countertimer::ValueHistory,
    /// Ring buffer holding recent playback audio for spectrum analysis.
    pub audio_buffer: spectrum::AudioRingBuffer,
}

pub(crate) fn update_capture_signal_status(
    capture_status: &Arc<RwLock<CaptureStatus>>,
    chunk_stats: &audiochunk::ChunkStats,
    rms_values: &mut Vec<f32>,
    peak_values: &mut Vec<f32>,
) {
    chunk_stats.rms_linear(rms_values);
    chunk_stats.peak_linear(peak_values);
    if let Some(mut capture_status) = capture_status.try_write() {
        capture_status.signal_rms.add_record_squared(rms_values);
        capture_status.signal_peak.add_record(peak_values);
        signal_monitor::mark_capture_updated();
    } else {
        xtrace!("capture status blocked, skip update");
    }
}

pub(crate) fn push_capture_audio_buffer(
    capture_status: &Arc<RwLock<CaptureStatus>>,
    chunk: &audiochunk::AudioChunk,
) {
    if let Some(mut status) = capture_status.try_write() {
        status.audio_buffer.push_chunk(chunk);
    }
}

pub(crate) fn push_playback_audio_buffer(
    playback_status: &Arc<RwLock<PlaybackStatus>>,
    chunk: &audiochunk::AudioChunk,
) {
    if let Some(mut status) = playback_status.try_write() {
        status.audio_buffer.push_chunk(chunk);
    }
}

/// Update `capture_status.state` and notify signal monitors if the state changed.
pub fn update_capture_state(capture_status: &mut CaptureStatus, state: ProcessingState) {
    if capture_status.state != state {
        capture_status.state = state;
        signal_monitor::mark_state_updated();
    }
}

/// Acquire the write lock on `capture_status` and call [`update_capture_state`].
pub fn set_capture_state(capture_status: &Arc<RwLock<CaptureStatus>>, state: ProcessingState) {
    let mut capture_status = capture_status.write();
    update_capture_state(&mut capture_status, state);
}

/// Update `processing_status.stop_reason` if it has changed.
pub fn update_stop_reason(processing_status: &mut ProcessingStatus, stop_reason: StopReason) {
    if processing_status.stop_reason != stop_reason {
        processing_status.stop_reason = stop_reason;
    }
}

/// Acquire the write lock on `processing_status` and call [`update_stop_reason`].
pub fn set_stop_reason(processing_status: &Arc<RwLock<ProcessingStatus>>, stop_reason: StopReason) {
    let mut processing_status = processing_status.write();
    update_stop_reason(&mut processing_status, stop_reason);
}

pub(crate) fn update_playback_signal_status(
    playback_status: &Arc<RwLock<PlaybackStatus>>,
    chunk_stats: &audiochunk::ChunkStats,
    rms_values: &mut Vec<f32>,
    peak_values: &mut Vec<f32>,
    clipped_samples: usize,
) {
    chunk_stats.rms_linear(rms_values);
    chunk_stats.peak_linear(peak_values);
    if let Some(mut playback_status) = playback_status.try_write() {
        if clipped_samples > 0 {
            playback_status.clipped_samples += clipped_samples;
        }
        playback_status.signal_rms.add_record_squared(rms_values);
        playback_status.signal_peak.add_record(peak_values);
        signal_monitor::mark_playback_updated();
    } else {
        xtrace!("playback status blocked, skip update");
    }
}

static PARAMS_EPOCH: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds elapsed since the first call to this function (monotonic, process-local epoch).
pub fn nanos_since_epoch() -> u64 {
    PARAMS_EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// Lock-free shared state for volume, mute, and load metrics, accessible from any thread.
#[derive(Debug)]
pub struct ProcessingParameters {
    // Optimization: volumes are actually `f32`s, but by representing their
    // bits as a `u32` of equal size we can use atomic operations instead of a
    // mutex.
    target_volume: [AtomicU32; Self::NUM_FADERS],
    target_volume_set_at: [AtomicU64; Self::NUM_FADERS],
    current_volume: [AtomicU32; Self::NUM_FADERS],
    mute: [AtomicBool; Self::NUM_FADERS],
    processing_load: AtomicU32,
    resampler_load: AtomicU32,
}

impl ProcessingParameters {
    /// Number of independent volume faders.
    pub const NUM_FADERS: usize = 5;

    /// Default volume level in dB (0 dB = unity gain).
    pub const DEFAULT_VOLUME: f32 = 0.0;
    /// Default mute state (`false` = unmuted).
    pub const DEFAULT_MUTE: bool = false;

    /// Create a new instance with the given initial volumes (dB) and mute states.
    pub fn new(initial_volumes: &[f32; 5], initial_mutes: &[bool; 5]) -> Self {
        Self {
            target_volume: [
                AtomicU32::new(initial_volumes[0].to_bits()),
                AtomicU32::new(initial_volumes[1].to_bits()),
                AtomicU32::new(initial_volumes[2].to_bits()),
                AtomicU32::new(initial_volumes[3].to_bits()),
                AtomicU32::new(initial_volumes[4].to_bits()),
            ],
            target_volume_set_at: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            current_volume: [
                AtomicU32::new(initial_volumes[0].to_bits()),
                AtomicU32::new(initial_volumes[1].to_bits()),
                AtomicU32::new(initial_volumes[2].to_bits()),
                AtomicU32::new(initial_volumes[3].to_bits()),
                AtomicU32::new(initial_volumes[4].to_bits()),
            ],
            mute: [
                AtomicBool::new(initial_mutes[0]),
                AtomicBool::new(initial_mutes[1]),
                AtomicBool::new(initial_mutes[2]),
                AtomicBool::new(initial_mutes[3]),
                AtomicBool::new(initial_mutes[4]),
            ],
            processing_load: AtomicU32::new(0.0f32.to_bits()),
            resampler_load: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    /// Return the requested target volume (dB) for `fader`.
    pub fn target_volume(&self, fader: usize) -> f32 {
        f32::from_bits(self.target_volume[fader].load(Ordering::Relaxed))
    }

    /// Set the target volume (dB) for `fader` and record the timestamp of the change.
    pub fn set_target_volume(&self, fader: usize, target: f32) {
        self.target_volume[fader].store(target.to_bits(), Ordering::Relaxed);
        self.target_volume_set_at[fader].store(nanos_since_epoch(), Ordering::Relaxed);
    }

    /// Return the [`nanos_since_epoch`] timestamp when the target volume for `fader` was last set.
    pub fn target_volume_set_at(&self, fader: usize) -> u64 {
        self.target_volume_set_at[fader].load(Ordering::Relaxed)
    }

    /// Return the currently applied volume (dB) for `fader` (may lag behind the target during a ramp).
    pub fn current_volume(&self, fader: usize) -> f32 {
        f32::from_bits(self.current_volume[fader].load(Ordering::Relaxed))
    }

    /// Immediately snap all current volumes to their targets, bypassing any ramp.
    pub fn sync_volumes_to_target(&self) {
        for fader in 0..Self::NUM_FADERS {
            let target = self.target_volume(fader);
            self.set_current_volume(fader, target);
        }
    }

    /// Set the currently applied volume (dB) for `fader`.
    pub fn set_current_volume(&self, fader: usize, current: f32) {
        self.current_volume[fader].store(current.to_bits(), Ordering::Relaxed)
    }

    /// Return the mute state for `fader`.
    pub fn is_mute(&self, fader: usize) -> bool {
        self.mute[fader].load(Ordering::Relaxed)
    }

    /// Set the mute state for `fader`.
    pub fn set_mute(&self, fader: usize, mute: bool) {
        self.mute[fader].store(mute, Ordering::Relaxed)
    }

    /// Toggle the mute state for `fader`; returns the previous state.
    pub fn toggle_mute(&self, fader: usize) -> bool {
        self.mute[fader].fetch_xor(true, Ordering::Relaxed)
    }

    /// Return a snapshot of target volumes (dB) for all faders.
    pub fn volumes(&self) -> [f32; Self::NUM_FADERS] {
        [
            f32::from_bits(self.target_volume[0].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[1].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[2].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[3].load(Ordering::Relaxed)),
            f32::from_bits(self.target_volume[4].load(Ordering::Relaxed)),
        ]
    }

    /// Return a snapshot of mute states for all faders.
    pub fn mutes(&self) -> [bool; Self::NUM_FADERS] {
        [
            self.mute[0].load(Ordering::Relaxed),
            self.mute[1].load(Ordering::Relaxed),
            self.mute[2].load(Ordering::Relaxed),
            self.mute[3].load(Ordering::Relaxed),
            self.mute[4].load(Ordering::Relaxed),
        ]
    }

    /// Store the pipeline processing load as a percentage (100 % = one chunk duration).
    pub fn set_processing_load(&self, load: f32) {
        self.processing_load
            .store(load.to_bits(), Ordering::Relaxed)
    }

    /// Return the last recorded pipeline processing load percentage.
    pub fn processing_load(&self) -> f32 {
        f32::from_bits(self.processing_load.load(Ordering::Relaxed))
    }

    /// Store the resampler processing load as a percentage.
    pub fn set_resampler_load(&self, load: f32) {
        self.resampler_load.store(load.to_bits(), Ordering::Relaxed)
    }

    /// Return the last recorded resampler processing load percentage.
    pub fn resampler_load(&self) -> f32 {
        f32::from_bits(self.resampler_load.load(Ordering::Relaxed))
    }
}

impl Default for ProcessingParameters {
    fn default() -> Self {
        Self::new(
            &[
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
                Self::DEFAULT_VOLUME,
            ],
            &[
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
                Self::DEFAULT_MUTE,
            ],
        )
    }
}

/// Shared status of the current processing run, primarily recording why it stopped.
#[derive(Clone, Debug)]
pub struct ProcessingStatus {
    /// The reason the last processing run ended (or `None` while still running).
    pub stop_reason: StopReason,
}

/// Reason a processing run ended, reported via [`ProcessingStatus`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

/// Bundle of `Arc`-wrapped status objects passed between the engine, device threads, and WebSocket server.
#[derive(Clone)]
pub struct StatusStructs {
    /// Shared capture status (sample rate, signal levels, processing state).
    pub capture: Arc<RwLock<CaptureStatus>>,
    /// Shared playback status (buffer level, signal levels, clipped samples).
    pub playback: Arc<RwLock<PlaybackStatus>>,
    /// Lock-free volume, mute, and load parameters.
    pub processing: Arc<ProcessingParameters>,
    /// Stop reason and other run-level status.
    pub status: Arc<RwLock<ProcessingStatus>>,
}

impl Default for CaptureStatus {
    fn default() -> Self {
        Self {
            measured_samplerate: 0,
            update_interval: 1000,
            signal_range: 0.0,
            rate_adjust: 0.0,
            state: ProcessingState::Inactive,
            signal_rms: utils::countertimer::ValueHistory::new(1024, 2),
            signal_peak: utils::countertimer::ValueHistory::new(1024, 2),
            used_channels: Vec::new(),
            audio_buffer: Default::default(),
        }
    }
}

impl Default for PlaybackStatus {
    fn default() -> Self {
        Self {
            buffer_level: 0,
            clipped_samples: 0,
            update_interval: 1000,
            signal_rms: utils::countertimer::ValueHistory::new(1024, 2),
            signal_peak: utils::countertimer::ValueHistory::new(1024, 2),
            audio_buffer: Default::default(),
        }
    }
}

impl Default for StatusStructs {
    fn default() -> Self {
        Self {
            capture: Arc::new(RwLock::new(CaptureStatus::default())),
            playback: Arc::new(RwLock::new(PlaybackStatus::default())),
            processing: Arc::new(ProcessingParameters::default()),
            status: Arc::new(RwLock::new(ProcessingStatus {
                stop_reason: StopReason::None,
            })),
        }
    }
}

/// Shared access to the active and previous configurations, used when hot-reloading.
pub struct SharedConfigs {
    /// The configuration currently driving the running pipeline, if any.
    pub active: Arc<Mutex<Option<config::Configuration>>>,
    /// The configuration that was active before the last reload, for diffing.
    pub previous: Arc<Mutex<Option<config::Configuration>>>,
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

/// Return `(playback_types, capture_types)`: the device type names supported by this build.
pub fn list_supported_devices() -> (Vec<String>, Vec<String>) {
    let mut playbacktypes = vec!["File".to_owned(), "Stdout".to_owned()];
    let mut capturetypes = vec![
        "RawFile".to_owned(),
        "WavFile".to_owned(),
        "Stdin".to_owned(),
        "SignalGenerator".to_owned(),
    ];

    if cfg!(target_os = "linux") {
        playbacktypes.push("Alsa".to_owned());
        capturetypes.push("Alsa".to_owned());
    }
    if cfg!(all(target_os = "linux", feature = "pulse-backend")) {
        playbacktypes.push("Pulse".to_owned());
        capturetypes.push("Pulse".to_owned());
    }
    if cfg!(all(target_os = "linux", feature = "pipewire-backend")) {
        playbacktypes.push("PipeWire".to_owned());
        capturetypes.push("PipeWire".to_owned());
    }
    if cfg!(all(target_os = "linux", feature = "bluez-backend")) {
        capturetypes.push("Bluez".to_owned());
    }
    if cfg!(all(target_os = "linux", feature = "jack-backend")) {
        playbacktypes.push("Jack".to_owned());
        capturetypes.push("Jack".to_owned());
    }
    if cfg!(target_os = "macos") {
        playbacktypes.push("CoreAudio".to_owned());
        capturetypes.push("CoreAudio".to_owned());
    }
    if cfg!(target_os = "windows") {
        playbacktypes.push("Wasapi".to_owned());
        capturetypes.push("Wasapi".to_owned());
    }
    if cfg!(all(target_os = "windows", feature = "asio-backend")) {
        playbacktypes.push("Asio".to_owned());
        capturetypes.push("Asio".to_owned());
    }
    (playbacktypes, capturetypes)
}

/// Curated list of standard audio sample rates used across all backends
/// for capability probing. Backends should use this instead of defining
/// their own local rate tables.
pub const STANDARD_RATES: &[u32] = &[
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    352800, 384000, 705600, 768000,
];

/// The sample formats supported by a device at a specific sample rate.
#[derive(Debug, PartialEq, Serialize)]
pub struct SamplerateCapability {
    /// Sample rate in Hz.
    pub samplerate: usize,
    /// Names of the supported sample formats at this rate.
    pub formats: Vec<String>,
}

/// The sample rates (and their formats) supported by a device at a specific channel count.
#[derive(Debug, PartialEq, Serialize)]
pub struct ChannelCapability {
    /// Number of channels.
    pub channels: usize,
    /// Supported sample rates for this channel count.
    pub samplerates: Vec<SamplerateCapability>,
}

#[derive(Debug, PartialEq, Serialize)]
pub enum CapabilityMode {
    /// Device uses a unified capability model (ALSA, CoreAudio, ASIO).
    Unified,
    /// WASAPI shared-mode capabilities (derived from the mix format).
    Shared,
    /// WASAPI exclusive-mode capabilities (probed independently).
    Exclusive,
}

/// A set of device capabilities associated with a single access mode (e.g. exclusive vs. shared).
#[derive(Debug, PartialEq, Serialize)]
pub struct DeviceCapabilitySet {
    /// The access mode these capabilities were probed under.
    pub mode: CapabilityMode,
    /// Per-channel-count capability entries.
    pub capabilities: Vec<ChannelCapability>,
}

/// Full capability descriptor for a named audio device.
#[derive(Debug, PartialEq, Serialize)]
pub struct AudioDeviceDescriptor {
    /// Backend-specific device identifier (e.g. `"hw:0,0"` for ALSA).
    pub name: String,
    /// Human-readable device name.
    pub description: String,
    /// Capability sets, one per access mode supported by the backend.
    pub capability_sets: Vec<DeviceCapabilitySet>,
}

/// Return available device names for `backend` (`"alsa"`, `"coreaudio"`, `"wasapi"`, `"asio"`).
///
/// Each entry is `(device_id, human_readable_name)`. Some backends return the same string twice.
/// Pass `input = true` for capture devices, `false` for playback.
pub fn list_available_devices(backend: &str, input: bool) -> Vec<(String, String)> {
    match backend.to_lowercase().as_str() {
        #[cfg(target_os = "linux")]
        "alsa" => alsa_backend::utils::list_device_names(input),
        #[cfg(target_os = "macos")]
        "coreaudio" => coreaudio_backend::device::list_available_devices(input),
        #[cfg(target_os = "windows")]
        "wasapi" => wasapi_backend::capabilities::list_device_names(input),
        #[cfg(all(target_os = "windows", feature = "asio-backend"))]
        "asio" => asio_backend::device::list_available_devices(),
        _ => Vec::new(),
    }
}

/// Error returned by [`get_device_capabilities`] when probing a device fails.
#[derive(Debug, PartialEq, serde::Serialize)]
pub enum DeviceError {
    /// No device with the given name was found.
    DeviceNotFound(String),
    /// The device exists but could not be opened (e.g. already in use).
    DeviceBusy(String),
    /// Any other backend-specific error.
    Other(String),
}

/// Probe and return the full capability descriptor for `device_name` on `backend`.
///
/// Pass `input = true` for capture devices, `false` for playback.
pub fn get_device_capabilities(
    backend: &str,
    device_name: &str,
    input: bool,
) -> Result<AudioDeviceDescriptor, DeviceError> {
    match backend.to_lowercase().as_str() {
        #[cfg(target_os = "linux")]
        "alsa" => alsa_backend::utils::get_device_capabilities(device_name, input),
        #[cfg(target_os = "macos")]
        "coreaudio" => coreaudio_backend::device::get_device_capabilities(device_name, input),
        #[cfg(target_os = "windows")]
        "wasapi" => wasapi_backend::capabilities::get_device_capabilities(device_name, input),
        #[cfg(all(target_os = "windows", feature = "asio-backend"))]
        "asio" => asio_backend::device::get_device_capabilities(device_name, input),
        _ => Err(DeviceError::Other("Unsupported backend".to_string())),
    }
}
