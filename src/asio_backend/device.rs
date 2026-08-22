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

// ASIO backend for playback and capture.
// This implementation uses the azo crate, which talks to the driver COM objects
// directly instead of going through the Steinberg ASIO SDK.

use crate::ToF32;
use std::collections::VecDeque;
use std::ffi::{c_long, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex, OnceLock};
use std::thread;

use crossbeam_channel::{TrySendError, bounded};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use ringbuf::{HeapRb, traits::*};

use azo::dto::ChannelId;
use azo::sys::{
    Bool, BufferSwitch, BufferSwitchTimeInfo, Callbacks, MessageSelector, SampleRate,
    SampleRateDidChange, Time,
};

use crate::CommandMessage;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::asio_backend::driver::{
    com_init_this_thread, driver_is_loaded, teardown_asio_driver, with_driver,
};
use crate::asio_backend::utils::{
    ChannelBuffers, asio_format_to_str, asio_sample_type_name, copy_from_queue_at_offset,
    create_asio_buffers, dispose_asio_buffers, get_preferred_buffer_size, log_asio_latencies,
    make_channel_ids, read_current_asio_sample_rate_hz, resolve_binary_format, resolve_format,
    start_asio_stream, stop_asio_stream,
};
use crate::audiochunk::ChunkStats;
use crate::audiodevice::*;
use crate::config::{AsioSampleFormat, BinarySampleFormat, ConfigError};
use crate::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::utils::countertimer;
use crate::utils::rate_controller::PIRateController;
use crate::utils::resampling::{ChunkResampler, new_resampler, resampler_is_async};
use crate::{CaptureStatus, PlaybackStatus};

// ---------------------------------------------------------------------------
// Public device structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AsioPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: Option<AsioSampleFormat>,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
    pub full_duplex: bool,
}

#[derive(Clone, Debug)]
pub struct AsioCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub capture_samplerate: usize,
    pub resampler_config: Option<crate::config::Resampler>,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: Option<AsioSampleFormat>,
    pub silence_threshold: f64,
    pub silence_timeout: f64,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
    pub full_duplex: bool,
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Context passed to the ASIO playback callback via a global AtomicPtr.
struct AsioPlaybackContext {
    /// Ring buffer consumer — reads bytes written by the device loop.
    device_consumer: ringbuf::wrap::caching::Caching<Arc<HeapRb<u8>>, false, true>,
    /// Sample queue used inside the callback to buffer partial reads.
    sample_queue: VecDeque<u8>,
    /// Double-buffer pointers, one entry per channel.
    channel_buffers: Vec<ChannelBuffers>,
    num_channels: usize,
    buffer_size: usize,
    bytes_per_sample: usize,
    /// Preallocated scratch buffer used when reading from the ring buffer in callback.
    read_tmp: Vec<u8>,
    target_level: usize,
    /// Estimator for the current buffer fill level.
    buffer_fill: Arc<Mutex<countertimer::DeviceBufferEstimator>>,
    /// Whether the stream is currently running (receiving data).
    running: bool,
}

/// Context passed to the ASIO capture callback via a global AtomicPtr.
struct AsioCaptureContext {
    /// Ring buffer producer — writes bytes read from hardware.
    device_producer: ringbuf::wrap::caching::Caching<Arc<HeapRb<u8>>, true, false>,
    /// Notification channel: (chunk_counter, pushed_bytes).
    tx_dev: crossbeam_channel::Sender<(u64, usize)>,
    /// Double-buffer pointers, one entry per channel.
    channel_buffers: Vec<ChannelBuffers>,
    num_channels: usize,
    buffer_size: usize,
    bytes_per_sample: usize,
    /// Preallocated interleaved capture buffer reused by callback.
    interleaved_tmp: Vec<u8>,
    chunk_counter: u64,
}

static PLAYBACK_CONTEXT: AtomicPtr<AsioPlaybackContext> = AtomicPtr::new(ptr::null_mut());
static CAPTURE_CONTEXT: AtomicPtr<AsioCaptureContext> = AtomicPtr::new(ptr::null_mut());
/// Gates the capture callback until the capture loop is ready to consume.
///
/// The stream has to be started before the device loop can wait on the startup barrier, but
/// with capture and playback on separate devices the other side may take a long time to open
/// its own driver. Without this gate the callback would fill the ring buffer with audio nobody
/// is draining yet, overflow it, and leave the first rate measurement badly skewed.
static CAPTURE_STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);
static ASIO_PLAYBACK_RATE_CHANGED: AtomicBool = AtomicBool::new(false);
static ASIO_CAPTURE_RATE_CHANGED: AtomicBool = AtomicBool::new(false);

fn clear_playback_rate_change_event() {
    ASIO_PLAYBACK_RATE_CHANGED.store(false, Ordering::Release);
}

fn clear_capture_rate_change_event() {
    ASIO_CAPTURE_RATE_CHANGED.store(false, Ordering::Release);
}

fn take_playback_rate_change_event() -> bool {
    ASIO_PLAYBACK_RATE_CHANGED.swap(false, Ordering::AcqRel)
}

fn take_capture_rate_change_event() -> bool {
    ASIO_CAPTURE_RATE_CHANGED.swap(false, Ordering::AcqRel)
}

// ---------------------------------------------------------------------------
// Shared state for full-duplex ASIO coordination
// ---------------------------------------------------------------------------

/// State shared between playback and capture threads when both use the same ASIO driver.
struct AsioSharedState {
    driver_name: String,
    num_inputs: i32,
    num_outputs: i32,
    preferred_buf_size: i32,
    /// Pending output (playback) channel registration: (channel_ids, num_channels).
    pending_output: Option<(Vec<ChannelId>, usize)>,
    /// Pending input (capture) channel registration: (channel_ids, num_channels).
    pending_input: Option<(Vec<ChannelId>, usize)>,
    /// Whether the ASIO stream has been started.
    stream_started: bool,
    /// Setup error produced by the side that attempted combined startup.
    /// If set, the other side returns immediately instead of waiting indefinitely.
    setup_error: Option<String>,
    /// Number of sides (playback/capture) still active. Last one to exit stops the stream.
    active_count: u8,
    /// The `Callbacks` struct passed to `createBuffers`.
    /// The driver requires this struct to remain valid for the lifetime of the stream.
    callbacks_for_driver: Option<Box<Callbacks>>,
}

static ASIO_SHARED: OnceLock<(Mutex<Option<AsioSharedState>>, Condvar)> = OnceLock::new();
static PLAYBACK_CALLBACK_SEEN: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn playback_callback_seen_lock() -> &'static (Mutex<bool>, Condvar) {
    PLAYBACK_CALLBACK_SEEN.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn reset_playback_callback_seen() {
    let (mutex, _condvar) = playback_callback_seen_lock();
    let mut seen = mutex.lock().unwrap();
    *seen = false;
}

fn mark_playback_callback_seen() {
    let (mutex, condvar) = playback_callback_seen_lock();
    let mut seen = mutex.lock().unwrap();
    if !*seen {
        *seen = true;
        condvar.notify_all();
    }
}

fn wait_for_playback_callback(timeout: std::time::Duration) -> bool {
    let (mutex, condvar) = playback_callback_seen_lock();
    let seen = mutex.lock().unwrap();
    if *seen {
        return true;
    }
    let (seen, _timeout_res) = condvar.wait_timeout(seen, timeout).unwrap();
    *seen
}

// ---------------------------------------------------------------------------
// ASIO callbacks  (unsafe extern "system" — called from ASIO driver thread)
// ---------------------------------------------------------------------------

/// Assemble the `Callbacks` struct the driver is given.
fn make_callbacks(
    buffer_switch: BufferSwitch,
    buffer_switch_time_info: BufferSwitchTimeInfo,
    sample_rate_did_change: SampleRateDidChange,
) -> Callbacks {
    Callbacks {
        buffer_switch,
        sample_rate_did_change,
        asio_message: asio_message_callback,
        buffer_switch_time_info,
    }
}

