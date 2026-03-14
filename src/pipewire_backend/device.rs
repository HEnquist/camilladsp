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

use pipewire as pw;
use pw::spa::param::audio::AudioFormat;
use pw::spa::utils::Direction;
use pw::spa::utils::Id;
use pw::stream::{Stream, StreamFlags};

// Re-import the properties macro
use pipewire::properties::properties;

use crate::audiodevice::*;
use crate::config;
use crate::config::BinarySampleFormat;
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::utils::countertimer;
use crate::utils::rate_controller::PIRateController;
use crate::resampling::{ChunkResampler, new_resampler, resampler_is_async};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use ringbuf::{HeapRb, traits::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

/// Thread-safe handle to quit a PipeWire MainLoop from any thread.
/// PipeWire's pw_main_loop_quit() is documented as thread-safe:
/// "This function can be safely called from another thread."
struct MainLoopQuitter {
    raw: usize,
}

// SAFETY: pw_main_loop_quit() is thread-safe according to PipeWire documentation
unsafe impl Send for MainLoopQuitter {}
unsafe impl Sync for MainLoopQuitter {}

impl MainLoopQuitter {
    fn new(mainloop: &pw::main_loop::MainLoop) -> Self {
        Self {
            raw: mainloop.as_raw_ptr() as usize,
        }
    }

    fn quit(&self) {
        unsafe {
            pw::sys::pw_main_loop_quit(self.raw as *mut pw::sys::pw_main_loop);
        }
    }
}

#[derive(Debug)]
pub struct PipeWireError {
    desc: String,
}

impl std::fmt::Display for PipeWireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl std::error::Error for PipeWireError {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl PipeWireError {
    pub fn new(desc: &str) -> Self {
        PipeWireError {
            desc: format!("PipeWire error: {}", desc),
        }
    }
}

/// PipeWire playback device
pub struct PipeWirePlaybackDevice {
    pub node_name: Option<String>,
    pub node_description: Option<String>,
    pub node_group_name: Option<String>,
    pub autoconnect_to: Option<String>,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

/// PipeWire capture device
pub struct PipeWireCaptureDevice {
    pub node_name: Option<String>,
    pub node_description: Option<String>,
    pub node_group_name: Option<String>,
    pub autoconnect_to: Option<String>,
    pub samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
}

/// Build audio format POD for stream parameters
/// Always uses F32LE format since PipeWire uses F32 internally
fn build_audio_params(samplerate: u32, channels: u32) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(1024);
    let _pod = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(&mut buffer),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
            id: pw::spa::sys::SPA_PARAM_EnumFormat,
            properties: vec![
                pw::spa::pod::Property {
                    key: pw::spa::sys::SPA_FORMAT_mediaType,
                    flags: pw::spa::pod::PropertyFlags::empty(),
                    value: pw::spa::pod::Value::Id(Id(pw::spa::sys::SPA_MEDIA_TYPE_audio)),
                },
                pw::spa::pod::Property {
                    key: pw::spa::sys::SPA_FORMAT_mediaSubtype,
                    flags: pw::spa::pod::PropertyFlags::empty(),
                    value: pw::spa::pod::Value::Id(Id(pw::spa::sys::SPA_MEDIA_SUBTYPE_raw)),
                },
                pw::spa::pod::Property {
                    key: pw::spa::sys::SPA_FORMAT_AUDIO_format,
                    flags: pw::spa::pod::PropertyFlags::empty(),
                    value: pw::spa::pod::Value::Id(Id(AudioFormat::F32LE.as_raw())),
                },
                pw::spa::pod::Property {
                    key: pw::spa::sys::SPA_FORMAT_AUDIO_rate,
                    flags: pw::spa::pod::PropertyFlags::empty(),
                    value: pw::spa::pod::Value::Int(samplerate as i32),
                },
                pw::spa::pod::Property {
                    key: pw::spa::sys::SPA_FORMAT_AUDIO_channels,
                    flags: pw::spa::pod::PropertyFlags::empty(),
                    value: pw::spa::pod::Value::Int(channels as i32),
                },
            ],
        }),
    );
    buffer
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for PipeWirePlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let node_name = self
            .node_name
            .clone()
            .unwrap_or("camilladsp-playback".to_string());
        let node_description = self
            .node_description
            .clone()
            .unwrap_or("CamillaDSP Playback".to_string());
        let node_group_name = self
            .node_group_name
            .clone()
            .unwrap_or("camilladsp".to_string());
        let autoconnect_to = self.autoconnect_to.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let binary_format = BinarySampleFormat::F32_LE;
        let store_bytes_per_sample = binary_format.bytes_per_sample();

        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let adjust = self.adjust_period > 0.0 && self.enable_rate_adjust;

        let handle = thread::Builder::new()
            .name("PipeWirePlayback".to_string())
            .spawn(move || {
                // Initialize PipeWire
                pw::init();

                let mainloop = match pw::main_loop::MainLoop::new(None) {
                    Ok(ml) => ml,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire main loop: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                let context = match pw::context::Context::new(&mainloop) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire context: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                let core = match context.connect(None) {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = format!("Failed to connect to PipeWire: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                // Node properties for WirePlumber matching
                // NODE_LATENCY requests PipeWire to use a quantum matching our chunksize
                let latency_str = format!("{}/{}", chunksize, samplerate);
                let mut props = properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Playback",
                    *pw::keys::MEDIA_ROLE => "DSP",
                    *pw::keys::APP_NAME => "CamillaDSP",
                    *pw::keys::NODE_NAME => node_name,
                    *pw::keys::NODE_DESCRIPTION => node_description,
                    *pw::keys::NODE_LATENCY => latency_str,
                    *pw::keys::NODE_GROUP => node_group_name,
                };
                if let Some(ref target) = autoconnect_to {
                    // the key PW_KEY_TARGET_OBJECT doesn not (yet?) exist in pw::keys
                    props.insert("target.object", target.as_str());
                }

                let stream = match Stream::new(&core, "CamillaDSP-Playback", props) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire stream: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                // Shared state for callbacks
                // Use DeviceBufferEstimator to track buffer level with time-based interpolation
                let buffer_fill = Arc::new(Mutex::new(countertimer::DeviceBufferEstimator::new(
                    samplerate,
                )));
                let buffer_fill_clone = buffer_fill.clone();

                // Ring buffer for lock-free, allocation-free audio data transfer
                // The producer (receiver thread) pushes raw bytes, consumer (callback) pops them
                let ring_size = 4 * chunksize * channels * store_bytes_per_sample;
                let ringbuffer = HeapRb::<u8>::new(ring_size);
                let (mut rb_producer, rb_consumer) = ringbuffer.split();
                let rb_consumer = Rc::new(RefCell::new(rb_consumer));

                // Set up stream listener
                let rb_consumer_clone = rb_consumer.clone();
                let _listener = stream
                    .add_local_listener_with_user_data(())
                    .state_changed(move |_, _, old, new| {
                        debug!("PipeWire playback stream state: {:?} -> {:?}", old, new);
                    })
                    .process(move |stream, _| {
                        // Get buffer from PipeWire
                        let mut pw_buffer = match stream.dequeue_buffer() {
                            Some(b) => b,
                            None => {
                                trace!("PipeWire playback: no buffer available");
                                return;
                            }
                        };

                        let datas = pw_buffer.datas_mut();
                        if datas.is_empty() {
                            xtrace!("PW playback callback with empty data");
                            return;
                        }

                        let data = &mut datas[0];
                        let stride = channels * store_bytes_per_sample;

                        // Get output slice - data() returns slice sized to maxsize
                        let out_slice = match data.data() {
                            Some(s) => s,
                            None => return,
                        };
                        let max_bytes = out_slice.len();
                        xtrace!("PW playback callback with {} bytes", max_bytes);

                        // Pop bytes from ring buffer directly into output - no allocations!
                        let mut consumer = rb_consumer_clone.borrow_mut();
                        let available_bytes = consumer.occupied_len();
                        let bytes_to_write = available_bytes.min(max_bytes);

                        if available_bytes == 0 {
                            trace!("PipeWire playback: buffer empty, outputting silence");
                        }

                        // Pop directly into output slice
                        consumer.pop_slice(&mut out_slice[..bytes_to_write]);

                        // CRITICAL: Tell PipeWire how much data we wrote
                        // For output streams, we must set chunk offset, size, and stride
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.size_mut() = bytes_to_write as u32;
                        *chunk.stride_mut() = stride as i32;

                        // Update buffer level estimator
                        if let Ok(mut estimator) = buffer_fill_clone.try_lock() {
                            estimator.add(available_bytes / stride);
                        }
                    })
                    .register();

                // Build format params with channel count and sample rate (F32LE format)
                let params_buffer = build_audio_params(samplerate as u32, channels as u32);

                // Convert the buffer to a Pod reference
                let pod = pw::spa::pod::Pod::from_bytes(&params_buffer)
                    .expect("Failed to create Pod from params buffer");

                // Connect stream - set AUTOCONNECT flag if autoconnect_to is set
                let mut flags = StreamFlags::RT_PROCESS | StreamFlags::MAP_BUFFERS;
                if autoconnect_to.is_some() {
                    flags |= StreamFlags::AUTOCONNECT;
                }

                match stream.connect(
                    Direction::Output,
                    None,
                    flags,
                    &mut [pod],
                ) {
                    Ok(_) => {
                        debug!("PipeWire playback stream connected");
                    }
                    Err(e) => {
                        let msg = format!("Failed to connect PipeWire stream: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                }

                // Signal ready
                match status_channel.send(StatusMessage::PlaybackReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                debug!("Starting PipeWire playback loop");

                // Spawn a thread to receive AudioMessages and send to PipeWire thread
                let status_channel_clone = status_channel.clone();
                let playback_status_clone = playback_status.clone();
                let quitter = MainLoopQuitter::new(&mainloop);
                let stride = channels * store_bytes_per_sample;
                // Buffer level monitoring parameters (similar to other backends)
                let receiver_handle = thread::spawn(move || {
                    let mut chunk_stats = ChunkStats {
                        rms: vec![0.0; channels],
                        peak: vec![0.0; channels],
                    };
                    // Pre-allocate conversion buffer to avoid repeated allocations
                    let mut raw_buffer = vec![0u8; chunksize * stride];
                    // Buffer level tracking with time-based estimation
                    let mut buffer_avg = countertimer::Averager::new();
                    let mut buffer_level_timer = countertimer::Stopwatch::new();
                    let mut rate_controller = PIRateController::new_with_default_gains(
                        samplerate,
                        adjust_period as f64,
                        target_level,
                    );
                    let mut rate_adjust_value = 1.0;
                    let mut conversion_result;

                    let mut running = false;
                    loop {
                        match channel.recv() {
                            Ok(AudioMessage::Audio(chunk)) => {
                                let estimated_buffer_fill = buffer_fill.try_lock().map(|b| b.estimate() as f64).unwrap_or_default();
                                buffer_avg.add_value(estimated_buffer_fill + (channel.len() * chunksize) as f64);
                                if adjust && buffer_level_timer.larger_than_millis((1000.0 * adjust_period) as u64) {
                                    if let Some(av_delay) = buffer_avg.average() {
                                        let speed = rate_controller.next(av_delay);
                                        let changed = (speed - rate_adjust_value).abs() > 0.000_001;

                                        buffer_level_timer.restart();
                                        buffer_avg.restart();
                                        if changed {
                                            debug!(
                                                "Current buffer level {:.1}, set capture rate to {:.4}%.",
                                                av_delay,
                                                100.0 * speed
                                            );
                                            status_channel
                                                .send(StatusMessage::SetSpeed(speed))
                                                .unwrap_or(());
                                            rate_adjust_value = speed;
                                        }
                                        else {
                                            debug!(
                                                "Current buffer level {:.1}, leaving capture rate at {:.4}%.",
                                                av_delay,
                                                100.0 * rate_adjust_value
                                            );
                                        }
                                        playback_status.write().buffer_level = av_delay as usize;
                                    }
                                }
                                chunk.update_stats(&mut chunk_stats);

                                conversion_result = chunk_to_buffer_rawbytes(
                                    chunk,
                                    &mut raw_buffer,
                                    &binary_format,
                                );
                                if let Some(mut playback_status) = playback_status_clone.try_write() {
                                    if conversion_result.1 > 0 {
                                        playback_status.clipped_samples += conversion_result.1;
                                    }
                                    playback_status
                                        .signal_rms
                                        .add_record_squared(chunk_stats.rms_linear());
                                    playback_status
                                        .signal_peak
                                        .add_record(chunk_stats.peak_linear());
                                }
                                else {
                                    xtrace!("Playback status blocked, skipping rms update.");
                                }

                                // Wait for enough space in the ring buffer before pushing.
                                // This is essential when the capture side is not rate-limited
                                // (e.g. signal generator): without this wait the data would
                                // arrive far faster than the playback callback can drain it
                                // and most of it would be dropped.  The sleep duration is
                                // based on the time it takes to play back one chunksize.
                                let bytes_to_write = conversion_result.0;
                                let sleep_duration = std::time::Duration::from_micros(
                                    (1_000_000 * chunksize / samplerate / 2) as u64
                                );
                                let max_retries = 8;
                                for _ in 0..max_retries {
                                    if rb_producer.vacant_len() >= bytes_to_write {
                                        break;
                                    }
                                    std::thread::sleep(sleep_duration);
                                }
                                let pushed = rb_producer.push_slice(&raw_buffer[..bytes_to_write]);
                                if pushed < bytes_to_write {
                                    trace!(
                                       "Playback ring buffer full, dropped {} bytes",
                                       bytes_to_write - pushed
                                    );
                                    if running {
                                        warn!("Playback ring buffer full, dropping audio data");
                                        running = false;
                                    }
                                }
                                else if !running {
                                    running = true;
                                    debug!("PipeWire playback running")

                                }
                            }
                            Ok(AudioMessage::Pause) => {
                                trace!("Pause message received");
                            }
                            Ok(AudioMessage::EndOfStream) => {
                                status_channel_clone
                                    .send(StatusMessage::PlaybackDone)
                                    .unwrap();
                                break;
                            }
                            Err(err) => {
                                error!("Message channel error: {}", err);
                                status_channel_clone
                                    .send(StatusMessage::PlaybackDone)
                                    .unwrap();
                                break;
                            }
                        }
                    }
                    // Signal mainloop to quit - pw_main_loop_quit is thread-safe
                    quitter.quit();
                });

                // Run PipeWire main loop
                mainloop.run();

                // Wait for receiver thread
                let _ = receiver_handle.join();
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

fn nbr_capture_bytes(
    resampler: &Option<ChunkResampler>,
    capture_bytes: usize,
    channels: usize,
    store_bytes_per_sample: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        resampl.resampler.input_frames_next() * channels * store_bytes_per_sample
    } else {
        capture_bytes
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for PipeWireCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let node_name = self
            .node_name
            .clone()
            .unwrap_or("camilladsp-capture".to_string());
        let node_description = self
            .node_description
            .clone()
            .unwrap_or("CamillaDSP Capture".to_string());
        let node_group_name = self
            .node_group_name
            .clone()
            .unwrap_or("camilladsp".to_string());
        let autoconnect_to = self.autoconnect_to.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let binary_format = BinarySampleFormat::F32_LE;
        let store_bytes_per_sample = binary_format.bytes_per_sample();
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;

        let handle = thread::Builder::new()
            .name("PipeWireCapture".to_string())
            .spawn(move || {
                // Initialize PipeWire
                pw::init();

                let mainloop = match pw::main_loop::MainLoop::new(None) {
                    Ok(ml) => ml,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire main loop: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                let context = match pw::context::Context::new(&mainloop) {
                    Ok(ctx) => ctx,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire context: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                let core = match context.connect(None) {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = format!("Failed to connect to PipeWire: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                // Node properties for WirePlumber matching
                // NODE_LATENCY requests PipeWire to use a quantum matching our chunksize
                let latency_str = format!("{}/{}", chunksize, capture_samplerate);
                let mut props = properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Capture",
                    *pw::keys::MEDIA_ROLE => "DSP",
                    *pw::keys::APP_NAME => "CamillaDSP",
                    *pw::keys::NODE_NAME => node_name,
                    *pw::keys::NODE_DESCRIPTION => node_description,
                    *pw::keys::NODE_LATENCY => latency_str,
                    *pw::keys::NODE_GROUP => node_group_name,
                };
                if let Some(ref target) = autoconnect_to {
                    // the key PW_KEY_TARGET_OBJECT doesn not (yet?) exist in pw::keys
                    props.insert("target.object", target.as_str());
                }

                let stream = match Stream::new(&core, "CamillaDSP-Capture", props) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = format!("Failed to create PipeWire stream: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                // Ring buffer for lock-free, allocation-free capture data transfer
                let ring_size = 4 * chunksize * channels * store_bytes_per_sample;
                let ringbuffer = HeapRb::<u8>::new(ring_size);
                let (rb_producer, mut rb_consumer) = ringbuffer.split();
                let rb_producer = Rc::new(RefCell::new(rb_producer));

                // Notification channel to wake up processing thread when data is available
                let (notify_tx, notify_rx) = crossbeam_channel::bounded::<()>(1);

                let exit_flag = Arc::new(AtomicBool::new(false));
                let exit_flag_clone = exit_flag.clone();
                let mainloop_clone = mainloop.clone();

                // Set up stream listener for capture
                let rb_producer_clone = rb_producer.clone();
                let notify_tx_clone = notify_tx.clone();
                let _listener = stream
                    .add_local_listener_with_user_data(())
                    .state_changed(move |_, _, old, new| {
                        debug!("PipeWire capture stream state: {:?} -> {:?}", old, new);
                    })
                    .process(move |stream, _| {
                        if exit_flag_clone.load(Ordering::Relaxed) {
                            mainloop_clone.quit();
                            return;
                        }

                        // Get buffer from PipeWire
                        let mut pw_buffer = match stream.dequeue_buffer() {
                            Some(b) => b,
                            None => {
                                warn!("PipeWire capture: no buffer available");
                                return;
                            }
                        };

                        let datas = pw_buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }

                        let data = &mut datas[0];
                        let chunk_data = data.chunk();
                        let offset = chunk_data.offset() as usize;
                        let size = chunk_data.size() as usize;
                        xtrace!("PW capture callback with data size {} bytes", size);

                        if size == 0 {
                            return;
                        }

                        // Get input data slice
                        let in_slice = match data.data() {
                            Some(s) => &s[offset..offset + size],
                            None => return,
                        };

                        // Push directly to ring buffer - no allocation!
                        let mut producer = rb_producer_clone.borrow_mut();
                        let pushed = producer.push_slice(in_slice);
                        if pushed < size {
                            trace!("Capture ring buffer full, dropped {} bytes", size - pushed);
                        }

                        // Notify processing thread that data is available
                        let _ = notify_tx_clone.try_send(());
                    })
                    .register();

                // Build format params with channel count and sample rate (F32LE format)
                let params_buffer = build_audio_params(
                    capture_samplerate as u32,
                    channels as u32,
                );

                // Convert the buffer to a Pod reference
                let pod = pw::spa::pod::Pod::from_bytes(&params_buffer)
                    .expect("Failed to create Pod from params buffer");

                // Connect stream - set AUTOCONNECT flag if autoconnect_to is set
                let mut flags = StreamFlags::RT_PROCESS | StreamFlags::MAP_BUFFERS;
                if autoconnect_to.is_some() {
                    flags |= StreamFlags::AUTOCONNECT;
                }

                match stream.connect(
                    Direction::Input,
                    None,
                    flags,
                    &mut [pod],
                ) {
                    Ok(_) => {
                        debug!("PipeWire capture stream connected");
                    }
                    Err(e) => {
                        let msg = format!("Failed to connect PipeWire capture stream: {:?}", e);
                        error!("{}", msg);
                        status_channel
                            .send(StatusMessage::CaptureError(msg))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                }

                // Signal ready
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                debug!("Starting PipeWire capture loop");

                // Initialize resampler
                let mut resampler = new_resampler(
                    &resampler_config,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                    processing_params.clone(),
                );

                // Spawn processing thread
                let status_channel_clone = status_channel.clone();
                let capture_status_clone = capture_status.clone();
                let quitter = MainLoopQuitter::new(&mainloop);
                let processing_handle = thread::spawn(move || {
                    let mut averager = countertimer::TimeAverage::new();
                    let mut silence_counter = countertimer::SilenceCounter::new(
                        silence_threshold,
                        silence_timeout,
                        capture_samplerate,
                        chunksize,
                    );
                    let mut value_range;
                    let mut rate_adjust = 0.0;
                    let mut state = ProcessingState::Running;
                    let mut chunk_stats = ChunkStats {
                        rms: vec![0.0; channels],
                        peak: vec![0.0; channels],
                    };
                    let mut channel_mask = vec![true; channels];
                    let chunksize_bytes = channels * chunksize * store_bytes_per_sample;
                    // Pre-allocated buffer for capture data (sized for max resampler input)
                    let max_capture_bytes = if resampler.is_some() {
                        // Resampler might need more input frames
                        4 * chunksize_bytes
                    } else {
                        chunksize_bytes
                    };
                    let mut data_buffer = vec![0u8; max_capture_bytes];

                    loop {
                        // Check for commands
                        match command_channel.try_recv() {
                            Ok(CommandMessage::Exit) => {
                                debug!("Exit message received, sending EndOfStream");
                                exit_flag.store(true, Ordering::Relaxed);
                                let msg = AudioMessage::EndOfStream;
                                channel.send(msg).unwrap();
                                status_channel_clone.send(StatusMessage::CaptureDone).unwrap();
                                break;
                            }
                            Ok(CommandMessage::SetSpeed { speed }) => {
                                rate_adjust = speed;
                                if let Some(resampl) = &mut resampler {
                                    if async_src {
                                        if resampl.resampler.set_resample_ratio_relative(speed, true).is_err() {
                                            debug!("Failed to set resampling speed to {}", speed);
                                        }
                                    } else {
                                        warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                    }
                                }
                            }
                            Err(crossbeam_channel::TryRecvError::Empty) => {}
                            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                error!("Command channel was closed");
                                break;
                            }
                        };

                        // Calculate needed bytes for resampler or direct output
                        let capture_bytes = nbr_capture_bytes(
                            &resampler,
                            chunksize_bytes,
                            channels,
                            store_bytes_per_sample,
                        );

                        let mut tries = 0;
                        // Wait until ring buffer has enough data for a complete chunk
                        // Abort wait after 50 notifications if there still is not enough data.
                        while rb_consumer.occupied_len() < capture_bytes && tries < 50 {
                            // Wait for notification from callback (with timeout)
                            let _ = notify_rx.recv_timeout(std::time::Duration::from_millis(20));
                            tries += 1;
                        }
                        if rb_consumer.occupied_len() < capture_bytes {
                            continue;
                        }

                        // Pop exactly the needed bytes into pre-allocated buffer
                        rb_consumer.pop_slice(&mut data_buffer[0..capture_bytes]);
                        averager.add_value(capture_bytes);

                        // Update channel mask from capture status
                        {
                            let status = capture_status_clone.read();
                            channel_mask.copy_from_slice(&status.used_channels);
                        }

                        // Convert to audio chunk
                        let mut chunk = buffer_to_chunk_rawbytes(
                            &data_buffer[0..capture_bytes],
                            channels,
                            &binary_format,
                            capture_bytes,
                            &channel_mask,
                            false,
                        );

                        chunk.update_stats(&mut chunk_stats);
                        value_range = chunk.maxval - chunk.minval;

                        // Update capture status
                        if let Some(capture_status) = capture_status_clone.try_upgradable_read() {
                            if averager.larger_than_millis(capture_status.update_interval as u64) {
                                let bytes_per_sec = averager.average();
                                averager.restart();
                                let measured_rate_f = bytes_per_sec / (channels * store_bytes_per_sample) as f64;
                                trace!(
                                    "Measured sample rate is {:.1} Hz, signal RMS is {:?}",
                                    measured_rate_f,
                                    capture_status.signal_rms.last_sqrt(),
                                );
                                if let Ok(mut capture_status) = RwLockUpgradableReadGuard::try_upgrade(capture_status) {
                                    capture_status.measured_samplerate = measured_rate_f as usize;
                                    capture_status.signal_range = value_range as f32;
                                    capture_status.rate_adjust = rate_adjust as f32;
                                    capture_status.state = state;
                                }
                                else {
                                    xtrace!("Capture status upgrade blocked, skip update.");
                                }
                            }
                        }
                        else {
                            xtrace!("Capture status blocked, skip update.");
                        }
                        if let Some(mut capture_status) = capture_status_clone.try_write() {
                            capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                            capture_status.signal_peak.add_record(chunk_stats.peak_linear());
                        }
                        else {
                            xtrace!("Capture status blocked, skip rms update.");
                        }

                        state = silence_counter.update(value_range);

                        if state == ProcessingState::Running {
                            if let Some(resampl) = &mut resampler {
                                resampl.resample_chunk(&mut chunk, chunksize, channels);
                            }
                            let msg = AudioMessage::Audio(chunk);
                            // Use try_send to avoid blocking if pipeline is full
                            match channel.try_send(msg) {
                                Ok(()) => {}
                                Err(crossbeam_channel::TrySendError::Full(_)) => {
                                    trace!("Capture: processing pipeline full, dropping frame");
                                }
                                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                    info!("Processing thread has already stopped.");
                                    exit_flag.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                        } else if state == ProcessingState::Paused {
                            let msg = AudioMessage::Pause;
                            if channel.send(msg).is_err() {
                                info!("Processing thread has already stopped.");
                                exit_flag.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                    }

                    capture_status_clone.write().state = ProcessingState::Inactive;
                    // Signal mainloop to quit - pw_main_loop_quit is thread-safe
                    quitter.quit();
                });

                // Run PipeWire main loop
                mainloop.run();

                // Wait for processing thread
                let _ = processing_handle.join();
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
