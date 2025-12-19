//! PipeWire audio backend for CamillaDSP
//!
//! This module provides native PipeWire support, creating filter nodes
//! in the PipeWire graph that can be connected via WirePlumber rules.
//!
//! Like PulseAudio, PipeWire uses F32 internally, so we always use F32
//! format for audio exchange - no format configuration is needed.

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
use crate::countertimer;
use crate::resampling::{new_resampler, resampler_is_async, ChunkResampler};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
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
pub struct PipewireError {
    desc: String,
}

impl std::fmt::Display for PipewireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl std::error::Error for PipewireError {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl PipewireError {
    pub fn new(desc: &str) -> Self {
        PipewireError {
            desc: format!("PipeWire error: {}", desc),
        }
    }
}

/// PipeWire playback device
pub struct PipewirePlaybackDevice {
    pub node_name: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
}

/// PipeWire capture device
pub struct PipewireCaptureDevice {
    pub node_name: String,
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
impl PlaybackDevice for PipewirePlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let node_name = self.node_name.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let binary_format = BinarySampleFormat::F32_LE;
        let store_bytes_per_sample = binary_format.bytes_per_sample();

        let handle = thread::Builder::new()
            .name("PipewirePlayback".to_string())
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
                let props = properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Playback",
                    *pw::keys::MEDIA_ROLE => "DSP",
                    *pw::keys::APP_NAME => "CamillaDSP",
                    *pw::keys::NODE_NAME => node_name.as_str(),
                    *pw::keys::NODE_DESCRIPTION => "CamillaDSP Playback",
                    *pw::keys::NODE_LATENCY => latency_str.as_str(),
                };

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
                let running = Rc::new(RefCell::new(true));
                let running_clone = running.clone();
                let buffer: Rc<RefCell<VecDeque<u8>>> = Rc::new(RefCell::new(
                    VecDeque::with_capacity(4 * chunksize * channels * store_bytes_per_sample),
                ));
                let buffer_clone = buffer.clone();
                let buffer_fill = Arc::new(AtomicUsize::new(0));
                let buffer_fill_clone = buffer_fill.clone();
                let mainloop_clone = mainloop.clone();

                // Channel for receiving audio data in the PipeWire thread
                let (tx_chunk, rx_chunk) = std::sync::mpsc::sync_channel::<AudioChunk>(2);

                // Set up stream listener
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
                        let max_frames = max_bytes / stride;

                        // Fill from internal buffer
                        let mut buf = buffer_clone.borrow_mut();

                        // Try to receive more chunks if buffer is low
                        while buf.len() < max_frames * stride {
                            match rx_chunk.try_recv() {
                                Ok(chunk) => {
                                    // Convert chunk to raw bytes and add to buffer
                                    let mut temp_buf = vec![0u8; chunk.frames * stride];
                                    chunk_to_buffer_rawbytes(chunk, &mut temp_buf, &binary_format);
                                    for byte in temp_buf {
                                        buf.push_back(byte);
                                    }
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    *running_clone.borrow_mut() = false;
                                    mainloop_clone.quit();
                                    return;
                                }
                            }
                        }

                        // Copy to output
                        let frames_to_write = (buf.len() / stride).min(max_frames);
                        let bytes_to_write = frames_to_write * stride;

                        for i in 0..bytes_to_write {
                            if let Some(byte) = buf.pop_front() {
                                out_slice[i] = byte;
                            } else {
                                out_slice[i] = 0;
                            }
                        }

                        // CRITICAL: Tell PipeWire how much data we wrote
                        // For output streams, we must set chunk offset, size, and stride
                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.size_mut() = bytes_to_write as u32;
                        *chunk.stride_mut() = stride as i32;