/// ASIO bufferSwitch callback for playback.
/// Reads converted audio bytes from the ring buffer and copies them into the ASIO output buffers.
///
/// # Safety
/// Called from the ASIO driver thread. The caller must ensure that `PLAYBACK_CONTEXT`
/// points to a valid `AsioPlaybackContext` or is null.
pub unsafe extern "system" fn buffer_switch_playback(buffer_index: c_long, _direct_process: Bool) {
    xtrace!("ASIO playback callback: buffer_index={}", buffer_index);
    let ctx_ptr = PLAYBACK_CONTEXT.load(Ordering::Acquire);
    if ctx_ptr.is_null() {
        xtrace!("ASIO playback callback: null context, returning.");
        return;
    }
    if !(0..=1).contains(&buffer_index) {
        debug!("ASIO playback callback got invalid buffer index {buffer_index}, ignoring.");
        return;
    }
    let ctx = unsafe { &mut *ctx_ptr };
    if ctx.channel_buffers.len() < ctx.num_channels {
        error!(
            "ASIO playback callback buffer mismatch: buffers={}, channels={}",
            ctx.channel_buffers.len(),
            ctx.num_channels
        );
        return;
    }
    mark_playback_callback_seen();
    let bytes_per_frame = ctx.bytes_per_sample * ctx.num_channels;
    let needed_bytes = ctx.buffer_size * bytes_per_frame;
    let buffer_index = buffer_index as usize;

    // Fill the sample queue from the ring buffer
    while ctx.sample_queue.len() < needed_bytes {
        let available = ctx.device_consumer.occupied_len();
        if available == 0 {
            // No data — fill remainder with silence
            warn!(
                "ASIO playback callback: underrun, filled {} bytes of silence.",
                needed_bytes - ctx.sample_queue.len()
            );
            ctx.sample_queue.resize(needed_bytes, 0);
            if ctx.running {
                ctx.running = false;
            }
            break;
        }
        if !ctx.running {
            ctx.running = true;
            // Prefill at least one full callback's worth of frames so the loop
            // below doesn't immediately re-drain the ring buffer to empty and
            // re-trigger an underrun when target_level is smaller than the
            // driver's actual buffer size (see issue #498).
            let prefill_frames = ctx.target_level.max(ctx.buffer_size);
            // On first startup, start immediately without extra silence prefill.
            // On restart after underrun, keep target_level prefill to rebuild delay.
            let new_len = ctx.sample_queue.len() + prefill_frames * bytes_per_frame;
            ctx.sample_queue.resize(new_len, 0);
        }
        let to_read = available.min(needed_bytes.saturating_sub(ctx.sample_queue.len()));
        let tmp = &mut ctx.read_tmp[0..to_read];
        ctx.device_consumer.pop_slice(tmp);
        ctx.sample_queue.extend(tmp.iter().copied());
    }

    // Copy interleaved data into per-channel ASIO buffers (de-interleave)
    let mut src_offset = 0usize;
    for frame in 0..ctx.buffer_size {
        for ch in 0..ctx.num_channels {
            let out_ptr = ctx.channel_buffers[ch][buffer_index];
            if !out_ptr.is_null() {
                let dst = unsafe { (out_ptr as *mut u8).add(frame * ctx.bytes_per_sample) };
                let dst_slice =
                    unsafe { std::slice::from_raw_parts_mut(dst, ctx.bytes_per_sample) };
                copy_from_queue_at_offset(&ctx.sample_queue, src_offset, dst_slice);
            } else if frame == 0 {
                xtrace!(
                    "ASIO playback callback: null output buffer pointer at channel {}, index {}.",
                    ch,
                    buffer_index
                );
            }
            src_offset += ctx.bytes_per_sample;
        }
    }
    if needed_bytes > 0 {
        ctx.sample_queue.drain(0..needed_bytes);
    }

    // Update buffer fill estimate.
    // Include both the callback-local queue and the remaining ringbuffer data
    // to represent total pending playback frames.
    let curr_buffer_fill =
        (ctx.sample_queue.len() + ctx.device_consumer.occupied_len()) / bytes_per_frame;
    if let Ok(mut estimator) = ctx.buffer_fill.try_lock() {
        estimator.add(curr_buffer_fill);
    }
}

/// ASIO bufferSwitch callback for capture.
/// Reads audio bytes from the ASIO input buffers and pushes them into the ring buffer.
///
/// # Safety
/// Called from the ASIO driver thread. The caller must ensure that `CAPTURE_CONTEXT`
/// points to a valid `AsioCaptureContext` or is null.
pub unsafe extern "system" fn buffer_switch_capture(buffer_index: c_long, _direct_process: Bool) {
    xtrace!("ASIO capture callback: buffer_index={}", buffer_index);
    if !CAPTURE_STREAM_ACTIVE.load(Ordering::Acquire) {
        // The capture loop is not consuming yet, drop this buffer instead of filling the
        // ring buffer with audio that would only be discarded. See CAPTURE_STREAM_ACTIVE.
        xtrace!("ASIO capture callback: stream not active yet, discarding buffer.");
        return;
    }
    let ctx_ptr = CAPTURE_CONTEXT.load(Ordering::Acquire);
    if ctx_ptr.is_null() {
        debug!("ASIO capture callback: null context, returning.");
        return;
    }
    if !(0..=1).contains(&buffer_index) {
        debug!("ASIO capture callback got invalid buffer index {buffer_index}, ignoring.");
        return;
    }
    let ctx = unsafe { &mut *ctx_ptr };
    if ctx.channel_buffers.len() < ctx.num_channels {
        error!(
            "ASIO capture callback buffer mismatch: buffers={}, channels={}",
            ctx.channel_buffers.len(),
            ctx.num_channels
        );
        return;
    }
    let bytes_per_frame = ctx.bytes_per_sample * ctx.num_channels;
    let total_bytes = ctx.buffer_size * bytes_per_frame;
    let buffer_index = buffer_index as usize;
    if ctx.interleaved_tmp.len() != total_bytes {
        error!(
            "ASIO capture callback buffer size mismatch: scratch={}, expected={}",
            ctx.interleaved_tmp.len(),
            total_bytes
        );
        return;
    }
    let buf = &mut ctx.interleaved_tmp;

    // Read from per-channel ASIO input buffers and interleave into buf
    for frame in 0..ctx.buffer_size {
        for ch in 0..ctx.num_channels {
            let in_ptr = ctx.channel_buffers[ch][buffer_index];
            if !in_ptr.is_null() {
                let src = unsafe { (in_ptr as *const u8).add(frame * ctx.bytes_per_sample) };
                let offset = (frame * ctx.num_channels + ch) * ctx.bytes_per_sample;
                for byte_idx in 0..ctx.bytes_per_sample {
                    buf[offset + byte_idx] = unsafe { *src.add(byte_idx) };
                }
            } else if frame == 0 {
                xtrace!(
                    "ASIO capture callback: null input buffer pointer at channel {}, index {}.",
                    ch,
                    buffer_index
                );
            }
        }
    }

    // Push into ring buffer
    let pushed_bytes = ctx.device_producer.push_slice(buf);
    if pushed_bytes < buf.len() {
        // Ring buffer full — data will be lost
        warn!(
            "ASIO capture callback: ringbuffer full, dropped {} of {} bytes.",
            buf.len() - pushed_bytes,
            buf.len()
        );
    }
    match ctx.tx_dev.try_send((ctx.chunk_counter, pushed_bytes)) {
        Ok(()) => {}
        Err(TrySendError::Full((nbr, length_bytes))) => {
            // Channel full, drop notification
            xtrace!(
                "ASIO capture callback: notify channel full, dropped notification chunk={}, bytes={}",
                nbr,
                length_bytes
            );
            let _ = (nbr, length_bytes);
        }
        Err(_) => {
            // Channel disconnected
            xtrace!("ASIO capture callback: notification channel disconnected.");
        }
    }
    ctx.chunk_counter += 1;
}

/// ASIO bufferSwitchTimeInfo callback for playback.
/// Some drivers call this callback path even when only bufferSwitch is expected.
///
/// # Safety
/// Called from the ASIO driver thread. `params` is provided by the driver.
pub unsafe extern "system" fn buffer_switch_timeinfo_playback(
    params: *mut Time,
    buffer_index: c_long,
    direct_process: Bool,
) -> *mut Time {
    unsafe {
        buffer_switch_playback(buffer_index, direct_process);
    }
    params
}

/// ASIO bufferSwitchTimeInfo callback for capture.
///
/// # Safety
/// Called from the ASIO driver thread. `params` is provided by the driver.
pub unsafe extern "system" fn buffer_switch_timeinfo_capture(
    params: *mut Time,
    buffer_index: c_long,
    direct_process: Bool,
) -> *mut Time {
    unsafe {
        buffer_switch_capture(buffer_index, direct_process);
    }
    params
}

