// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2025 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// This file is licensed under the GNU General Public License version 3 only.
// It links against the ASIO SDK, which is licensed under GPLv3.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

// ASIO backend for playback and capture.
// This implementation uses the asio-sys crate to interface with the ASIO driver system.

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex, OnceLock};
use std::thread;

use crossbeam_channel::{TrySendError, bounded};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use ringbuf::{HeapRb, traits::*};

use asio_sys::bindings::asio_import::{
    ASIOBufferInfo, ASIOCallbacks, ASIOChannelInfo, ASIOCreateBuffers, ASIODisposeBuffers,
    ASIOExit, ASIOGetBufferSize, ASIOGetChannelInfo, ASIOGetChannels, ASIOStart, ASIOStop,
    ASIOTime,
};
use asio_sys::{Asio, AsioSampleType, Driver};

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::audiodevice::*;
use crate::config::{AsioSampleFormat, BinarySampleFormat, ConfigError};
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use crate::helpers::PIRateController;
use crate::resampling::{ChunkResampler, new_resampler, resampler_is_async};
use crate::{CaptureStatus, PlaybackStatus};

// ---------------------------------------------------------------------------
// Public device structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AsioPlaybackDevice {
    pub devname: Option<String>,
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
    pub devname: Option<String>,
    pub samplerate: usize,
    pub capture_samplerate: usize,
    pub resampler_config: Option<crate::config::Resampler>,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: Option<AsioSampleFormat>,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
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
    buffer_infos: Vec<ASIOBufferInfo>,
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
    starting: bool,
}

/// Context passed to the ASIO capture callback via a global AtomicPtr.
struct AsioCaptureContext {
    /// Ring buffer producer — writes bytes read from hardware.
    device_producer: ringbuf::wrap::caching::Caching<Arc<HeapRb<u8>>, true, false>,
    /// Notification channel: (chunk_counter, pushed_bytes).
    tx_dev: crossbeam_channel::Sender<(u64, usize)>,
    buffer_infos: Vec<ASIOBufferInfo>,
    num_channels: usize,
    buffer_size: usize,
    bytes_per_sample: usize,
    /// Preallocated interleaved capture buffer reused by callback.
    interleaved_tmp: Vec<u8>,
    chunk_counter: u64,
}

static PLAYBACK_CONTEXT: AtomicPtr<AsioPlaybackContext> = AtomicPtr::new(ptr::null_mut());
static CAPTURE_CONTEXT: AtomicPtr<AsioCaptureContext> = AtomicPtr::new(ptr::null_mut());
static ASIO_WRAPPER_DRIVER: OnceLock<Mutex<Option<Driver>>> = OnceLock::new();

fn wrapper_driver_lock() -> &'static Mutex<Option<Driver>> {
    ASIO_WRAPPER_DRIVER.get_or_init(|| Mutex::new(None))
}

fn loaded_wrapper_driver() -> Result<Driver, ConfigError> {
    let guard = wrapper_driver_lock().lock().unwrap();
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| ConfigError::new("ASIO driver is not loaded"))
}

fn drop_wrapper_driver() {
    let mut guard = wrapper_driver_lock().lock().unwrap();
    let _ = guard.take();
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
    /// Pending output (playback) buffer registration: (buffer_infos, num_channels).
    pending_output: Option<(Vec<ASIOBufferInfo>, usize)>,
    /// Pending input (capture) buffer registration: (buffer_infos, num_channels).
    pending_input: Option<(Vec<ASIOBufferInfo>, usize)>,
    /// Whether the ASIO stream has been started.
    stream_started: bool,
    /// Number of sides (playback/capture) still active. Last one to exit calls ASIOStop.
    active_count: u8,
    /// The original `ASIOBufferInfo` array passed to `ASIOCreateBuffers`.
    /// The ASIO SDK requires this array to remain valid for the lifetime of the stream.
    buffer_infos_for_driver: Vec<ASIOBufferInfo>,
    /// The `ASIOCallbacks` struct passed to `ASIOCreateBuffers`.
    /// The ASIO SDK requires this struct to remain valid for the lifetime of the stream.
    callbacks_for_driver: Option<Box<ASIOCallbacks>>,
}

// SAFETY: AsioSharedState is only accessed under a Mutex lock.
// The raw pointers in ASIOBufferInfo are transient (used during setup only)
// and never dereferenced outside of ASIO callback context.
unsafe impl Send for AsioSharedState {}

static ASIO_SHARED: OnceLock<(Mutex<Option<AsioSharedState>>, Condvar)> = OnceLock::new();

fn copy_from_queue_at_offset(queue: &VecDeque<u8>, offset: usize, dst: &mut [u8]) {
    let (head, tail) = queue.as_slices();
    if offset < head.len() {
        let first = (head.len() - offset).min(dst.len());
        dst[..first].copy_from_slice(&head[offset..offset + first]);
        if first < dst.len() {
            let remaining = dst.len() - first;
            dst[first..].copy_from_slice(&tail[..remaining]);
        }
    } else {
        let tail_offset = offset - head.len();
        dst.copy_from_slice(&tail[tail_offset..tail_offset + dst.len()]);
    }
}

// ---------------------------------------------------------------------------
// ASIO callbacks  (unsafe extern "C" — called from ASIO driver thread)
// ---------------------------------------------------------------------------