                        buffer_fill_clone.store(buf.len() / stride, Ordering::Relaxed);
                    })
                    .register();

                // Build format params with channel count and sample rate (F32LE format)
                let params_buffer = build_audio_params(
                    samplerate as u32,
                    channels as u32,
                );

                // Convert the buffer to a Pod reference
                let pod = pw::spa::pod::Pod::from_bytes(&params_buffer)
                    .expect("Failed to create Pod from params buffer");

                // Connect stream - NO AUTOCONNECT, let WirePlumber handle routing
                // DRIVER flag ensures process callback is called regularly even when not connected
                let flags = StreamFlags::RT_PROCESS | StreamFlags::MAP_BUFFERS | StreamFlags::DRIVER;

                match stream.connect(
                    Direction::Output,
                    None,  // No target - WirePlumber handles routing
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
                let receiver_handle = thread::spawn(move || {
                    let mut chunk_stats = ChunkStats {
                        rms: vec![0.0; channels],
                        peak: vec![0.0; channels],
                    };

                    loop {
                        match channel.recv() {
                            Ok(AudioMessage::Audio(chunk)) => {
                                chunk.update_stats(&mut chunk_stats);
                                {
                                    let mut playback_status = playback_status_clone.write();
                                    playback_status
                                        .signal_rms
                                        .add_record_squared(chunk_stats.rms_linear());
                                    playback_status
                                        .signal_peak
                                        .add_record(chunk_stats.peak_linear());
                                }
                                // Use try_send - if buffer is full, drop the frame
                                // This handles the case where playback isn't connected yet
                                // (PipeWire won't call process callback if nothing is receiving)
                                match tx_chunk.try_send(chunk) {
                                    Ok(()) => {}
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        // Buffer full - playback not connected or not consuming
                                        // Just drop the frame and continue
                                        trace!("Playback buffer full, dropping frame (playback may not be connected)");
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        debug!("Playback tx_chunk disconnected, exiting");
                                        break;
                                    }
                                }
                            }
                            Ok(AudioMessage::Pause) => {
                                trace!("Pause message received");
                            }
                            Ok(AudioMessage::EndOfStream) => {
                                status_channel_clone.send(StatusMessage::PlaybackDone).unwrap();
                                break;
                            }
                            Err(err) => {
                                error!("Message channel error: {}", err);
                                status_channel_clone.send(StatusMessage::PlaybackDone).unwrap();
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
impl CaptureDevice for PipewireCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        _processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let node_name = self.node_name.clone();
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
            .name("PipewireCapture".to_string())
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
                let props = properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Capture",
                    *pw::keys::MEDIA_ROLE => "DSP",
                    *pw::keys::APP_NAME => "CamillaDSP",
                    *pw::keys::NODE_NAME => node_name.as_str(),
                    *pw::keys::NODE_DESCRIPTION => "CamillaDSP Capture",
                    *pw::keys::NODE_LATENCY => latency_str.as_str(),
                };

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

                // Channel to send captured audio to main processing
                let (tx_raw, rx_raw) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
                let exit_flag = Arc::new(AtomicBool::new(false));
                let exit_flag_clone = exit_flag.clone();
                let mainloop_clone = mainloop.clone();

                // Set up stream listener for capture
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

                        if size == 0 {
                            return;
                        }

                        // Get input data slice
                        let in_slice = match data.data() {
                            Some(s) => &s[offset..offset + size],
                            None => return,
                        };

                        // Send raw bytes to processing thread
                        let raw_data = in_slice.to_vec();
                        if tx_raw.try_send(raw_data).is_err() {
                            trace!("Capture buffer full, dropping frame");
                        }
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

                // Connect stream - NO AUTOCONNECT, let WirePlumber handle routing
                // DRIVER flag ensures process callback is called regularly even when not connected
                let flags = StreamFlags::RT_PROCESS | StreamFlags::MAP_BUFFERS | StreamFlags::DRIVER;

                match stream.connect(
                    Direction::Input,
                    None,  // No target - WirePlumber handles routing
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
                    #[allow(unused_assignments)]
                    let mut value_range = 0.0;
                    let mut rate_adjust = 0.0;
                    let mut state = ProcessingState::Running;
                    let mut chunk_stats = ChunkStats {
                        rms: vec![0.0; channels],
                        peak: vec![0.0; channels],
                    };
                    let mut channel_mask = vec![true; channels];
                    let chunksize_bytes = channels * chunksize * store_bytes_per_sample;
                    let mut accumulated_buf: Vec<u8> = Vec::with_capacity(chunksize_bytes * 2);

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

                        // Receive raw audio data from PipeWire callback
                        let raw_data = match rx_raw.recv_timeout(std::time::Duration::from_millis(100)) {
                            Ok(data) => data,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                debug!("Capture channel disconnected");
                                break;
                            }
                        };

                        // Accumulate data
                        accumulated_buf.extend_from_slice(&raw_data);
                        averager.add_value(raw_data.len());

                        // Calculate needed bytes for resampler or direct output
                        let capture_bytes = nbr_capture_bytes(
                            &resampler,
                            chunksize_bytes,
                            channels,
                            store_bytes_per_sample,
                        );

                        // Process complete chunks
                        while accumulated_buf.len() >= capture_bytes {
                            // Update channel mask from capture status
                            {
                                let status = capture_status_clone.read();
                                channel_mask.copy_from_slice(&status.used_channels);
                            }

                            // Convert to audio chunk
                            let mut chunk = buffer_to_chunk_rawbytes(
                                &accumulated_buf[0..capture_bytes],
                                channels,
                                &binary_format,
                                capture_bytes,
                                &channel_mask,
                                false,
                            );

                            // Remove processed bytes
                            accumulated_buf.drain(0..capture_bytes);

                            chunk.update_stats(&mut chunk_stats);
                            value_range = chunk.maxval - chunk.minval;

                            // Update capture status
                            {
                                let capture_status = capture_status_clone.upgradable_read();
                                if averager.larger_than_millis(capture_status.update_interval as u64) {
                                    let bytes_per_sec = averager.average();
                                    averager.restart();
                                    let measured_rate_f = bytes_per_sec / (channels * store_bytes_per_sample) as f64;
                                    trace!(
                                        "Measured sample rate is {:.1} Hz, signal RMS is {:?}",
                                        measured_rate_f,
                                        capture_status.signal_rms.last_sqrt(),
                                    );
                                    let mut capture_status = RwLockUpgradableReadGuard::upgrade(capture_status);
                                    capture_status.measured_samplerate = measured_rate_f as usize;
                                    capture_status.signal_range = value_range as f32;
                                    capture_status.rate_adjust = rate_adjust as f32;
                                    capture_status.state = state;
                                }
                            }
                            {
                                let mut capture_status = capture_status_clone.write();
                                capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                                capture_status.signal_peak.add_record(chunk_stats.peak_linear());
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