/// ASIO bufferSwitchTimeInfo callback for full-duplex mode.
///
/// # Safety
/// Called from the ASIO driver thread. `params` is provided by the driver.
pub unsafe extern "system" fn buffer_switch_timeinfo_combined(
    params: *mut Time,
    buffer_index: c_long,
    direct_process: Bool,
) -> *mut Time {
    unsafe {
        buffer_switch_combined(buffer_index, direct_process);
    }
    params
}

/// ASIO sampleRateDidChange callback.
///
/// # Safety
/// Called from the ASIO driver thread. `_s_rate` is provided by the driver.
pub unsafe extern "system" fn sample_rate_changed_callback(_s_rate: SampleRate) {
    ASIO_PLAYBACK_RATE_CHANGED.store(true, Ordering::Release);
    ASIO_CAPTURE_RATE_CHANGED.store(true, Ordering::Release);
    warn!("ASIO sampleRateDidChange callback received.");
}

/// ASIO asioMessage callback.
/// Handles driver queries about supported features.
/// Returning 0 means "not supported" or "no" for most selectors.
///
/// # Safety
/// Called from the ASIO driver thread. All parameters are provided by the driver.
pub unsafe extern "system" fn asio_message_callback(
    selector: MessageSelector,
    value: c_long,
    _message: *const c_void,
    _opt: *const f64,
) -> c_long {
    match selector {
        MessageSelector::SELECTOR_SUPPORTED => {
            // The driver asks if we support a given selector.
            match MessageSelector(value) {
                MessageSelector::SELECTOR_SUPPORTED
                | MessageSelector::ENGINE_VERSION
                | MessageSelector::RESET_REQUEST
                | MessageSelector::RESYNC_REQUEST
                | MessageSelector::LATENCIES_CHANGED
                | MessageSelector::SUPPORTS_TIME_INFO => 1, // yes
                // Dynamic buffer resize is not implemented, and the time code part of
                // the ASIOTime struct is ignored.
                MessageSelector::BUFFER_SIZE_CHANGE | MessageSelector::SUPPORTS_TIME_CODE => 0,
                _ => 0,
            }
        }
        MessageSelector::ENGINE_VERSION => 2, // ASIO 2.0
        MessageSelector::SUPPORTS_TIME_INFO => 1,
        MessageSelector::SUPPORTS_TIME_CODE => 0,
        MessageSelector::RESET_REQUEST => {
            warn!("ASIO reset request received. A stream restart may be required by the driver.");
            1
        }
        MessageSelector::BUFFER_SIZE_CHANGE => {
            warn!(
                "ASIO buffer size change request received. Dynamic resize is not implemented in this backend."
            );
            0
        }
        MessageSelector::RESYNC_REQUEST => {
            debug!("ASIO resync request received.");
            1
        }
        MessageSelector::LATENCIES_CHANGED => {
            debug!("ASIO latencies changed notification.");
            1
        }
        other => {
            trace!("Unhandled ASIO message selector {}.", other.0);
            0
        }
    }
}

/// Combined ASIO bufferSwitch callback for full-duplex mode.
/// Dispatches to both playback and capture callbacks.
///
/// # Safety
/// Called from the ASIO driver thread. Both `PLAYBACK_CONTEXT` and `CAPTURE_CONTEXT`
/// must point to valid contexts or be null.
pub unsafe extern "system" fn buffer_switch_combined(buffer_index: c_long, direct_process: Bool) {
    unsafe {
        buffer_switch_playback(buffer_index, direct_process);
        buffer_switch_capture(buffer_index, direct_process);
    }
}

// ---------------------------------------------------------------------------
// Full-duplex coordination helpers
// ---------------------------------------------------------------------------

/// Initialize the shared ASIO driver state.
/// The first caller loads and initialises the driver. Subsequent callers for the same driver
/// reuse the existing state. Returns (num_inputs, num_outputs, preferred_buf_size).
fn init_shared_asio(devname: &str, samplerate: usize) -> Result<(i32, i32, i32), ConfigError> {
    trace!(
        "init_shared_asio: dev='{}', samplerate={}",
        devname, samplerate
    );
    let (mutex, _condvar) = ASIO_SHARED.get_or_init(|| (Mutex::new(None), Condvar::new()));
    let mut guard = mutex.lock().unwrap();

    if let Some(ref shared) = *guard {
        // Driver already loaded by the other side. This path is only taken in full-duplex
        // mode, which by definition means both sides named the same device, so a mismatch
        // here means the shared state was left behind by an earlier session.
        if shared.driver_name != devname {
            return Err(ConfigError::new(&format!(
                "Full-duplex ASIO state is still held by device '{}' while opening '{devname}'",
                shared.driver_name
            )));
        }
        trace!(
            "init_shared_asio: reusing existing shared state for '{}'",
            shared.driver_name
        );
        Ok((
            shared.num_inputs,
            shared.num_outputs,
            shared.preferred_buf_size,
        ))
    } else {
        // First caller — load and initialise the driver
        let (num_inputs, num_outputs) = open_asio_device(devname, samplerate)?;

        // Query preferred buffer size
        let preferred_buf = get_preferred_buffer_size(devname)?;

        *guard = Some(AsioSharedState {
            driver_name: devname.to_string(),
            num_inputs,
            num_outputs,
            preferred_buf_size: preferred_buf,
            pending_output: None,
            pending_input: None,
            stream_started: false,
            setup_error: None,
            active_count: 0,
            callbacks_for_driver: None,
        });

        Ok((num_inputs, num_outputs, preferred_buf))
    }
}

/// Register one side (playback or capture) for full-duplex operation.
///
/// When both sides have registered, the second caller creates the combined ASIO buffers,
/// updates both contexts' `buffer_infos` through the global atomics, and calls `ASIOStart()`.
/// The first caller blocks on a condvar until this is done.
fn register_and_wait(is_input: bool, num_channels: usize) -> Result<(), ConfigError> {
    trace!(
        "register_and_wait: is_input={}, num_channels={}",
        is_input, num_channels
    );
    let (mutex, condvar) = ASIO_SHARED
        .get()
        .expect("ASIO_SHARED must be initialised before register_and_wait");
    let mut guard = mutex.lock().unwrap();

    if let Some(existing_error) = guard.as_ref().and_then(|shared| shared.setup_error.clone()) {
        return Err(ConfigError::new(&format!(
            "ASIO full-duplex setup aborted: {existing_error}"
        )));
    }

    // Register our channels
    {
        let shared = guard.as_mut().expect("shared state must exist");
        let channel_ids = make_channel_ids(num_channels, is_input);
        trace!(
            "ASIO register side: is_input={}, num_channels={}, stream_started={}, active_count={}",
            is_input, num_channels, shared.stream_started, shared.active_count
        );
        if is_input {
            shared.pending_input = Some((channel_ids, num_channels));
        } else {
            shared.pending_output = Some((channel_ids, num_channels));
        }
    }

    let both_ready = {
        let shared = guard.as_ref().unwrap();
        shared.pending_input.is_some() && shared.pending_output.is_some()
    };

    if both_ready {
        // I am the second side to register — do combined buffer creation + start.
        let shared = guard.as_mut().unwrap();
        let devname = shared.driver_name.clone();
        let (out_channels, out_ch) = shared.pending_output.take().unwrap();
        let (in_channels, in_ch) = shared.pending_input.take().unwrap();
        let preferred_buf = shared.preferred_buf_size;
        trace!(
            "ASIO both sides ready: out_ch={}, in_ch={}, preferred_buf={}",
            out_ch, in_ch, preferred_buf
        );

        // Build combined list: outputs first, then inputs.
        let mut combined: Vec<ChannelId> = Vec::with_capacity(out_ch + in_ch);
        combined.extend(out_channels);
        combined.extend(in_channels);

        // Heap-allocate callbacks so the struct remains at a stable address.
        // The driver requires it to remain valid for the lifetime of the stream.
        shared.callbacks_for_driver = Some(Box::new(make_callbacks(
            buffer_switch_combined,
            buffer_switch_timeinfo_combined,
            sample_rate_changed_callback,
        )));
        trace!("register_and_wait: callbacks registered for combined stream, creating buffers");

        // SAFETY: the callbacks live in the shared state, which outlives the stream.
        let callbacks_ptr: *const Callbacks = shared.callbacks_for_driver.as_deref().unwrap();
        let channel_buffers =
            match unsafe { create_asio_buffers(&devname, &combined, preferred_buf, callbacks_ptr) }
            {
                Ok(buffers) => buffers,
                Err(err) => {
                    let msg = format!("createBuffers failed in full-duplex setup: {err}");
                    shared.setup_error = Some(msg.clone());
                    condvar.notify_all();
                    return Err(ConfigError::new(&msg));
                }
            };

        // Hand each side its slice of the buffer pointers through the global atomics.
        // Both contexts are guaranteed to be stored before register_and_wait is called.
        let pb_ctx = PLAYBACK_CONTEXT.load(Ordering::Acquire);
        if !pb_ctx.is_null() {
            unsafe {
                (*pb_ctx).channel_buffers = channel_buffers[..out_ch].to_vec();
            }
        }
        let cap_ctx = CAPTURE_CONTEXT.load(Ordering::Acquire);
        if !cap_ctx.is_null() {
            unsafe {
                (*cap_ctx).channel_buffers = channel_buffers[out_ch..].to_vec();
            }
        }

        log_asio_latencies(&devname);

        // Start the stream
        trace!("register_and_wait: starting the stream (full-duplex)");
        if let Err(err) = start_asio_stream(&devname) {
            let msg = format!("Failed to start ASIO stream: {err}");
            shared.setup_error = Some(msg.clone());
            condvar.notify_all();
            return Err(ConfigError::new(&msg));
        }
        debug!("Full-duplex ASIO stream started.");
        trace!("register_and_wait: ASIOStart returned success");

        shared.stream_started = true;
        shared.setup_error = None;
        shared.active_count = 2;
        condvar.notify_all();
    } else {
        // I am the first side — wait for the other side to complete setup.
        debug!("Waiting for other ASIO side to register for full-duplex...");
        while !guard.as_ref().unwrap().stream_started
            && guard.as_ref().unwrap().setup_error.is_none()
        {
            guard = condvar.wait(guard).unwrap();
        }
        if let Some(setup_error) = guard.as_ref().unwrap().setup_error.clone() {
            return Err(ConfigError::new(&format!(
                "ASIO full-duplex setup aborted: {setup_error}"
            )));
        }
        debug!("Full-duplex ASIO setup complete, proceeding.");
    }

    Ok(())
}

