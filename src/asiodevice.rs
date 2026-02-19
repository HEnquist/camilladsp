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
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use crossbeam_channel::{bounded, TrySendError};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use ringbuf::{HeapRb, traits::*};

use asio_sys::bindings::asio_import::{
    ASIOBufferInfo, ASIOCallbacks, ASIOCreateBuffers, ASIODriverInfo, ASIOGetBufferSize,
    ASIOGetChannels, ASIOInit, ASIOStart, ASIOStop,
};

use crate::audiodevice::*;
use crate::config::{AsioSampleFormat, BinarySampleFormat, ConfigError};
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use crate::helpers::PIRateController;
use crate::resampling::{new_resampler, resampler_is_async, ChunkResampler};
use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
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
    chunk_counter: u64,
}

static PLAYBACK_CONTEXT: AtomicPtr<AsioPlaybackContext> = AtomicPtr::new(ptr::null_mut());
static CAPTURE_CONTEXT: AtomicPtr<AsioCaptureContext> = AtomicPtr::new(ptr::null_mut());

// ---------------------------------------------------------------------------
// ASIO callbacks  (unsafe extern "C" — called from ASIO driver thread)
// ---------------------------------------------------------------------------

/// ASIO bufferSwitch callback for playback.
/// Reads converted audio bytes from the ring buffer and copies them into the ASIO output buffers.
pub unsafe extern "C" fn buffer_switch_playback(buffer_index: i32, _direct_process: i32) {
    let ctx_ptr = PLAYBACK_CONTEXT.load(Ordering::Acquire);
    if ctx_ptr.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx_ptr };
    let bytes_per_frame = ctx.bytes_per_sample * ctx.num_channels;
    let needed_bytes = ctx.buffer_size * bytes_per_frame;

    // Fill the sample queue from the ring buffer
    while ctx.sample_queue.len() < needed_bytes {
        let available = ctx.device_consumer.occupied_len();
        if available == 0 {
            // No data — fill remainder with silence
            for _ in 0..(needed_bytes - ctx.sample_queue.len()) {
                ctx.sample_queue.push_back(0);
            }
            if ctx.running {
                ctx.running = false;
            }
            break;
        }
        if !ctx.running {
            ctx.running = true;
            if ctx.starting {
                ctx.starting = false;
            }
            // Insert target_level silent frames to build initial buffer
            for _ in 0..(ctx.target_level * bytes_per_frame) {
                ctx.sample_queue.push_back(0);
            }
        }
        let to_read = available.min(needed_bytes.saturating_sub(ctx.sample_queue.len()));
        let mut tmp = vec![0u8; to_read];
        ctx.device_consumer.pop_slice(&mut tmp);
        for b in tmp {
            ctx.sample_queue.push_back(b);
        }
    }

    // Copy interleaved data into per-channel ASIO buffers (de-interleave)
    for frame in 0..ctx.buffer_size {
        for ch in 0..ctx.num_channels {
            let buffer_info = &ctx.buffer_infos[ch];
            let out_ptr = buffer_info.buffers[buffer_index as usize];
            if !out_ptr.is_null() {
                let dst = unsafe { (out_ptr as *mut u8).add(frame * ctx.bytes_per_sample) };
                for byte_idx in 0..ctx.bytes_per_sample {
                    let sample_byte = ctx.sample_queue.pop_front().unwrap_or(0);
                    unsafe {
                        *dst.add(byte_idx) = sample_byte;
                    }
                }
            } else {
                // Discard bytes even if buffer pointer is null
                for _ in 0..ctx.bytes_per_sample {
                    ctx.sample_queue.pop_front();
                }
            }
        }
    }

    // Update buffer fill estimate
    let curr_buffer_fill = ctx.sample_queue.len() / bytes_per_frame;
    if let Ok(mut estimator) = ctx.buffer_fill.try_lock() {
        estimator.add(curr_buffer_fill);
    }
}