/// ASIO bufferSwitch callback for playback.
/// Reads converted audio bytes from the ring buffer and copies them into the ASIO output buffers.
///
/// # Safety
/// Called from the ASIO driver thread. The caller must ensure that `PLAYBACK_CONTEXT`
/// points to a valid `AsioPlaybackContext` or is null.
pub unsafe extern "C" fn buffer_switch_playback(buffer_index: i32, _direct_process: i32) {
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
    if ctx.buffer_infos.len() < ctx.num_channels {
        error!(
            "ASIO playback callback buffer info mismatch: infos={}, channels={}",
            ctx.buffer_infos.len(),
            ctx.num_channels
        );
        return;
    }
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
            let prefill_frames = if ctx.starting { 0 } else { ctx.target_level };
            ctx.starting = false;
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
            let buffer_info = &ctx.buffer_infos[ch];
            let out_ptr = buffer_info.buffers[buffer_index];
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
pub unsafe extern "C" fn buffer_switch_capture(buffer_index: i32, _direct_process: i32) {
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
    if ctx.buffer_infos.len() < ctx.num_channels {
        error!(
            "ASIO capture callback buffer info mismatch: infos={}, channels={}",
            ctx.buffer_infos.len(),
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
            let buffer_info = &ctx.buffer_infos[ch];
            let in_ptr = buffer_info.buffers[buffer_index];
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
    let pushed_bytes = ctx.device_producer.push_slice(&buf);
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
pub unsafe extern "C" fn buffer_switch_timeinfo_playback(
    params: *mut ASIOTime,
    buffer_index: i32,
    direct_process: i32,
) -> *mut ASIOTime {
    unsafe {
        buffer_switch_playback(buffer_index, direct_process);
    }
    params
}

/// ASIO bufferSwitchTimeInfo callback for capture.
///
/// # Safety
/// Called from the ASIO driver thread. `params` is provided by the driver.
pub unsafe extern "C" fn buffer_switch_timeinfo_capture(
    params: *mut ASIOTime,
    buffer_index: i32,
    direct_process: i32,
) -> *mut ASIOTime {
    unsafe {
        buffer_switch_capture(buffer_index, direct_process);
    }
    params
}

/// ASIO bufferSwitchTimeInfo callback for full-duplex mode.
///
/// # Safety
/// Called from the ASIO driver thread. `params` is provided by the driver.
pub unsafe extern "C" fn buffer_switch_timeinfo_combined(
    params: *mut ASIOTime,
    buffer_index: i32,
    direct_process: i32,
) -> *mut ASIOTime {
    unsafe {
        buffer_switch_combined(buffer_index, direct_process);
    }
    params
}

/// ASIO asioMessage callback.
/// Handles driver queries about supported features.
/// Returning 0 means "not supported" or "no" for most selectors.
///
/// # Safety
/// Called from the ASIO driver thread. All parameters are provided by the driver.
pub unsafe extern "C" fn asio_message_callback(
    selector: i32,
    value: i32,
    _message: *mut std::os::raw::c_void,
    _opt: *mut f64,
) -> i32 {
    // Standard ASIO message selectors:
    const K_ASIO_SELECTOR_SUPPORTED: i32 = 1;
    const K_ASIO_ENGINE_VERSION: i32 = 2;
    const K_ASIO_SUPPORTS_TIME_INFO: i32 = 3;
    const K_ASIO_SUPPORTS_TIME_CODE: i32 = 4;
    // Reset/resync request selectors:
    const K_ASIO_RESET_REQUEST: i32 = 5;
    const K_ASIO_BUFFER_SIZE_CHANGE: i32 = 6;
    const K_ASIO_RESYNC_REQUEST: i32 = 7;
    const K_ASIO_LATENCIES_CHANGED: i32 = 8;

    match selector {
        K_ASIO_SELECTOR_SUPPORTED => {
            // The driver asks if we support a given selector.
            match value {
                K_ASIO_ENGINE_VERSION
                | K_ASIO_RESYNC_REQUEST
                | K_ASIO_LATENCIES_CHANGED
                | K_ASIO_RESET_REQUEST
                | K_ASIO_BUFFER_SIZE_CHANGE
                | K_ASIO_SUPPORTS_TIME_INFO
                | K_ASIO_SELECTOR_SUPPORTED => 1, // yes
                K_ASIO_SUPPORTS_TIME_CODE => 0, // no
                _ => 0,
            }
        }
        K_ASIO_ENGINE_VERSION => 2, // ASIO 2.0
        K_ASIO_SUPPORTS_TIME_INFO => 1,
        K_ASIO_RESET_REQUEST => {
            warn!("ASIO reset request received. A stream restart may be required by the driver.");
            1
        }
        K_ASIO_BUFFER_SIZE_CHANGE => {
            warn!(
                "ASIO buffer size change request received. Dynamic resize is not implemented in this backend."
            );
            1
        }
        K_ASIO_RESYNC_REQUEST => {
            debug!("ASIO resync request received.");
            1
        }
        K_ASIO_LATENCIES_CHANGED => {
            debug!("ASIO latencies changed notification.");
            1
        }
        _ => 0,
    }
}

/// Combined ASIO bufferSwitch callback for full-duplex mode.
/// Dispatches to both playback and capture callbacks.
///
/// # Safety
/// Called from the ASIO driver thread. Both `PLAYBACK_CONTEXT` and `CAPTURE_CONTEXT`
/// must point to valid contexts or be null.
pub unsafe extern "C" fn buffer_switch_combined(buffer_index: i32, direct_process: i32) {
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
    let (mutex, _condvar) = ASIO_SHARED.get_or_init(|| (Mutex::new(None), Condvar::new()));
    let mut guard = mutex.lock().unwrap();

    if let Some(ref shared) = *guard {
        // Driver already loaded by the other side
        if shared.driver_name != devname {
            return Err(ConfigError::new(
                "Different ASIO driver names for capture and playback are not supported",
            ));
        }
        Ok((
            shared.num_inputs,
            shared.num_outputs,
            shared.preferred_buf_size,
        ))
    } else {
        // First caller — load and initialise the driver
        let (num_inputs, num_outputs) = open_asio_device(devname, samplerate)?;

        // Query preferred buffer size
        let mut min_buf: i32 = 0;
        let mut max_buf: i32 = 0;
        let mut preferred_buf: i32 = 0;
        let mut granularity: i32 = 0;
        let res = unsafe {
            ASIOGetBufferSize(
                &mut min_buf,
                &mut max_buf,
                &mut preferred_buf,
                &mut granularity,
            )
        };
        if res != 0 {
            return Err(ConfigError::new(&format!(
                "ASIOGetBufferSize failed with error code {res}"
            )));
        }
        debug!(
            "ASIO buffer sizes: min={min_buf}, max={max_buf}, preferred={preferred_buf}, granularity={granularity}."
        );

        *guard = Some(AsioSharedState {
            driver_name: devname.to_string(),
            num_inputs,
            num_outputs,
            preferred_buf_size: preferred_buf,
            pending_output: None,
            pending_input: None,
            stream_started: false,
            active_count: 0,
            buffer_infos_for_driver: Vec::new(),
            callbacks_for_driver: None,
        });

        Ok((num_inputs, num_outputs, preferred_buf))
    }
}

/// Create an `ASIOBufferInfo` array for the given number of channels.
fn make_buffer_infos(num_channels: usize, is_input: bool) -> Vec<ASIOBufferInfo> {
    (0..num_channels)
        .map(|ch| ASIOBufferInfo {
            isInput: if is_input { 1 } else { 0 },
            channelNum: ch as i32,
            buffers: [ptr::null_mut(), ptr::null_mut()],
        })
        .collect()
}

/// Register one side (playback or capture) for full-duplex operation.
///
/// When both sides have registered, the second caller creates the combined ASIO buffers,
/// updates both contexts' `buffer_infos` through the global atomics, and calls `ASIOStart()`.
/// The first caller blocks on a condvar until this is done.
fn register_and_wait(is_input: bool, num_channels: usize) -> Result<(), ConfigError> {
    let (mutex, condvar) = ASIO_SHARED
        .get()
        .expect("ASIO_SHARED must be initialised before register_and_wait");
    let mut guard = mutex.lock().unwrap();

    // Register our buffer infos
    {
        let shared = guard.as_mut().expect("shared state must exist");
        let infos = make_buffer_infos(num_channels, is_input);
        trace!(
            "ASIO register side: is_input={}, num_channels={}, stream_started={}, active_count={}",
            is_input, num_channels, shared.stream_started, shared.active_count
        );
        if is_input {
            shared.pending_input = Some((infos, num_channels));
        } else {
            shared.pending_output = Some((infos, num_channels));
        }
    }

    let both_ready = {
        let shared = guard.as_ref().unwrap();
        shared.pending_input.is_some() && shared.pending_output.is_some()
    };

    if both_ready {
        // I am the second side to register — do combined buffer creation + start.
        let shared = guard.as_mut().unwrap();
        let (out_infos, out_ch) = shared.pending_output.take().unwrap();
        let (in_infos, in_ch) = shared.pending_input.take().unwrap();
        let preferred_buf = shared.preferred_buf_size;
        trace!(
            "ASIO both sides ready: out_ch={}, in_ch={}, preferred_buf={}",
            out_ch, in_ch, preferred_buf
        );

        // Build combined array: outputs first, then inputs.
        let mut combined: Vec<ASIOBufferInfo> = Vec::with_capacity(out_ch + in_ch);
        combined.extend(out_infos);
        combined.extend(in_infos);
        let total_ch = (out_ch + in_ch) as i32;

        // Heap-allocate callbacks so the struct remains at a stable address.
        // The ASIO SDK requires both the ASIOBufferInfo array and ASIOCallbacks
        // struct to remain valid for the lifetime of the stream.
        shared.callbacks_for_driver = Some(Box::new(ASIOCallbacks {
            bufferSwitch: Some(buffer_switch_combined),
            sampleRateDidChange: None,
            asioMessage: Some(asio_message_callback),
            bufferSwitchTimeInfo: Some(buffer_switch_timeinfo_combined),
        }));

        create_asio_buffers(
            &mut combined,
            total_ch,
            preferred_buf,
            shared.callbacks_for_driver.as_mut().unwrap().as_mut(),
        )?;

        // Update both contexts' buffer_infos through the global atomics.
        // Both contexts are guaranteed to be stored before register_and_wait is called.
        let pb_ctx = PLAYBACK_CONTEXT.load(Ordering::Acquire);
        if !pb_ctx.is_null() {
            unsafe {
                (*pb_ctx).buffer_infos = combined[..out_ch].to_vec();
            }
        }
        let cap_ctx = CAPTURE_CONTEXT.load(Ordering::Acquire);
        if !cap_ctx.is_null() {
            unsafe {
                (*cap_ctx).buffer_infos = combined[out_ch..].to_vec();
            }
        }

        // Keep the original buffer_infos array alive for the ASIO driver.
        shared.buffer_infos_for_driver = combined;

        // Start the stream
        let start_res = unsafe { ASIOStart() };
        if start_res != 0 {
            return Err(ConfigError::new(&format!(
                "ASIOStart failed with error code {start_res}"
            )));
        }
        debug!("Full-duplex ASIO stream started.");

        shared.stream_started = true;
        shared.active_count = 2;
        condvar.notify_all();
    } else {
        // I am the first side — wait for the other side to complete setup.
        debug!("Waiting for other ASIO side to register for full-duplex...");
        while !guard.as_ref().unwrap().stream_started {
            guard = condvar.wait(guard).unwrap();
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
            let stop_res = unsafe { ASIOStop() };
            let _ = stop_res;
            trace!("ASIOStop (first side exit) returned {}.", stop_res);
        } else if shared.active_count == 0 {
            // Last side to exit — dispose buffers and the driver.
            // The stream was already stopped by the first side.
            debug!("Last ASIO side exiting, disposing driver.");
            let dispose_res = unsafe { ASIODisposeBuffers() };
            drop_wrapper_driver();
            let exit_res = unsafe { ASIOExit() };
            let _ = (dispose_res, exit_res);
            trace!(
                "ASIODisposeBuffers (last side exit) returned {}, ASIOExit returned {}.",
                dispose_res, exit_res
            );
            *guard = None; // Reset for next session
        }
    }
}

// ---------------------------------------------------------------------------
// ASIO low-level helpers
// ---------------------------------------------------------------------------

/// Resolve ASIO sample format to a BinarySampleFormat.
fn resolve_binary_format(format: &AsioSampleFormat) -> BinarySampleFormat {
    match format {
        AsioSampleFormat::S16_LE => BinarySampleFormat::S16_LE,
        AsioSampleFormat::S24_4_LE => BinarySampleFormat::S24_4_LJ_LE,
        AsioSampleFormat::S24_3_LE => BinarySampleFormat::S24_3_LE,
        AsioSampleFormat::S32_LE => BinarySampleFormat::S32_LE,
        AsioSampleFormat::F32_LE => BinarySampleFormat::F32_LE,
        AsioSampleFormat::F64_LE => BinarySampleFormat::F64_LE,
    }
}

/// Convert a fixed-size C char buffer to String without reading past the buffer.
///
/// Some ASIO drivers may return char arrays without NUL termination; this helper
/// safely truncates at the first NUL if present, otherwise uses the full buffer.
fn fixed_cstr_buf_to_string(buf: &[i8]) -> String {
    let end = buf.iter().position(|&ch| ch == 0).unwrap_or(buf.len());
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, end) };
    String::from_utf8_lossy(bytes).into_owned()
}

// ---------------------------------------------------------------------------
// ASIO sample type constants (from ASIO SDK asio.h)
// ---------------------------------------------------------------------------
const ASIO_ST_INT16_MSB: i32 = 0;
const ASIO_ST_INT24_MSB: i32 = 1; // 3 bytes packed
const ASIO_ST_INT32_MSB: i32 = 2;
const ASIO_ST_FLOAT32_MSB: i32 = 3;
const ASIO_ST_FLOAT64_MSB: i32 = 4;
// 32-bit container MSB variants
const ASIO_ST_INT32_MSB_16: i32 = 8; // 16-bit in 32-bit
const ASIO_ST_INT32_MSB_18: i32 = 9;
const ASIO_ST_INT32_MSB_20: i32 = 10;
const ASIO_ST_INT32_MSB_24: i32 = 11; // 24-bit in 32-bit
// LSB (little-endian) variants
const ASIO_ST_INT16_LSB: i32 = 16;
const ASIO_ST_INT24_LSB: i32 = 17; // 3 bytes packed
const ASIO_ST_INT32_LSB: i32 = 18;
const ASIO_ST_FLOAT32_LSB: i32 = 19;
const ASIO_ST_FLOAT64_LSB: i32 = 20;
// 32-bit container LSB variants
const ASIO_ST_INT32_LSB_16: i32 = 24; // 16-bit in 32-bit
const ASIO_ST_INT32_LSB_18: i32 = 25;
const ASIO_ST_INT32_LSB_20: i32 = 26;
const ASIO_ST_INT32_LSB_24: i32 = 27; // 24-bit in 32-bit
// DSD formats
const ASIO_ST_DSD_INT8_LSB_1: i32 = 32;
const ASIO_ST_DSD_INT8_MSB_1: i32 = 33;
const ASIO_ST_DSD_INT8_NER8: i32 = 40;

/// Return a human-readable name for an ASIO sample type (from `ASIOChannelInfo::type_`).
fn asio_sample_type_name(type_id: i32) -> &'static str {
    match type_id {
        ASIO_ST_INT16_MSB => "Int16 MSB (big-endian)",
        ASIO_ST_INT24_MSB => "Int24 MSB (3-byte packed, big-endian)",
        ASIO_ST_INT32_MSB => "Int32 MSB (big-endian)",
        ASIO_ST_FLOAT32_MSB => "Float32 MSB (big-endian)",
        ASIO_ST_FLOAT64_MSB => "Float64 MSB (big-endian)",
        ASIO_ST_INT32_MSB_16 => "Int32 MSB 16-bit (big-endian)",
        ASIO_ST_INT32_MSB_18 => "Int32 MSB 18-bit (big-endian)",
        ASIO_ST_INT32_MSB_20 => "Int32 MSB 20-bit (big-endian)",
        ASIO_ST_INT32_MSB_24 => "Int32 MSB 24-bit (big-endian)",
        ASIO_ST_INT16_LSB => "Int16 LSB",
        ASIO_ST_INT24_LSB => "Int24 LSB (3-byte packed)",
        ASIO_ST_INT32_LSB => "Int32 LSB",
        ASIO_ST_FLOAT32_LSB => "Float32 LSB",
        ASIO_ST_FLOAT64_LSB => "Float64 LSB",
        ASIO_ST_INT32_LSB_16 => "Int32 LSB 16-bit",
        ASIO_ST_INT32_LSB_18 => "Int32 LSB 18-bit",
        ASIO_ST_INT32_LSB_20 => "Int32 LSB 20-bit",
        ASIO_ST_INT32_LSB_24 => "Int32 LSB 24-bit",
        ASIO_ST_DSD_INT8_LSB_1 => "DSD Int8 LSB 1",
        ASIO_ST_DSD_INT8_MSB_1 => "DSD Int8 MSB 1",
        ASIO_ST_DSD_INT8_NER8 => "DSD Int8 NER8",
        _ => "Unknown",
    }
}

/// Map an ASIO sample type to an `AsioSampleFormat`.
/// Returns `None` for types that CamillaDSP cannot handle (e.g. big-endian, DSD).
fn asio_sample_type_to_format(type_id: i32) -> Option<AsioSampleFormat> {
    match type_id {
        ASIO_ST_INT16_LSB => Some(AsioSampleFormat::S16_LE),
        ASIO_ST_INT24_LSB => Some(AsioSampleFormat::S24_3_LE),
        ASIO_ST_INT32_LSB => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_16 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_18 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_20 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_24 => Some(AsioSampleFormat::S24_4_LE),
        ASIO_ST_FLOAT32_LSB => Some(AsioSampleFormat::F32_LE),
        ASIO_ST_FLOAT64_LSB => Some(AsioSampleFormat::F64_LE),
        _ => None,
    }
}

fn asio_sys_sample_type_to_id(sample_type: AsioSampleType) -> i32 {
    sample_type as i32
}

/// Query the native sample format of channel 0 for the given direction.
/// Must be called after the driver is loaded and initialized.
fn query_device_format(is_input: bool) -> Result<i32, ConfigError> {
    let driver = loaded_wrapper_driver()?;
    let sample_type = if is_input {
        driver
            .input_data_type()
            .map_err(|err| ConfigError::new(&format!("ASIO input_data_type failed: {err:?}")))?
    } else {
        driver
            .output_data_type()
            .map_err(|err| ConfigError::new(&format!("ASIO output_data_type failed: {err:?}")))?
    };
    let type_id = asio_sys_sample_type_to_id(sample_type);
    debug!(
        "ASIO channel 0 ({}): type={} ({})",
        if is_input { "input" } else { "output" },
        type_id,
        asio_sample_type_name(type_id),
    );
    Ok(type_id)
}

/// Resolve the sample format to use for a given direction.
///
/// ASIO drivers do not perform sample format conversion — the application must
/// use the device's native format. This function queries the device for its
/// native sample type and returns the corresponding `AsioSampleFormat`.
///
/// If the user specified a format in the config that differs from the native
/// format, an error is returned. If the format is `None`, auto-detect from the device.
/// Must be called after the driver is loaded and initialized.
fn resolve_format(
    configured: &Option<AsioSampleFormat>,
    is_input: bool,
) -> Result<AsioSampleFormat, ConfigError> {
    let device_type = query_device_format(is_input)?;
    let device_format = asio_sample_type_to_format(device_type);
    let direction = if is_input { "capture" } else { "playback" };

    let native_format = match device_format {
        Some(fmt) => fmt,
        None => {
            return Err(ConfigError::new(&format!(
                "ASIO {direction}: device uses unsupported sample type {} ({})",
                device_type,
                asio_sample_type_name(device_type),
            )));
        }
    };

    if let Some(fmt) = configured {
        if *fmt != native_format {
            return Err(ConfigError::new(&format!(
                "ASIO {direction}: configured format {fmt:?} does not match device native format \
                 {native_format:?} ({}). ASIO drivers do not convert sample formats. \
                 Please remove the format setting to auto-detect, or set it to {native_format:?}",
                asio_sample_type_name(device_type),
            )));
        }
        debug!("ASIO {direction}: configured format {fmt:?} matches device native format.");
    } else {
        debug!("ASIO {direction}: auto-detected format {native_format:?} from device.");
    }

    Ok(native_format)
}

/// Load an ASIO driver by name.
pub fn load_driver_by_name(name: &str) -> Result<(), ConfigError> {
    let host = Asio::new();
    let driver = host.load_driver(name).map_err(|err| {
        ConfigError::new(&format!("Failed to load ASIO driver '{name}': {err:?}"))
    })?;
    let mut guard = wrapper_driver_lock().lock().unwrap();
    let _ = guard.replace(driver);
    Ok(())
}

/// Force an ASIO sample rate change by running a short dummy stream cycle.
///
/// Some ASIO drivers (e.g. Steinberg) only reconfigure the hardware sample rate
/// after a complete buffer-creation cycle.  This helper performs:
///   1. Query channels and buffer sizes
///   2. Create minimal (1-channel) buffers
///   3. Start the stream briefly
///   4. Stop, dispose buffers, and exit the driver
///   5. Re-load and re-initialise the driver
///   6. Set the rate again and verify
///
/// On return the driver is loaded, initialised and running at the requested rate,
/// ready for `ASIOGetChannels` / `ASIOGetBufferSize` / `ASIOCreateBuffers`.
fn force_sample_rate_with_dummy_cycle(devname: &str, rate: f64) -> Result<(), ConfigError> {
    // --- Phase 1: run a dummy stream to force hardware reconfiguration ---
    let mut num_in: i32 = 0;
    let mut num_out: i32 = 0;
    let ch_res = unsafe { ASIOGetChannels(&mut num_in, &mut num_out) };
    if ch_res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOGetChannels failed during rate-change cycle (error code {ch_res})"
        )));
    }

    let mut min_buf: i32 = 0;
    let mut max_buf: i32 = 0;
    let mut preferred_buf: i32 = 0;
    let mut granularity: i32 = 0;
    let buf_res = unsafe {
        ASIOGetBufferSize(
            &mut min_buf,
            &mut max_buf,
            &mut preferred_buf,
            &mut granularity,
        )
    };
    if buf_res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOGetBufferSize failed during rate-change cycle (error code {buf_res})"
        )));
    }

    // Create a minimal set of buffers (one output channel, or one input if no outputs).
    let is_input = num_out == 0;
    let mut dummy_bufs = vec![ASIOBufferInfo {
        isInput: if is_input { 1 } else { 0 },
        channelNum: 0,
        buffers: [ptr::null_mut(), ptr::null_mut()],
    }];

    /// Dummy callback that does nothing — we just need the stream to run briefly.
    ///
    /// # Safety
    ///
    /// Called by the ASIO driver from its audio thread.
    unsafe extern "C" fn dummy_buffer_switch(_double_buffer_index: i32, _direct_process: i32) {}

    /// Dummy time-info callback that forwards to the plain dummy callback.
    ///
    /// # Safety
    ///
    /// Called by the ASIO driver from its audio thread.
    unsafe extern "C" fn dummy_buffer_switch_time_info(
        params: *mut ASIOTime,
        double_buffer_index: i32,
        direct_process: i32,
    ) -> *mut ASIOTime {
        unsafe {
            dummy_buffer_switch(double_buffer_index, direct_process);
        }
        params
    }

    /// Dummy message callback for the short-lived dummy stream.
    ///
    /// # Safety
    ///
    /// Called by the ASIO driver.
    unsafe extern "C" fn dummy_asio_message(
        selector: i32,
        _value: i32,
        _message: *mut std::ffi::c_void,
        _opt: *mut f64,
    ) -> i32 {
        // kAsioSelectorSupported
        if selector == 1 {
            return 1;
        }
        0
    }

    let dummy_callbacks = Box::leak(Box::new(ASIOCallbacks {
        bufferSwitch: Some(dummy_buffer_switch),
        sampleRateDidChange: None,
        asioMessage: Some(dummy_asio_message),
        bufferSwitchTimeInfo: Some(dummy_buffer_switch_time_info),
    }));

    let create_res = unsafe {
        ASIOCreateBuffers(
            dummy_bufs.as_mut_ptr(),
            1,
            preferred_buf,
            dummy_callbacks as *mut ASIOCallbacks,
        )
    };
    if create_res != 0 {
        // Clean up the leaked callbacks before returning
        let _ = unsafe { Box::from_raw(dummy_callbacks) };
        return Err(ConfigError::new(&format!(
            "ASIOCreateBuffers failed during rate-change cycle (error code {create_res})"
        )));
    }

    // Start and immediately stop — just enough to force hardware reconfiguration.
    let start_res = unsafe { ASIOStart() };
    if start_res != 0 {
        let _ = unsafe { ASIODisposeBuffers() };
        let _ = unsafe { Box::from_raw(dummy_callbacks) };
        return Err(ConfigError::new(&format!(
            "ASIOStart failed during rate-change cycle (error code {start_res})"
        )));
    }
    // Give the driver a moment to reconfigure hardware
    std::thread::sleep(std::time::Duration::from_millis(50));
    let _ = unsafe { ASIOStop() };
    let _ = unsafe { ASIODisposeBuffers() };
    let _ = unsafe { Box::from_raw(dummy_callbacks) };

    // --- Phase 2: full teardown and clean re-initialisation ---
    drop_wrapper_driver();
    let _ = unsafe { ASIOExit() };
    load_driver_by_name(devname)?;
    let driver = loaded_wrapper_driver()?;

    // Set the rate on the fresh driver instance
    driver.set_sample_rate(rate).map_err(|err| {
        ConfigError::new(&format!(
            "Failed to set sample rate after rate-change cycle: {err:?}"
        ))
    })?;

    // Verify the rate
    let verify = driver.sample_rate().map_err(|err| {
        ConfigError::new(&format!(
            "Failed to read ASIO sample rate after rate-change cycle: {err:?}"
        ))
    })?;
    debug!("ASIO sample rate after dummy-stream cycle: {verify} Hz (requested {rate} Hz).");
    if (verify - rate).abs() > 0.5 {
        return Err(ConfigError::new(&format!(
            "ASIO sample rate is {verify} Hz after rate-change cycle, expected {rate} Hz. \
             The driver may require the rate to be set via its control panel."
        )));
    }
    Ok(())
}