/// Decrement the active-sides counter. When it reaches zero, stop the ASIO stream
/// and clear the shared state so a fresh session can be started later.
///
/// Both context pointers are nulled before `ASIOStop()` is called so that even a
/// late-arriving callback (possible on some drivers) sees null and returns harmlessly.
/// By the time either thread enters cleanup, both have exited their main loops, so
/// the contexts are only accessed from callbacks.
fn release_shared_asio() {
    let Some((mutex, _condvar)) = ASIO_SHARED.get() else {
        return;
    };
    let mut guard = mutex.lock().unwrap();
    if let Some(ref mut shared) = *guard {
        shared.active_count = shared.active_count.saturating_sub(1);
        if shared.active_count == 1 {
            // First side to exit — null both context pointers, then stop the stream.
            debug!("First ASIO side exiting, stopping stream.");
            PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
            CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
            if let Err(err) = stop_asio_stream(&shared.driver_name) {
                trace!("Stopping the ASIO stream on first side exit failed: {err}");
            }
        } else if shared.active_count == 0 {
            // Last side to exit — dispose buffers and the driver.
            // The stream was already stopped by the first side.
            debug!("Last ASIO side exiting, disposing driver.");
            if let Err(err) = dispose_asio_buffers(&shared.driver_name) {
                trace!("Disposing ASIO buffers on last side exit failed: {err}");
            }
            teardown_asio_driver(&shared.driver_name);
            *guard = None; // Reset for next session
        }
    }
}

// ---------------------------------------------------------------------------
// ASIO low-level helpers
// ---------------------------------------------------------------------------

pub use crate::asio_backend::driver::load_driver_by_name;

/// Open an ASIO device: load driver, init, set sample rate, query channels.
/// The sample rate is set immediately after ASIOInit, before any other calls,
/// because some ASIO drivers lock in the rate once channels or buffers are queried.
/// Returns (num_inputs, num_outputs).
pub fn open_asio_device(devname: &str, samplerate: usize) -> Result<(i32, i32), ConfigError> {
    trace!(
        "open_asio_device: dev='{}', samplerate={}",
        devname, samplerate
    );
    let available = list_device_names();
    debug!("Available ASIO devices: {:?}", available);
    if let Err(load_err) = load_driver_by_name(devname) {
        // Driver load failed — provide a helpful error with available devices.
        let err_desc = load_err.to_string();
        let exact_match = available.iter().any(|n| n == devname);
        let hint = if exact_match {
            String::from(
                " A driver matching the provided name was found, so the device may be turned off or disconnected.",
            )
        } else {
            String::from(" No driver matching the provided name was found.")
        };
        let msg = if exact_match {
            format!(
                "Failed to load ASIO driver '{}': {}{}",
                devname, err_desc, hint
            )
        } else {
            format!(
                "Failed to load ASIO driver '{}': {} Available devices: {:?}.{}",
                devname, err_desc, available, hint
            )
        };
        return Err(ConfigError::new(&msg));
    }

    // Log current sample rate before any changes
    let current_rate = with_driver(devname, |driver| {
        driver
            .get_sample_rate()
            .map_err(|err| ConfigError::new(&format!("Failed to read ASIO sample rate: {err:?}")))
    })?;
    debug!("ASIO current sample rate: {current_rate} Hz");

    // Log supported sample rates
    let supported: Vec<u32> = with_driver(devname, |driver| {
        Ok(crate::STANDARD_RATES
            .iter()
            .copied()
            .filter(|&r| driver.can_sample_rate(r as f64).is_ok())
            .collect())
    })?;
    debug!("ASIO supported sample rates: {:?}", supported);

    // Set the requested sample rate IMMEDIATELY after init, before getChannels.
    // Some drivers lock in the rate once channels or buffers are queried.
    let rate = samplerate as f64;
    let rate_supported = with_driver(devname, |driver| Ok(driver.can_sample_rate(rate).is_ok()))?;
    if !rate_supported {
        return Err(ConfigError::new(&format!(
            "ASIO device does not support sample rate {samplerate} Hz. Supported rates: {supported:?}"
        )));
    }

    // Check if the rate is already correct
    let already_correct = (current_rate - rate).abs() <= 0.5;

    if already_correct {
        debug!("ASIO sample rate already at {samplerate} Hz, no change needed.");
    } else {
        // Try setting on the current driver instance
        with_driver(devname, |driver| {
            driver.set_sample_rate(rate).map_err(|err| {
                ConfigError::new(&format!(
                    "Failed to set ASIO sample rate to {samplerate} Hz: {err:?}"
                ))
            })
        })?;

        // Recreating the driver instance is what actually makes the new rate take effect.
        // Do NOT remove this: without it the Steinberg built-in driver reports the
        // requested rate from getSampleRate while the hardware keeps running at the old
        // one, which was measured directly (asking for 96 kHz gave a measured capture rate
        // of ~48 kHz, and 48 kHz requests became unstable). Only the measured rate exposes
        // this, the driver's own report does not.
        //
        // Keep the teardown a real drop: releasing the COM object is what resets the
        // driver's own state, so leaking the handle instead would silently break this.
        teardown_asio_driver(devname);
        load_driver_by_name(devname)?;
        let after_reload = with_driver(devname, |driver| {
            driver.get_sample_rate().map_err(|err| {
                ConfigError::new(&format!(
                    "Failed to read ASIO sample rate after reinitialising: {err:?}"
                ))
            })
        })?;

        if (after_reload - rate).abs() > 0.5 {
            return Err(ConfigError::new(&format!(
                "ASIO device still reports {after_reload} Hz after being asked for \
                 {samplerate} Hz and reinitialised. The driver may require the rate to be \
                 set from its own control panel."
            )));
        }
        debug!("ASIO sample rate {samplerate} Hz applied after reinitialising the driver.");
    }

    // Query channels AFTER the sample rate has been set.
    let counts = with_driver(devname, |driver| {
        driver
            .channel_counts()
            .map_err(|err| ConfigError::new(&format!("getChannels failed: {err:?}")))
    })?;
    let (num_inputs, num_outputs) = (counts.in_, counts.out);
    debug!("ASIO device opened: {num_inputs} input channels, {num_outputs} output channels.");

    // Log per-channel details (name and sample format)
    log_channel_details(devname, num_inputs, true);
    log_channel_details(devname, num_outputs, false);

    Ok((num_inputs, num_outputs))
}