/// ASIO bufferSwitch callback for capture.
/// Reads audio bytes from the ASIO input buffers and pushes them into the ring buffer.
pub unsafe extern "C" fn buffer_switch_capture(buffer_index: i32, _direct_process: i32) {
    let ctx_ptr = CAPTURE_CONTEXT.load(Ordering::Acquire);
    if ctx_ptr.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx_ptr };
    let bytes_per_frame = ctx.bytes_per_sample * ctx.num_channels;
    let total_bytes = ctx.buffer_size * bytes_per_frame;
    let mut buf = vec![0u8; total_bytes];

    // Read from per-channel ASIO input buffers and interleave into buf
    for frame in 0..ctx.buffer_size {
        for ch in 0..ctx.num_channels {
            let buffer_info = &ctx.buffer_infos[ch];
            let in_ptr = buffer_info.buffers[buffer_index as usize];
            if !in_ptr.is_null() {
                let src = unsafe { (in_ptr as *const u8).add(frame * ctx.bytes_per_sample) };
                let offset = (frame * ctx.num_channels + ch) * ctx.bytes_per_sample;
                for byte_idx in 0..ctx.bytes_per_sample {
                    buf[offset + byte_idx] = unsafe { *src.add(byte_idx) };
                }
            }
        }
    }

    // Push into ring buffer
    let pushed_bytes = ctx.device_producer.push_slice(&buf);
    if pushed_bytes < buf.len() {
        // Ring buffer full — data will be lost
    }
    match ctx.tx_dev.try_send((ctx.chunk_counter, pushed_bytes)) {
        Ok(()) => {}
        Err(TrySendError::Full((nbr, length_bytes))) => {
            // Channel full, drop notification
            let _ = (nbr, length_bytes);
        }
        Err(_) => {
            // Channel disconnected
        }
    }
    ctx.chunk_counter += 1;
}

/// ASIO asioMessage callback.
/// Handles driver queries about supported features.
/// Returning 0 means "not supported" or "no" for most selectors.
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
                | K_ASIO_SELECTOR_SUPPORTED => 1, // yes
                K_ASIO_SUPPORTS_TIME_INFO
                | K_ASIO_SUPPORTS_TIME_CODE
                | K_ASIO_RESET_REQUEST
                | K_ASIO_BUFFER_SIZE_CHANGE => 0, // no
                _ => 0,
            }
        }
        K_ASIO_ENGINE_VERSION => 2, // ASIO 2.0
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

// ---------------------------------------------------------------------------
// ASIO low-level helpers
// ---------------------------------------------------------------------------

/// Resolve ASIO sample format to a BinarySampleFormat.
fn resolve_binary_format(format: &Option<AsioSampleFormat>) -> BinarySampleFormat {
    match format {
        Some(AsioSampleFormat::S16) => BinarySampleFormat::S16_LE,
        Some(AsioSampleFormat::S24) => BinarySampleFormat::S24_4_LJ_LE,
        Some(AsioSampleFormat::S32) => BinarySampleFormat::S32_LE,
        Some(AsioSampleFormat::F32) => BinarySampleFormat::F32_LE,
        Some(AsioSampleFormat::F64) => BinarySampleFormat::F64_LE,
        None => BinarySampleFormat::S32_LE, // default for ASIO
    }
}

/// Load an ASIO driver by name.
pub fn load_driver_by_name(name: &str) -> Result<(), ConfigError> {
    let c_name = CString::new(name)
        .map_err(|_| ConfigError::new("Invalid device name (contains null)"))?;
    let ok =
        unsafe { asio_sys::bindings::asio_import::load_asio_driver(c_name.as_ptr() as *mut i8) };
    if ok {
        Ok(())
    } else {
        Err(ConfigError::new("Failed to load ASIO driver"))
    }
}

/// Open an ASIO device: load driver, init, query channels.
/// Returns (num_inputs, num_outputs).
pub fn open_asio_device(devname: &str) -> Result<(i32, i32), ConfigError> {
    let available = list_device_names();
    debug!("Available ASIO devices: {:?}", available);
    load_driver_by_name(devname)?;
    let mut driver_info = ASIODriverInfo {
        asioVersion: 2,
        driverVersion: 0,
        name: [0; 32],
        errorMessage: [0; 124],
        sysRef: ptr::null_mut(),
    };
    let init_result = unsafe { ASIOInit(&mut driver_info) };
    if init_result != 0 {
        let err_msg = unsafe {
            CStr::from_ptr(driver_info.errorMessage.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        error!("ASIOInit error message: {err_msg}");
        return Err(ConfigError::new(&format!(
            "ASIOInit failed with error code {init_result}"
        )));
    }
    let mut num_inputs: i32 = 0;
    let mut num_outputs: i32 = 0;
    let ch_result = unsafe { ASIOGetChannels(&mut num_inputs, &mut num_outputs) };
    if ch_result != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOGetChannels failed with error code {ch_result}"
        )));
    }
    debug!(
        "ASIO device opened: {num_inputs} input channels, {num_outputs} output channels."
    );
    Ok((num_inputs, num_outputs))
}