/// Open an ASIO device: load driver, init, set sample rate, query channels.
/// The sample rate is set immediately after ASIOInit, before any other calls,
/// because some ASIO drivers lock in the rate once channels or buffers are queried.
/// Returns (num_inputs, num_outputs).
pub fn open_asio_device(devname: &str, samplerate: usize) -> Result<(i32, i32), ConfigError> {
    let available = list_device_names();
    debug!("Available ASIO devices: {:?}", available);
    if load_driver_by_name(devname).is_err() {
        // Driver load failed — provide a helpful error with available devices.
        let close_matches: Vec<&str> = available
            .iter()
            .filter(|n| {
                n.to_lowercase().contains(&devname.to_lowercase())
                    || devname.to_lowercase().contains(&n.to_lowercase())
            })
            .map(|s| s.as_str())
            .collect();
        let hint = if !close_matches.is_empty() && !available.iter().any(|n| n == devname) {
            format!(" Did you mean one of: {:?}?", close_matches)
        } else {
            String::new()
        };
        return Err(ConfigError::new(&format!(
            "Failed to load ASIO driver '{}'. Available devices: {:?}.{} \
             If the device is listed, this may be caused by an architecture mismatch \
             between the driver and the CamillaDSP binary (this binary is {}).",
            devname,
            available,
            hint,
            std::env::consts::ARCH
        )));
    }
    let driver = loaded_wrapper_driver()?;

    // Log current sample rate before any changes
    let current_rate = driver
        .sample_rate()
        .map_err(|err| ConfigError::new(&format!("Failed to read ASIO sample rate: {err:?}")))?;
    debug!("ASIO current sample rate: {current_rate} Hz");

    // Log supported sample rates
    const COMMON_RATES: &[u32] = &[
        8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800,
        384000, 705600, 768000,
    ];
    let supported: Vec<u32> = COMMON_RATES
        .iter()
        .copied()
        .filter(|&rate| driver.can_sample_rate(rate as f64).unwrap_or(false))
        .collect();
    debug!("ASIO supported sample rates: {:?}", supported);

    // Set the requested sample rate IMMEDIATELY after ASIOInit, before ASIOGetChannels.
    // Some drivers lock in the rate once channels or buffers are queried.
    let rate = samplerate as f64;
    if !driver.can_sample_rate(rate).unwrap_or(false) {
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
        driver.set_sample_rate(rate).map_err(|err| {
            ConfigError::new(&format!(
                "Failed to set ASIO sample rate to {samplerate} Hz: {err:?}"
            ))
        })?;

        // Some ASIO drivers (e.g. Steinberg) don't truly apply the rate change
        // until a full buffer-creation cycle has been performed.  Force this by
        // running a brief dummy stream: CreateBuffers → Start → Stop → Dispose,
        // then tear the driver down and re-initialise cleanly.
        debug!("Forcing ASIO rate change to {samplerate} Hz via dummy stream cycle.");
        force_sample_rate_with_dummy_cycle(devname, rate)?;
    }

    // Query channels AFTER the sample rate has been set
    let channels = driver
        .channels()
        .map_err(|err| ConfigError::new(&format!("ASIOGetChannels failed: {err:?}")))?;
    let num_inputs: i32 = channels.ins;
    let num_outputs: i32 = channels.outs;
    debug!("ASIO device opened: {num_inputs} input channels, {num_outputs} output channels.");

    // Log per-channel details (name and sample format)
    for ch in 0..num_inputs {
        let mut info = ASIOChannelInfo {
            channel: ch,
            isInput: 1,
            isActive: 0,
            channelGroup: 0,
            type_: 0,
            name: [0; 32],
        };
        if unsafe { ASIOGetChannelInfo(&mut info) } == 0 {
            let name = fixed_cstr_buf_to_string(&info.name);
            debug!(
                "  Input  channel {ch}: name='{name}', format={} ({})",
                info.type_,
                asio_sample_type_name(info.type_),
            );
        }
    }
    for ch in 0..num_outputs {
        let mut info = ASIOChannelInfo {
            channel: ch,
            isInput: 0,
            isActive: 0,
            channelGroup: 0,
            type_: 0,
            name: [0; 32],
        };
        if unsafe { ASIOGetChannelInfo(&mut info) } == 0 {
            let name = fixed_cstr_buf_to_string(&info.name);
            debug!(
                "  Output channel {ch}: name='{name}', format={} ({})",
                info.type_,
                asio_sample_type_name(info.type_),
            );
        }
    }

    Ok((num_inputs, num_outputs))
}