/// Log the name and sample format of each channel in one direction.
fn log_channel_details(devname: &str, num_channels: i32, is_input: bool) {
    let direction = if is_input { "Input " } else { "Output" };
    for ch in 0..num_channels {
        let info = with_driver(devname, |driver| {
            driver
                .channel_info(ChannelId {
                    input: is_input,
                    index: ch,
                })
                .map_err(|err| ConfigError::new(&format!("getChannelInfo failed: {err:?}")))
        });
        if let Ok(info) = info {
            debug!(
                "  {direction} channel {ch}: name='{}', format={} ({})",
                info.name.to_string_lossy(),
                info.sample_type.0,
                asio_sample_type_name(info.sample_type),
            );
        }
    }
}

/// Open and set up an ASIO device for playback.
/// Returns resolved_format.
fn open_asio_playback(
    devname: &str,
    num_channels: usize,
    samplerate: usize,
    configured_format: &Option<AsioSampleFormat>,
) -> Result<AsioSampleFormat, ConfigError> {
    let (_inputs, outputs) = open_asio_device(devname, samplerate)?;
    if num_channels > outputs as usize {
        return Err(ConfigError::new(&format!(
            "Requested {num_channels} output channels but device only has {outputs}"
        )));
    }
    let resolved_format = resolve_format(devname, configured_format, false)?;
    Ok(resolved_format)
}