/// Query ASIO preferred buffer size, allocate ASIOBufferInfo array.
fn setup_asio_buffers(
    num_channels: usize,
    is_input: bool,
) -> Result<(Vec<ASIOBufferInfo>, i32), ConfigError> {
    let mut min_buf: i32 = 0;
    let mut max_buf: i32 = 0;
    let mut preferred_buf: i32 = 0;
    let mut granularity: i32 = 0;
    let res = unsafe {
        ASIOGetBufferSize(&mut min_buf, &mut max_buf, &mut preferred_buf, &mut granularity)
    };
    if res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOGetBufferSize failed with error code {res}"
        )));
    }
    debug!(
        "ASIO buffer sizes: min={min_buf}, max={max_buf}, preferred={preferred_buf}, granularity={granularity}."
    );
    let mut buffer_infos = Vec::with_capacity(num_channels);
    for ch in 0..num_channels {
        buffer_infos.push(ASIOBufferInfo {
            isInput: if is_input { 1 } else { 0 },
            channelNum: ch as i32,
            buffers: [ptr::null_mut(), ptr::null_mut()],
        });
    }
    Ok((buffer_infos, preferred_buf))
}

/// Create ASIO buffers and register callbacks.
fn create_asio_buffers(
    buffer_infos: &mut [ASIOBufferInfo],
    num_channels: i32,
    buffer_size: i32,
    callbacks: &mut ASIOCallbacks,
) -> Result<(), ConfigError> {
    let res = unsafe {
        ASIOCreateBuffers(
            buffer_infos.as_mut_ptr(),
            num_channels,
            buffer_size,
            callbacks,
        )
    };
    if res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOCreateBuffers failed with error code {res}"
        )));
    }
    Ok(())
}

/// Open and set up an ASIO device for playback. Returns (buffer_infos, preferred_buf_size).
fn open_asio_playback(
    devname: &str,
    num_channels: usize,
) -> Result<(Vec<ASIOBufferInfo>, i32), ConfigError> {
    let (_inputs, outputs) = open_asio_device(devname)?;
    if num_channels > outputs as usize {
        return Err(ConfigError::new(&format!(
            "Requested {num_channels} output channels but device only has {outputs}"
        )));
    }
    let (buffer_infos, preferred_buf) = setup_asio_buffers(num_channels, false)?;
    Ok((buffer_infos, preferred_buf))
}