/// Create ASIO buffers and register callbacks.
fn create_asio_buffers(
    buffer_infos: &mut [ASIOBufferInfo],
    num_channels: i32,
    buffer_size: i32,
    callbacks: &mut ASIOCallbacks,
) -> Result<(), ConfigError> {
    trace!(
        "Calling ASIOCreateBuffers: infos_ptr={:p}, channels={}, buffer_size={}, callbacks_ptr={:p}",
        buffer_infos.as_mut_ptr(),
        num_channels,
        buffer_size,
        callbacks as *mut ASIOCallbacks
    );
    let res = unsafe {
        ASIOCreateBuffers(
            buffer_infos.as_mut_ptr(),
            num_channels,
            buffer_size,
            callbacks,
        )
    };
    trace!("ASIOCreateBuffers returned {}.", res);
    if res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOCreateBuffers failed with error code {res}"
        )));
    }
    Ok(())
}

fn prepare_output_stream_with_wrapper(
    num_channels: usize,
) -> Result<(Vec<ASIOBufferInfo>, i32), ConfigError> {
    let driver = loaded_wrapper_driver()?;
    let streams = driver
        .prepare_output_stream(None, num_channels, None)
        .map_err(|err| ConfigError::new(&format!("prepare_output_stream failed: {err:?}")))?;
    let output = streams
        .output
        .ok_or_else(|| ConfigError::new("ASIO wrapper did not return an output stream"))?;
    let buffer_size = output.buffer_size;
    let buffer_infos = output
        .buffer_infos
        .into_iter()
        .map(|info| ASIOBufferInfo {
            isInput: info.is_input,
            channelNum: info.channel_num,
            buffers: info.buffers,
        })
        .collect();
    Ok((buffer_infos, buffer_size))
}