/// Open and set up an ASIO device for capture.
/// Returns resolved_format.
fn open_asio_capture(
    devname: &str,
    num_channels: usize,
    samplerate: usize,
    configured_format: &Option<AsioSampleFormat>,
) -> Result<AsioSampleFormat, ConfigError> {
    let (inputs, _outputs) = open_asio_device(devname, samplerate)?;
    if num_channels > inputs as usize {
        return Err(ConfigError::new(&format!(
            "Requested {num_channels} input channels but device only has {inputs}"
        )));
    }
    let resolved_format = resolve_format(devname, configured_format, true)?;
    Ok(resolved_format)
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

pub use crate::asio_backend::driver::list_device_names;

/// List available ASIO devices as (name, description) pairs.
pub fn list_available_devices() -> Vec<(String, String)> {
    let names = list_device_names();
    names.iter().map(|n| (n.clone(), n.clone())).collect()
}

pub fn get_device_capabilities(
    device_name: &str,
    input: bool,
) -> Result<crate::AudioDeviceDescriptor, crate::DeviceError> {
    // Refuse to probe if an in-process ASIO driver is already loaded (live stream).
    // load_driver_by_name() unconditionally tears down any loaded driver, which
    // would silently interrupt active playback or capture.
    if driver_is_loaded(device_name) {
        return Err(crate::DeviceError::DeviceBusy(format!(
            "ASIO driver is already in use; cannot probe '{device_name}' while a stream is active"
        )));
    }

    let names = list_device_names();
    if !names.contains(&device_name.to_string()) {
        return Err(crate::DeviceError::DeviceNotFound(device_name.to_string()));
    }

    // Probing ASIO requires loading the driver.
    if let Err(e) = load_driver_by_name(device_name) {
        return Err(crate::DeviceError::Other(format!("{e}")));
    }

    let supported_rates: Vec<u32> = with_driver(device_name, |driver| {
        Ok(crate::STANDARD_RATES
            .iter()
            .copied()
            .filter(|&rate| driver.can_sample_rate(rate as f64).is_ok())
            .collect())
    })
    .unwrap_or_default();

    // ASIO uses one fixed format for all channels and rates within a driver.
    let direction_name = if input { "capture" } else { "playback" };
    let fmt = match resolve_format(device_name, &None, input) {
        Ok(fmt) => fmt,
        Err(_) => {
            teardown_asio_driver(device_name);
            return Err(crate::DeviceError::Other(format!(
                "Failed to detect {direction_name} sample format for ASIO device '{device_name}'"
            )));
        }
    };
    let fmt_str = asio_format_to_str(fmt).to_string();

    // Get channel count for the requested direction.
    // A failure from getChannels indicates a real driver error; treat it as a probe
    // failure rather than silently returning empty capabilities.
    let counts = match with_driver(device_name, |driver| {
        driver
            .channel_counts()
            .map_err(|err| ConfigError::new(&format!("getChannels failed: {err:?}")))
    }) {
        Ok(counts) => counts,
        Err(err) => {
            teardown_asio_driver(device_name);
            return Err(crate::DeviceError::Other(format!(
                "getChannels failed for '{device_name}': {err}"
            )));
        }
    };

    teardown_asio_driver(device_name);

    let channels = if input {
        counts.in_ as usize
    } else {
        counts.out as usize
    };

    // A driver that only supports the opposite direction will report 0 channels
    // for the requested direction — filter that out rather than emitting a
    // zero-channel capability entry.
    if channels == 0 || supported_rates.is_empty() {
        return Ok(crate::AudioDeviceDescriptor {
            name: device_name.to_string(),
            description: device_name.to_string(),
            capability_sets: Vec::new(),
        });
    }

    let samplerates: Vec<crate::SamplerateCapability> = supported_rates
        .iter()
        .map(|&rate| crate::SamplerateCapability {
            samplerate: rate as usize,
            formats: vec![fmt_str.clone()],
        })
        .collect();

    let capabilities = vec![crate::ChannelCapability {
        channels,
        samplerates,
    }];

    Ok(crate::AudioDeviceDescriptor {
        name: device_name.to_string(),
        description: device_name.to_string(),
        capability_sets: vec![crate::DeviceCapabilitySet {
            mode: crate::CapabilityMode::Unified,
            capabilities,
        }],
    })
}

// ---------------------------------------------------------------------------
// Helper: number of capture frames accounting for resampler
// ---------------------------------------------------------------------------

fn nbr_capture_frames(resampler: &Option<ChunkResampler>, capture_frames: usize) -> usize {
    if let Some(resampl) = &resampler {
        resampl.resampler.input_frames_next()
    } else {
        capture_frames
    }
}

// ---------------------------------------------------------------------------
// PlaybackDevice trait implementation
// ---------------------------------------------------------------------------

impl PlaybackDevice for AsioPlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let configured_format = self.sample_format;
        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let mut adjust = self.adjust_period > 0.0 && self.enable_rate_adjust;
        let full_duplex = self.full_duplex;
        if adjust && full_duplex {
            warn!("Rate adjust is not supported in full-duplex ASIO mode. Disabling rate adjust.");
            adjust = false;
        }

        let handle = thread::Builder::new()
            .name("AsioPlayback".to_string())
            .spawn(move || {
                // This thread calls into the driver COM object and may be the one that
                // drops it, so give it an apartment of its own.
                com_init_this_thread();

                let channel_capacity = 8 * 1024 / chunksize + 3;
                debug!("Using a playback channel capacity of {channel_capacity} chunks.");
                let (_tx_dev, _rx_dev) = bounded::<usize>(channel_capacity);

                let buffer_fill = Arc::new(Mutex::new(
                    countertimer::DeviceBufferEstimator::new(samplerate),
                ));
                let buffer_fill_clone = buffer_fill.clone();
                let mut buffer_avg = countertimer::Averager::new();
                let mut timer = countertimer::Stopwatch::new();
                let mut chunk_stats = ChunkStats {
                    rms: vec![0.0; channels],
                    peak: vec![0.0; channels],
                };
                let mut rms_values = Vec::new();
                let mut peak_values = Vec::new();

                let mut rate_controller = PIRateController::new_with_default_gains(
                    samplerate,
                    adjust_period as f64,
                    target_level,
                );

                // --- Device-specific setup (full-duplex vs single-direction) ---
                // Format is resolved inside; bytes_per_sample depends on it.
                let setup_result: Result<(usize, BinarySampleFormat, usize), String> = if full_duplex {
                    // Full-duplex: shared driver coordination
                    let (_inputs, outputs, preferred_buf) = match init_shared_asio(&devname, samplerate) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO playback open error: {err}");
                            error!("{msg}");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg.clone()))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    if channels > outputs as usize {
                        let msg = format!(
                            "Requested {channels} output channels but device only has {outputs}"
                        );
                        error!("{msg}");
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                    // Resolve sample format from device
                    let resolved_format = match resolve_format(&devname, &configured_format, false)
                    {
                        Ok(fmt) => fmt,
                        Err(err) => {
                            let msg = format!("ASIO playback format error: {err}");
                            error!("{msg}");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    let binary_format = resolve_binary_format(&resolved_format);
                    let bytes_per_sample = binary_format.bytes_per_sample();
                    let asio_buffer_size = preferred_buf as usize;
                    Ok((asio_buffer_size, binary_format, bytes_per_sample))
                } else {
                    // Single-direction: open device (also resolves format)
                    let resolved_format =
                        match open_asio_playback(&devname, channels, samplerate, &configured_format) {
                            Ok(result) => result,
                            Err(err) => {
                                let msg = format!("ASIO playback open error: {err}");
                                error!("{msg}");
                                status_channel
                                    .send(StatusMessage::PlaybackError(msg))
                                    .unwrap_or(());
                                barrier.wait();
                                return;
                            }
                        };
                    let binary_format = resolve_binary_format(&resolved_format);
                    let bytes_per_sample = binary_format.bytes_per_sample();
                    // Query the driver's actual buffer size now so the ring buffer below
                    // can be sized to fit it (see issue #498: a driver buffer larger than
                    // chunksize used to overflow the ring buffer capacity and cause underruns).
                    let preferred_buf = match get_preferred_buffer_size(&devname) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO playback buffer size query error: {err}");
                            error!("{msg}");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    Ok((preferred_buf as usize, binary_format, bytes_per_sample))
                };

                let (asio_buffer_size, binary_format, bytes_per_sample) = match setup_result {
                    Ok(result) => result,
                    Err(msg) => {
                        error!("{msg}");
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                // Size the ring buffer to fit at least the driver's actual buffer size,
                // not just chunksize, so a single ASIO callback can never request more
                // bytes than the ring buffer can hold.
                let ring_frames = chunksize.max(asio_buffer_size);
                let ringbuffer = HeapRb::<u8>::new(
                    channels * bytes_per_sample * (2 * ring_frames + 2048),
                );
                let (mut device_producer, device_consumer) = ringbuffer.split();
                let mut _single_playback_callbacks: Option<Box<Callbacks>> = None;

                // --- Create context and start ASIO ---
                clear_playback_rate_change_event();
                reset_playback_callback_seen();
                let ctx_raw = if full_duplex {
                    let ctx = Box::new(AsioPlaybackContext {
                        device_consumer,
                        sample_queue: VecDeque::with_capacity(
                            (16 * ring_frames + target_level) * bytes_per_sample * channels,
                        ),
                        // Filled in by register_and_wait once both sides have registered.
                        channel_buffers: Vec::new(),
                        num_channels: channels,
                        buffer_size: asio_buffer_size,
                        bytes_per_sample,
                        read_tmp: vec![0u8; asio_buffer_size * bytes_per_sample * channels],
                        target_level,
                        buffer_fill: buffer_fill_clone,
                        running: false,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    PLAYBACK_CONTEXT.store(ctx_raw, Ordering::Release);

                    // Register and wait for combined buffer creation + ASIOStart
                    if let Err(err) = register_and_wait(false, channels) {
                        let msg = format!("ASIO full-duplex playback setup error: {err}");
                        error!("{msg}");
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                        let _ = unsafe { Box::from_raw(ctx_raw) };
                        barrier.wait();
                        return;
                    }
                    ctx_raw
                } else {
                    let preferred_buf = asio_buffer_size as i32;

                    let driver_channels = make_channel_ids(channels, false);
                    let callbacks_for_driver = Box::new(make_callbacks(
                        buffer_switch_playback,
                        buffer_switch_timeinfo_playback,
                        sample_rate_changed_callback,
                    ));

                    // SAFETY: the callbacks are kept alive in `_single_playback_callbacks`
                    // for as long as the stream runs.
                    let channel_buffers = match unsafe {
                        create_asio_buffers(
                            &devname,
                            &driver_channels,
                            preferred_buf,
                            callbacks_for_driver.as_ref(),
                        )
                    } {
                        Ok(buffers) => buffers,
                        Err(err) => {
                            let msg = format!("ASIO playback create buffers error: {err}");
                            error!("{msg}");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };

                    let ctx = Box::new(AsioPlaybackContext {
                        device_consumer,
                        sample_queue: VecDeque::with_capacity(
                            (16 * ring_frames + target_level) * bytes_per_sample * channels,
                        ),
                        channel_buffers,
                        num_channels: channels,
                        buffer_size: preferred_buf as usize,
                        bytes_per_sample,
                        read_tmp: vec![
                            0u8;
                            (preferred_buf as usize) * bytes_per_sample * channels
                        ],
                        target_level,
                        buffer_fill: buffer_fill_clone,
                        running: false,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    PLAYBACK_CONTEXT.store(ctx_raw, Ordering::Release);

                    log_asio_latencies(&devname);

                    trace!("Playback: starting the stream");
                    if let Err(err) = start_asio_stream(&devname) {
                        let msg = format!("Failed to start ASIO stream: {err}");
                        error!("{msg}");
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                        let _ = unsafe { Box::from_raw(ctx_raw) };
                        barrier.wait();
                        return;
                    }
                    trace!("Playback: stream started");
                    _single_playback_callbacks = Some(callbacks_for_driver);
                    ctx_raw
                };

                match status_channel.send(StatusMessage::PlaybackReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }

                let mut buf =
                    vec![0u8; channels * chunksize * bytes_per_sample];

                debug!("Playback device ready and waiting.");
                let got_callback =
                    wait_for_playback_callback(std::time::Duration::from_millis(500));
                trace!(
                    "Playback startup callback gate: first_callback_received={}",
                    got_callback
                );
                barrier.wait();
                debug!("Playback device starts now!");

                let mut conversion_result;
                'deviceloop: loop {
                    if take_playback_rate_change_event() {
                        let new_rate = read_current_asio_sample_rate_hz(&devname).unwrap_or(0);
                        warn!(
                            "Playback sample rate change detected via callback: {} Hz. Stopping playback.",
                            new_rate
                        );
                        status_channel
                            .send(StatusMessage::PlaybackFormatChange(new_rate))
                            .unwrap_or(());
                        break 'deviceloop;
                    }

                    match channel.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(AudioMessage::Audio(chunk)) => {
                            let estimated_buffer_fill = buffer_fill
                                .try_lock()
                                .map(|b| b.estimate() as f64)
                                .unwrap_or_default();
                            buffer_avg.add_value(
                                estimated_buffer_fill + (channel.len() * chunksize) as f64,
                            );

                            if adjust
                                && timer.larger_than_millis((1000.0 * adjust_period) as u64)
                                && let Some(av_delay) = buffer_avg.average()
                            {
                                let speed = rate_controller.next(av_delay);
                                timer.restart();
                                buffer_avg.restart();
                                debug!(
                                    "Playback, current buffer level {:.1}, set capture rate to {:.4}%.",
                                    av_delay,
                                    100.0 * speed
                                );
                                status_channel
                                    .send(StatusMessage::SetSpeed(speed))
                                    .unwrap_or(());
                                if let Some(mut playback_status) = playback_status.try_write() {
                                    playback_status.buffer_level = av_delay as usize;
                                } else {
                                    xtrace!("playback status blocked, skip buffer level update");
                                }
                            }

                            chunk.update_stats(&mut chunk_stats);
                            crate::push_playback_audio_buffer(&playback_status, &chunk);
                            conversion_result =
                                chunk_to_buffer_rawbytes(chunk, &mut buf, &binary_format);
                            crate::update_playback_signal_status(
                                &playback_status,
                                &chunk_stats,
                                &mut rms_values,
                                &mut peak_values,
                                conversion_result.1,
                            );

                            // Wait for enough space in the ring buffer before pushing.
                            // This is essential when the capture side is not rate-limited
                            // (e.g. signal generator): without this wait the data would
                            // arrive far faster than the ASIO callback can drain it and
                            // most of it would be dropped.  The sleep duration is based
                            // on the time it takes to play back one chunksize.
                            let bytes_to_write = conversion_result.0;
                            let sleep_duration = std::time::Duration::from_micros(
                                (1_000_000 * chunksize / samplerate / 2) as u64
                            );
                            let max_retries = 8;
                            for _ in 0..max_retries {
                                if device_producer.vacant_len() >= bytes_to_write {
                                    break;
                                }
                                std::thread::sleep(sleep_duration);
                            }
                            let pushed_bytes =
                                device_producer.push_slice(&buf[0..bytes_to_write]);
                            if pushed_bytes < bytes_to_write {
                                debug!(
                                    "Playback ring buffer is full, dropped {} out of {} bytes.",
                                    bytes_to_write - pushed_bytes,
                                    bytes_to_write
                                );
                            }
                        }
                        Ok(AudioMessage::Pause) => {
                            trace!("Playback, pause message received.");
                        }
                        Ok(AudioMessage::EndOfStream) => {
                            status_channel
                                .send(StatusMessage::PlaybackDone)
                                .unwrap_or(());
                            break 'deviceloop;
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            let msg = "Playback message channel disconnected".to_string();
                            error!("{msg}.");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg))
                                .unwrap_or(());
                            break 'deviceloop;
                        }
                    }
                }

                // Stop ASIO and clean up.
                // In full-duplex mode, release_shared_asio() must be called BEFORE
                // nullifying the context, because the last side to exit stops the
                // stream, which waits for any in-flight callback to finish.
                // Only after that is it safe to free the context.
                debug!("Stopping ASIO playback.");
                if full_duplex {
                    release_shared_asio();
                } else {
                    PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    trace!("Playback: stopping the stream, disposing buffers and tearing down");
                    if let Err(err) = stop_asio_stream(&devname) {
                        trace!("Playback cleanup: stop failed: {err}");
                    }
                    if let Err(err) = dispose_asio_buffers(&devname) {
                        trace!("Playback cleanup: dispose failed: {err}");
                    }
                    teardown_asio_driver(&devname);
                }
                // Harmless if already nulled by release_shared_asio
                PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                let _ = unsafe { Box::from_raw(ctx_raw) };
            })?;
        Ok(Box::new(handle))
    }
}

// ---------------------------------------------------------------------------
// CaptureDevice trait implementation
// ---------------------------------------------------------------------------

impl CaptureDevice for AsioCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let configured_format = self.sample_format;
        let resampler_conf = self.resampler_config;
        let async_src = resampler_is_async(&resampler_conf);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = (1000.0 * self.rate_measure_interval) as u64;
        let full_duplex = self.full_duplex;

        let handle = thread::Builder::new()
            .name("AsioCapture".to_string())
            .spawn(move || {
                // This thread calls into the driver COM object and may be the one that
                // drops it, so give it an apartment of its own.
                com_init_this_thread();

                let mut resampler = new_resampler(
                    &resampler_conf,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                    processing_params.clone(),
                );

                let channel_capacity = if let Some(resamp) = &resampler {
                    let max_input_frames = resamp.resampler.input_frames_max();
                    32 * (chunksize + max_input_frames) / 1024 + 10
                } else {
                    32 * chunksize / 1024 + 10
                };
                debug!("Using a capture channel capacity of {channel_capacity} buffers.");
                let (tx_dev, rx_dev) = bounded(channel_capacity);

                // --- Device-specific setup (full-duplex vs single-direction) ---
                // Format is resolved inside; bytes_per_sample depends on it.
                let setup_result: Result<(usize, BinarySampleFormat, usize), String> = if full_duplex {
                    // Full-duplex: shared driver coordination
                    let (inputs, _outputs, preferred_buf) = match init_shared_asio(&devname, samplerate) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO capture open error: {err}");
                            error!("{msg}");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel
                                .send(StatusMessage::CaptureError(msg.clone()))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    if channels > inputs as usize {
                        let msg = format!(
                            "Requested {channels} input channels but device only has {inputs}"
                        );
                        error!("{msg}");
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                    // Resolve sample format from device
                    let resolved_format = match resolve_format(&devname, &configured_format, true)
                    {
                        Ok(fmt) => fmt,
                        Err(err) => {
                            let msg = format!("ASIO capture format error: {err}");
                            error!("{msg}");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel
                                .send(StatusMessage::CaptureError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    let binary_format = resolve_binary_format(&resolved_format);
                    let bytes_per_sample = binary_format.bytes_per_sample();
                    Ok((preferred_buf as usize, binary_format, bytes_per_sample))
                } else {
                    // Single-direction: open device (also resolves format)
                    let resolved_format =
                        match open_asio_capture(&devname, channels, samplerate, &configured_format) {
                            Ok(result) => result,
                            Err(err) => {
                                let msg = format!("ASIO capture open error: {err}");
                                error!("{msg}");
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                status_channel
                                    .send(StatusMessage::CaptureError(msg))
                                    .unwrap_or(());
                                barrier.wait();
                                return;
                            }
                        };
                    let binary_format = resolve_binary_format(&resolved_format);
                    let bytes_per_sample = binary_format.bytes_per_sample();
                    // Query the driver's actual buffer size now so the ring buffer below
                    // can be sized to fit it (see issue #498).
                    let preferred_buf = match get_preferred_buffer_size(&devname) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO capture buffer size query error: {err}");
                            error!("{msg}");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel
                                .send(StatusMessage::CaptureError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };
                    Ok((preferred_buf as usize, binary_format, bytes_per_sample))
                };

                let (asio_buffer_size, binary_format, bytes_per_sample) = match setup_result {
                    Ok(result) => result,
                    Err(msg) => {
                        error!("{msg}");
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                let blockalign = bytes_per_sample * channels;
                let buffer_capacity_frames = if let Some(resamp) = &resampler {
                    resamp.resampler.input_frames_max()
                } else {
                    chunksize
                };
                // Size the ring buffer to fit at least the driver's actual buffer size,
                // not just chunksize/resampler input, so a single ASIO callback can
                // never push more bytes than the ring buffer can hold (see issue #498).
                let ring_frames = buffer_capacity_frames.max(asio_buffer_size);
                let ringbuffer = HeapRb::<u8>::new(blockalign * (2 * ring_frames + 2048));
                let (device_producer, mut device_consumer) = ringbuffer.split();
                let mut _single_capture_callbacks: Option<Box<Callbacks>> = None;

                // --- Create context and start ASIO ---
                clear_capture_rate_change_event();
                // Keep the callback from pushing until the loop below is ready to consume.
                CAPTURE_STREAM_ACTIVE.store(false, Ordering::Release);
                let ctx_raw = if full_duplex {
                    let ctx = Box::new(AsioCaptureContext {
                        device_producer,
                        tx_dev,
                        // Filled in by register_and_wait once both sides have registered.
                        channel_buffers: Vec::new(),
                        num_channels: channels,
                        buffer_size: asio_buffer_size,
                        bytes_per_sample,
                        interleaved_tmp: vec![0u8; asio_buffer_size * bytes_per_sample * channels],
                        chunk_counter: 0,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    CAPTURE_CONTEXT.store(ctx_raw, Ordering::Release);

                    // Register and wait for combined buffer creation + ASIOStart
                    if let Err(err) = register_and_wait(true, channels) {
                        let msg = format!("ASIO full-duplex capture setup error: {err}");
                        error!("{msg}");
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                        let _ = unsafe { Box::from_raw(ctx_raw) };
                        barrier.wait();
                        return;
                    }
                    ctx_raw
                } else {
                    let preferred_buf = asio_buffer_size as i32;

                    let driver_channels = make_channel_ids(channels, true);
                    let callbacks_for_driver = Box::new(make_callbacks(
                        buffer_switch_capture,
                        buffer_switch_timeinfo_capture,
                        sample_rate_changed_callback,
                    ));

                    // SAFETY: the callbacks are kept alive in `_single_capture_callbacks`
                    // for as long as the stream runs.
                    let channel_buffers = match unsafe {
                        create_asio_buffers(
                            &devname,
                            &driver_channels,
                            preferred_buf,
                            callbacks_for_driver.as_ref(),
                        )
                    } {
                        Ok(buffers) => buffers,
                        Err(err) => {
                            let msg = format!("ASIO capture create buffers error: {err}");
                            error!("{msg}");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel
                                .send(StatusMessage::CaptureError(msg))
                                .unwrap_or(());
                            barrier.wait();
                            return;
                        }
                    };

                    let ctx = Box::new(AsioCaptureContext {
                        device_producer,
                        tx_dev,
                        channel_buffers,
                        num_channels: channels,
                        buffer_size: preferred_buf as usize,
                        bytes_per_sample,
                        interleaved_tmp: vec![
                            0u8;
                            (preferred_buf as usize) * bytes_per_sample * channels
                        ],
                        chunk_counter: 0,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    CAPTURE_CONTEXT.store(ctx_raw, Ordering::Release);

                    trace!("Capture: starting the stream");
                    if let Err(err) = start_asio_stream(&devname) {
                        let msg = format!("Failed to start ASIO stream: {err}");
                        error!("{msg}");
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                        let _ = unsafe { Box::from_raw(ctx_raw) };
                        barrier.wait();
                        return;
                    }
                    trace!("Capture: stream started");
                    _single_capture_callbacks = Some(callbacks_for_driver);
                    ctx_raw
                };

                // Capture monitoring state
                let mut capture_frames = chunksize;
                let mut averager = countertimer::TimeAverage::new();
                let mut watcher_averager = countertimer::TimeAverage::new();
                let mut valuewatcher = countertimer::ValueWatcher::new(
                    capture_samplerate as f32,
                    RATE_CHANGE_THRESHOLD_VALUE,
                    RATE_CHANGE_THRESHOLD_COUNT,
                );
                let mut value_range = 0.0;
                let mut chunk_stats = ChunkStats {
                    rms: vec![0.0; channels],
                    peak: vec![0.0; channels],
                };
                let mut rms_values = Vec::new();
                let mut peak_values = Vec::new();
                let mut rate_adjust = 0.0;
                // Sample rate measured over the last completed `rate_measure_interval` window,
                // kept separate from the short update cadence.
                let mut measured_rate = 0.0;
                let mut silence_counter = countertimer::SilenceCounter::new(
                    silence_threshold,
                    silence_timeout,
                    capture_samplerate,
                    chunksize,
                );
                let mut state = ProcessingState::Running;
                let mut data_buffer = vec![0u8; 4 * blockalign * capture_frames];
                let mut expected_chunk_nbr: u64 = 0;

                debug!("Capture device ready and waiting.");
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();

                // Discard anything the callback managed to queue before the gate closed, then
                // let it start pushing. Ordering matters: the callback only pushes once the
                // flag is set, so nothing can arrive between the drain and the store.
                let discarded = device_consumer.occupied_len();
                if discarded > 0 {
                    debug!("Discarding {discarded} bytes captured before the loop was ready.");
                    device_consumer.clear();
                }
                CAPTURE_STREAM_ACTIVE.store(true, Ordering::Release);

                debug!("Capture device starts now!");

                'deviceloop: loop {
                    if take_capture_rate_change_event() {
                        let new_rate = read_current_asio_sample_rate_hz(&devname).unwrap_or(0);
                        warn!(
                            "Capture sample rate change detected via callback: {} Hz. Stopping capture.",
                            new_rate
                        );
                        channel
                            .send(AudioMessage::EndOfStream)
                            .unwrap_or(());
                        status_channel
                            .send(StatusMessage::CaptureFormatChange(new_rate))
                            .unwrap_or(());
                        break 'deviceloop;
                    }

                    // Handle commands
                    match command_channel.try_recv() {
                        Ok(CommandMessage::Exit) => {
                            debug!("Exit message received, sending EndOfStream.");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel
                                .send(StatusMessage::CaptureDone)
                                .unwrap_or(());
                            break 'deviceloop;
                        }
                        Ok(CommandMessage::SetSpeed { speed }) => {
                            rate_adjust = speed;
                            debug!("Requested to adjust capture speed to {speed}.");
                            if let Some(resampl) = &mut resampler {
                                debug!("Adjusting resampler rate to {speed}.");
                                if async_src {
                                    if resampl
                                        .set_resample_ratio_relative(speed, true)
                                        .is_err()
                                    {
                                        debug!(
                                            "Failed to set resampling speed to {speed}."
                                        );
                                    }
                                } else {
                                    warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                }
                            }
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => {}
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            error!("Command channel was closed.");
                            break 'deviceloop;
                        }
                    }

                    // Determine how many frames to capture
                    capture_frames = nbr_capture_frames(&resampler, capture_frames);
                    let capture_bytes = blockalign * capture_frames;

                    // Ensure data_buffer is large enough
                    if data_buffer.len() < capture_bytes {
                        data_buffer.resize(capture_bytes, 0);
                    }

                    // Wait for enough data in the ring buffer
                    while device_consumer.occupied_len() < capture_bytes {
                        match rx_dev.recv_timeout(std::time::Duration::from_millis(250)) {
                            Ok((chunk_nbr, data_bytes)) => {
                                trace!(
                                    "Capture, received notification, length {data_bytes} bytes."
                                );
                                expected_chunk_nbr += 1;
                                if chunk_nbr > expected_chunk_nbr {
                                    warn!(
                                        "Capture, samples were dropped, missing {} buffers.",
                                        chunk_nbr - expected_chunk_nbr
                                    );
                                    expected_chunk_nbr = chunk_nbr;
                                }
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                                warn!("Capture, waiting for data timed out.");
                                break;
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                                error!("Capture, channel is closed.");
                                channel
                                    .send(AudioMessage::EndOfStream)
                                    .unwrap_or(());
                                status_channel
                                    .send(StatusMessage::CaptureError(
                                        "Capture notification channel closed".to_string(),
                                    ))
                                    .unwrap_or(());
                                break 'deviceloop;
                            }
                        }
                    }

                    // Read data from ring buffer
                    device_consumer.pop_slice(&mut data_buffer[0..capture_bytes]);

                    // Measure sample rate
                    averager.add_value(capture_frames);
                    if let Some(capture_status) = capture_status.try_upgradable_read() {
                        if averager
                            .larger_than_millis(capture_status.update_interval as u64)
                        {
                            averager.restart();
                            if let Ok(mut capture_status) =
                                RwLockUpgradableReadGuard::try_upgrade(capture_status)
                            {
                                capture_status.measured_samplerate =
                                    measured_rate as usize;
                                capture_status.signal_range = value_range.to_f32();
                                capture_status.rate_adjust = rate_adjust as f32;
                                crate::update_capture_state(&mut capture_status, state);
                            } else {
                                xtrace!("Capture status upgrade blocked, skip update.");
                            }
                        }
                    } else {
                        xtrace!("Capture status blocked, skip update.");
                    }

                    // Rate change detection
                    watcher_averager.add_value(capture_frames);
                    if watcher_averager.larger_than_millis(rate_measure_interval) {
                        let samples_per_sec = watcher_averager.average();
                        watcher_averager.restart();
                        let measured_rate_f = samples_per_sec;
                        measured_rate = measured_rate_f;
                        debug!(
                            "Capture, measured sample rate is {measured_rate_f:.1} Hz."
                        );
                        let changed =
                            valuewatcher.check_value(measured_rate_f as f32);
                        if changed {
                            warn!(
                                "Sample rate change detected, last rate was {measured_rate_f} Hz."
                            );
                            if stop_on_rate_change {
                                channel
                                    .send(AudioMessage::EndOfStream)
                                    .unwrap_or(());
                                status_channel
                                    .send(StatusMessage::CaptureFormatChange(
                                        measured_rate_f as usize,
                                    ))
                                    .unwrap_or(());
                                break 'deviceloop;
                            }
                        }
                    }

                    // Convert buffer to audio chunk
                    let mut chunk = buffer_to_chunk_rawbytes(
                        &data_buffer[0..capture_bytes],
                        channels,
                        &binary_format,
                        capture_bytes,
                        &capture_status.read().used_channels,
                        false,
                    );

                    // Signal statistics
                    chunk.update_stats(&mut chunk_stats);
                    crate::push_capture_audio_buffer(&capture_status, &chunk);
                    crate::update_capture_signal_status(
                        &capture_status,
                        &chunk_stats,
                        &mut rms_values,
                        &mut peak_values,
                    );

                    // Silence detection
                    value_range = chunk.maxval - chunk.minval;
                    state = silence_counter.update(value_range);
                    if state == ProcessingState::Running {
                        if let Some(resampl) = &mut resampler {
                            resampl.resample_chunk(&mut chunk, chunksize, channels);
                        }
                        let msg = AudioMessage::Audio(chunk);
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break 'deviceloop;
                        }
                    } else if state == ProcessingState::Paused {
                        let msg = AudioMessage::Pause;
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break 'deviceloop;
                        }
                    }
                }

                // Close the gate first, so callbacks arriving during teardown do no work.
                CAPTURE_STREAM_ACTIVE.store(false, Ordering::Release);

                // Stop ASIO and clean up.
                // In full-duplex mode, release_shared_asio() must be called BEFORE
                // nullifying the context, because the last side to exit stops the
                // stream, which waits for any in-flight callback to finish.
                // Only after that is it safe to free the context.
                debug!("Stopping ASIO capture.");
                if full_duplex {
                    release_shared_asio();
                } else {
                    CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    trace!("Capture: stopping the stream, disposing buffers and tearing down");
                    if let Err(err) = stop_asio_stream(&devname) {
                        trace!("Capture cleanup: stop failed: {err}");
                    }
                    if let Err(err) = dispose_asio_buffers(&devname) {
                        trace!("Capture cleanup: dispose failed: {err}");
                    }
                    teardown_asio_driver(&devname);
                }
                // Harmless if already nulled by release_shared_asio
                CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                let _ = unsafe { Box::from_raw(ctx_raw) };
                crate::set_capture_state(&capture_status, ProcessingState::Inactive);
            })?;
        Ok(Box::new(handle))
    }
}