/// Open and set up an ASIO device for capture. Returns (buffer_infos, preferred_buf_size).
fn open_asio_capture(
    devname: &str,
    num_channels: usize,
) -> Result<(Vec<ASIOBufferInfo>, i32), ConfigError> {
    let (inputs, _outputs) = open_asio_device(devname)?;
    if num_channels > inputs as usize {
        return Err(ConfigError::new(&format!(
            "Requested {num_channels} input channels but device only has {inputs}"
        )));
    }
    let (buffer_infos, preferred_buf) = setup_asio_buffers(num_channels, true)?;
    Ok((buffer_infos, preferred_buf))
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// List available ASIO driver names.
pub fn list_device_names() -> Vec<String> {
    const MAX_DRIVERS: usize = 32;
    const NAME_LEN: usize = 128;
    // Allocate buffers for the driver names — get_driver_names writes into these.
    let mut buffers = vec![[0i8; NAME_LEN]; MAX_DRIVERS];
    let mut ptrs: Vec<*mut i8> = buffers.iter_mut().map(|b| b.as_mut_ptr()).collect();
    let count = unsafe {
        asio_sys::bindings::asio_import::get_driver_names(ptrs.as_mut_ptr(), MAX_DRIVERS as i32)
    };
    let mut names = Vec::new();
    if count > 0 {
        for i in 0..(count as usize).min(MAX_DRIVERS) {
            let name = unsafe { CStr::from_ptr(ptrs[i]).to_string_lossy().into_owned() };
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
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
        let devname = self
            .devname
            .clone()
            .unwrap_or_default();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let binary_format = resolve_binary_format(&self.sample_format);
        let bytes_per_sample = binary_format.bytes_per_sample();
        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let adjust = self.adjust_period > 0.0 && self.enable_rate_adjust;

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

                let ringbuffer = HeapRb::<u8>::new(
                    channels * bytes_per_sample * (2 * chunksize + 2048),
                );
                let (mut device_producer, device_consumer) = ringbuffer.split();

                // Open ASIO device and set up buffers
                let (mut buffer_infos, preferred_buf) = match open_asio_playback(&devname, channels)
                {
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

                let asio_buffer_size = preferred_buf as usize;

                // Build the callback context
                let ctx = Box::new(AsioPlaybackContext {
                    device_consumer,
                    sample_queue: VecDeque::with_capacity(
                        (16 * chunksize + target_level) * bytes_per_sample * channels,
                    ),
                    buffer_infos: buffer_infos.clone(),
                    num_channels: channels,
                    buffer_size: asio_buffer_size,
                    bytes_per_sample,
                    target_level,
                    buffer_fill: buffer_fill_clone,
                    running: false,
                    starting: true,
                });
                let ctx_raw = Box::into_raw(ctx);
                PLAYBACK_CONTEXT.store(ctx_raw, Ordering::Release);

                // Register ASIO callbacks and create buffers
                let mut callbacks = ASIOCallbacks {
                    bufferSwitch: Some(buffer_switch_playback),
                    sampleRateDidChange: None,
                    asioMessage: Some(asio_message_callback),
                    bufferSwitchTimeInfo: None,
                };
                if let Err(err) = create_asio_buffers(
                    &mut buffer_infos,
                    channels as i32,
                    preferred_buf,
                    &mut callbacks,
                ) {
                    let msg = format!("ASIO playback buffer creation error: {err}");
                    error!("{msg}");
                    status_channel
                        .send(StatusMessage::PlaybackError(msg))
                        .unwrap_or(());
                    // Clean up context
                    PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    let _ = unsafe { Box::from_raw(ctx_raw) };
                    barrier.wait();
                    return;
                }

                // Update the context's buffer_infos with the ASIO-filled pointers
                {
                    let ctx = unsafe { &mut *ctx_raw };
                    ctx.buffer_infos = buffer_infos;
                }

                // Start ASIO stream
                let start_res = unsafe { ASIOStart() };
                if start_res != 0 {
                    let msg = format!("ASIOStart failed with error code {start_res}");
                    error!("{msg}");
                    status_channel
                        .send(StatusMessage::PlaybackError(msg))
                        .unwrap_or(());
                    PLAYBACK_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                    let _ = unsafe { Box::from_raw(ctx_raw) };
                    barrier.wait();
                    return;
                }

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

                            let pushed_bytes =
                                device_producer.push_slice(&buf[0..conversion_result.0]);
                            if pushed_bytes < conversion_result.0 {
                                debug!(
                                    "Playback ring buffer is full, dropped {} out of {} bytes.",
                                    conversion_result.0 - pushed_bytes,
                                    conversion_result.0
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

                // Stop ASIO and clean up
                debug!("Stopping ASIO playback.");
                let _ = unsafe { ASIOStop() };
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
        let devname = self
            .devname
            .clone()
            .unwrap_or_default();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let binary_format = resolve_binary_format(&self.sample_format);
        let bytes_per_sample = binary_format.bytes_per_sample();
        let resampler_conf = self.resampler_config;
        let async_src = resampler_is_async(&resampler_conf);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = (1000.0 * self.rate_measure_interval) as u64;

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

                let blockalign = bytes_per_sample * channels;
                let ringbuffer =
                    HeapRb::<u8>::new(blockalign * (2 * chunksize + 2048));
                let (device_producer, mut device_consumer) = ringbuffer.split();

                // Open ASIO device and set up buffers
                let (mut buffer_infos, preferred_buf) =
                    match open_asio_capture(&devname, channels) {
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

                // Build capture context
                let ctx = Box::new(AsioCaptureContext {
                    device_producer,
                    tx_dev,
                    buffer_infos: buffer_infos.clone(),
                    num_channels: channels,
                    buffer_size: preferred_buf as usize,
                    bytes_per_sample,
                    chunk_counter: 0,
                });
                let ctx_raw = Box::into_raw(ctx);
                CAPTURE_CONTEXT.store(ctx_raw, Ordering::Release);

                // Register ASIO callbacks and create buffers
                let mut callbacks = ASIOCallbacks {
                    bufferSwitch: Some(buffer_switch_capture),
                    sampleRateDidChange: None,
                    asioMessage: Some(asio_message_callback),
                    bufferSwitchTimeInfo: None,
                };
                if let Err(err) = create_asio_buffers(
                    &mut buffer_infos,
                    channels as i32,
                    preferred_buf,
                    &mut callbacks,
                ) {
                    let msg = format!("ASIO capture buffer creation error: {err}");
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

                // Update the context's buffer_infos with the ASIO-filled pointers
                {
                    let ctx = unsafe { &mut *ctx_raw };
                    ctx.buffer_infos = buffer_infos;
                }

                // Start ASIO stream
                let start_res = unsafe { ASIOStart() };
                if start_res != 0 {
                    let msg = format!("ASIOStart failed with error code {start_res}");
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
                        match rx_dev.recv() {
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
                            Err(err) => {
                                error!("Capture, channel is closed: {err}.");
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

                // Stop ASIO and clean up
                debug!("Stopping ASIO capture.");
                let _ = unsafe { ASIOStop() };
                CAPTURE_CONTEXT.store(ptr::null_mut(), Ordering::Release);
                let _ = unsafe { Box::from_raw(ctx_raw) };
                capture_status.write().state = ProcessingState::Inactive;
            })?;
        Ok(Box::new(handle))
    }
}