fn prepare_input_stream_with_wrapper(
    num_channels: usize,
) -> Result<(Vec<ASIOBufferInfo>, i32), ConfigError> {
    let driver = loaded_wrapper_driver()?;
    let streams = driver
        .prepare_input_stream(None, num_channels, None)
        .map_err(|err| ConfigError::new(&format!("prepare_input_stream failed: {err:?}")))?;
    let input = streams
        .input
        .ok_or_else(|| ConfigError::new("ASIO wrapper did not return an input stream"))?;
    let buffer_size = input.buffer_size;
    let buffer_infos = input
        .buffer_infos
        .into_iter()
        .map(|info| ASIOBufferInfo {
            isInput: info.is_input,
            channelNum: info.channel_num,
            buffers: info.buffers,
        })
        .collect();
    Ok((buffer_infos, buffer_size))
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
    let resolved_format = resolve_format(configured_format, false)?;
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
    let resolved_format = resolve_format(configured_format, true)?;
    Ok(resolved_format)
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// List available ASIO driver names.
pub fn list_device_names() -> Vec<String> {
    Asio::new().driver_names()
}

/// List available ASIO devices as (name, description) pairs.
pub fn list_available_devices() -> Vec<(String, String)> {
    let names = list_device_names();
    names.iter().map(|n| (n.clone(), n.clone())).collect()
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
        let devname = self.devname.clone().unwrap_or_default();
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

                let mut rate_controller = PIRateController::new_with_default_gains(
                    samplerate,
                    adjust_period as f64,
                    target_level,
                );

                // --- Device-specific setup (full-duplex vs single-direction) ---
                // Format is resolved inside; bytes_per_sample depends on it.
                let setup_result: Result<(Option<usize>, BinarySampleFormat, usize), String> = if full_duplex {
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
                    let resolved_format = match resolve_format(&configured_format, false) {
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
                    Ok((Some(asio_buffer_size), binary_format, bytes_per_sample))
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
                    Ok((None, binary_format, bytes_per_sample))
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

                // Now create ring buffer and context with the resolved bytes_per_sample
                let ringbuffer = HeapRb::<u8>::new(
                    channels * bytes_per_sample * (2 * chunksize + 2048),
                );
                let (mut device_producer, device_consumer) = ringbuffer.split();

                // --- Create context and start ASIO ---
                let ctx_raw = if full_duplex {
                    let buffer_infos = make_buffer_infos(channels, false);
                    let ctx = Box::new(AsioPlaybackContext {
                        device_consumer,
                        sample_queue: VecDeque::with_capacity(
                            (16 * chunksize + target_level) * bytes_per_sample * channels,
                        ),
                        buffer_infos,
                        num_channels: channels,
                        buffer_size: asio_buffer_size.expect(
                            "full_duplex setup must provide asio_buffer_size",
                        ),
                        bytes_per_sample,
                        read_tmp: vec![
                            0u8;
                            asio_buffer_size.expect("full_duplex setup must provide asio_buffer_size")
                                * bytes_per_sample
                                * channels
                        ],
                        target_level,
                        buffer_fill: buffer_fill_clone,
                        running: false,
                        starting: true,
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
                    let (buffer_infos, wrapper_buf_size) = match prepare_output_stream_with_wrapper(channels) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO playback stream prepare error: {err}");
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
                            (16 * chunksize + target_level) * bytes_per_sample * channels,
                        ),
                        buffer_infos,
                        num_channels: channels,
                        buffer_size: wrapper_buf_size as usize,
                        bytes_per_sample,
                        read_tmp: vec![
                            0u8;
                            (wrapper_buf_size as usize) * bytes_per_sample * channels
                        ],
                        target_level,
                        buffer_fill: buffer_fill_clone,
                        running: false,
                        starting: true,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    PLAYBACK_CONTEXT.store(ctx_raw, Ordering::Release);

                    let driver = match loaded_wrapper_driver() {
                        Ok(driver) => driver,
                        Err(err) => {
                            let msg = format!("ASIO playback wrapper driver error: {err}");
                            error!("{msg}");
                            status_channel
                                .send(StatusMessage::PlaybackError(msg))
                                .unwrap_or(());
                            PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                            let _ = unsafe { Box::from_raw(ctx_raw) };
                            barrier.wait();
                            return;
                        }
                    };

                    let _callback_id =
                        driver.add_callback(|callback_info| unsafe {
                            buffer_switch_playback(callback_info.buffer_index, 0)
                        });

                    if let Err(err) = driver.start() {
                        let msg = format!("ASIOStart failed: {err:?}");
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
                };

                match status_channel.send(StatusMessage::PlaybackReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }

                let mut buf =
                    vec![0u8; channels * chunksize * bytes_per_sample];

                debug!("Playback device ready and waiting.");
                barrier.wait();
                debug!("Playback device starts now!");

                let mut conversion_result;
                'deviceloop: loop {
                    match channel.recv() {
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
                            {
                                if let Some(av_delay) = buffer_avg.average() {
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
                                    playback_status.write().buffer_level = av_delay as usize;
                                }
                            }

                            chunk.update_stats(&mut chunk_stats);
                            conversion_result =
                                chunk_to_buffer_rawbytes(chunk, &mut buf, &binary_format);
                            if let Some(mut playback_status) = playback_status.try_write() {
                                if conversion_result.1 > 0 {
                                    playback_status.clipped_samples += conversion_result.1;
                                }
                                playback_status
                                    .signal_rms
                                    .add_record_squared(chunk_stats.rms_linear());
                                playback_status
                                    .signal_peak
                                    .add_record(chunk_stats.peak_linear());
                            } else {
                                xtrace!("Playback status blocked, skip rms update.");
                            }

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
                        Err(err) => {
                            error!("Playback, message channel error: {err}.");
                            status_channel
                                .send(StatusMessage::PlaybackError(err.to_string()))
                                .unwrap_or(());
                            break 'deviceloop;
                        }
                    }
                }

                // Stop ASIO and clean up.
                // In full-duplex mode, release_shared_asio() must be called BEFORE
                // nullifying the context, because the last side to exit calls
                // ASIOStop() which waits for any in-flight callback to finish.
                // Only after that is it safe to free the context.
                debug!("Stopping ASIO playback.");
                if full_duplex {
                    release_shared_asio();
                } else {
                    PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    let (stop_res, dispose_res) = if let Ok(driver) = loaded_wrapper_driver() {
                        let stop_res = driver.stop().is_ok();
                        let dispose_res = driver.dispose_buffers().is_ok();
                        (stop_res, dispose_res)
                    } else {
                        (false, false)
                    };
                    drop_wrapper_driver();
                    let _ = (stop_res, dispose_res);
                    trace!(
                        "Playback cleanup (wrapper): stop_ok={}, dispose_ok={}",
                        stop_res,
                        dispose_res
                    );
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
        _processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone().unwrap_or_default();
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
                let mut resampler = new_resampler(
                    &resampler_conf,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
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
                let setup_result: Result<(Option<usize>, BinarySampleFormat, usize), String> = if full_duplex {
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
                    let resolved_format = match resolve_format(&configured_format, true) {
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
                    Ok((Some(preferred_buf as usize), binary_format, bytes_per_sample))
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
                    Ok((None, binary_format, bytes_per_sample))
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
                let ringbuffer =
                    HeapRb::<u8>::new(blockalign * (2 * chunksize + 2048));
                let (device_producer, mut device_consumer) = ringbuffer.split();

                // --- Create context and start ASIO ---
                let ctx_raw = if full_duplex {
                    let buffer_infos = make_buffer_infos(channels, true);
                    let ctx = Box::new(AsioCaptureContext {
                        device_producer,
                        tx_dev,
                        buffer_infos,
                        num_channels: channels,
                        buffer_size: asio_buffer_size
                            .expect("full_duplex setup must provide asio_buffer_size"),
                        bytes_per_sample,
                        interleaved_tmp: vec![
                            0u8;
                            asio_buffer_size.expect("full_duplex setup must provide asio_buffer_size")
                                * bytes_per_sample
                                * channels
                        ],
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
                    let (buffer_infos, wrapper_buf_size) = match prepare_input_stream_with_wrapper(channels) {
                        Ok(result) => result,
                        Err(err) => {
                            let msg = format!("ASIO capture stream prepare error: {err}");
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
                        buffer_infos,
                        num_channels: channels,
                        buffer_size: wrapper_buf_size as usize,
                        bytes_per_sample,
                        interleaved_tmp: vec![
                            0u8;
                            (wrapper_buf_size as usize) * bytes_per_sample * channels
                        ],
                        chunk_counter: 0,
                    });
                    let ctx_raw = Box::into_raw(ctx);
                    CAPTURE_CONTEXT.store(ctx_raw, Ordering::Release);

                    let driver = match loaded_wrapper_driver() {
                        Ok(driver) => driver,
                        Err(err) => {
                            let msg = format!("ASIO capture wrapper driver error: {err}");
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
                    };

                    let _callback_id =
                        driver.add_callback(|callback_info| unsafe {
                            buffer_switch_capture(callback_info.buffer_index, 0)
                        });

                    if let Err(err) = driver.start() {
                        let msg = format!("ASIOStart failed: {err:?}");
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
                let mut rate_adjust = 0.0;
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
                debug!("Capture device starts now!");

                'deviceloop: loop {
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
                                        .resampler
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
                            let samples_per_sec = averager.average();
                            averager.restart();
                            let measured_rate_f = samples_per_sec;
                            if let Ok(mut capture_status) =
                                RwLockUpgradableReadGuard::try_upgrade(capture_status)
                            {
                                capture_status.measured_samplerate =
                                    measured_rate_f as usize;
                                capture_status.signal_range = value_range as f32;
                                capture_status.rate_adjust = rate_adjust as f32;
                                capture_status.state = state;
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
                    if let Some(mut capture_status) = capture_status.try_write() {
                        capture_status
                            .signal_rms
                            .add_record_squared(chunk_stats.rms_linear());
                        capture_status
                            .signal_peak
                            .add_record(chunk_stats.peak_linear());
                    } else {
                        xtrace!("Capture status blocked, skip rms update.");
                    }

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

                // Stop ASIO and clean up.
                // In full-duplex mode, release_shared_asio() must be called BEFORE
                // nullifying the context, because the last side to exit calls
                // ASIOStop() which waits for any in-flight callback to finish.
                // Only after that is it safe to free the context.
                debug!("Stopping ASIO capture.");
                if full_duplex {
                    release_shared_asio();
                } else {
                    CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    let (stop_res, dispose_res) = if let Ok(driver) = loaded_wrapper_driver() {
                        let stop_res = driver.stop().is_ok();
                        let dispose_res = driver.dispose_buffers().is_ok();
                        (stop_res, dispose_res)
                    } else {
                        (false, false)
                    };
                    drop_wrapper_driver();
                    let _ = (stop_res, dispose_res);
                    trace!(
                        "Capture cleanup (wrapper): stop_ok={}, dispose_ok={}",
                        stop_res,
                        dispose_res
                    );
                }
                // Harmless if already nulled by release_shared_asio
                CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                let _ = unsafe { Box::from_raw(ctx_raw) };
                capture_status.write().state = ProcessingState::Inactive;
            })?;
        Ok(Box::new(handle))
    }
}
