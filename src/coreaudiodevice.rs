use crate::audiodevice::*;
use crate::config;
use crate::config::{ConfigError, SampleFormat};
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use crossbeam_channel::{bounded, TryRecvError, TrySendError};
use dispatch::Semaphore;
use rubato::VecResampler;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Duration;

use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::macos_helpers::{
    audio_unit_from_device_id, find_matching_physical_format, get_default_device_id,
    get_device_id_from_name, get_hogging_pid, set_device_physical_stream_format,
    set_device_sample_rate, toggle_hog_mode, AliveListener, RateListener,
};
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, Scope, StreamFormat};
use coreaudio::sys::*;

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

fn take_ownership(device_id: AudioDeviceID) -> Res<pid_t> {
    let mut device_pid =
        get_hogging_pid(device_id).map_err(|e| ConfigError::new(&format!("{}", e)))?;
    let camilla_pid = std::process::id() as pid_t;
    if device_pid == camilla_pid {
        debug!("We already have exclusive access.");
    } else if device_pid != -1 {
        warn!(
            "Device is owned by another process with pid {}!",
            device_pid
        );
    } else {
        debug!("Device is free, trying to get exclusive access.");
        device_pid = toggle_hog_mode(device_id).map_err(|e| ConfigError::new(&format!("{}", e)))?;
        if device_pid == camilla_pid {
            debug!("We have exclusive access.");
        } else {
            warn!(
                "Could not get exclusive access. CamillaDSP pid: {}, device owner pid: {}",
                camilla_pid, device_pid
            );
        }
    }
    Ok(device_pid)
}

fn release_ownership(device_id: AudioDeviceID) -> Res<()> {
    let device_owner_pid =
        get_hogging_pid(device_id).map_err(|e| ConfigError::new(&format!("{}", e)))?;
    let camilla_pid = std::process::id() as pid_t;
    if device_owner_pid == camilla_pid {
        debug!("Releasing exclusive access.");
        let new_device_pid =
            toggle_hog_mode(device_id).map_err(|e| ConfigError::new(&format!("{}", e)))?;
        if new_device_pid == -1 {
            debug!("Exclusive access released.");
        } else {
            warn!(
                "Could not release exclusive access. CamillaDSP pid: {}, device owner pid: {}",
                camilla_pid, new_device_pid
            );
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct CoreaudioPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub change_format: bool,
    pub exclusive: bool,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

#[derive(Clone, Debug)]
pub struct CoreaudioCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub resampler_conf: config::Resampler,
    pub enable_resampling: bool,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub change_format: bool,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

fn open_coreaudio_playback(
    devname: &str,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
    change_format: bool,
    exclusive: bool,
) -> Res<(AudioUnit, AudioDeviceID)> {
    let device_id = if devname == "default" {
        match get_default_device_id(false) {
            Some(dev) => dev,
            None => {
                let msg = "Could not get default playback device".to_string();
                return Err(ConfigError::new(&msg).into());
            }
        }
    } else {
        match get_device_id_from_name(devname) {
            Some(dev) => dev,
            None => {
                let msg = format!("Could not find playback device '{}'", devname);
                return Err(ConfigError::new(&msg).into());
            }
        }
    };

    let mut audio_unit = audio_unit_from_device_id(device_id, false)
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

    if exclusive {
        take_ownership(device_id)?;
    } else {
        release_ownership(device_id)?;
    }

    if change_format {
        let phys_format = match *sample_format {
            SampleFormat::S16LE => coreaudio::audio_unit::SampleFormat::I16,
            SampleFormat::S24LE | SampleFormat::S24LE3 => coreaudio::audio_unit::SampleFormat::I24,
            SampleFormat::S32LE => coreaudio::audio_unit::SampleFormat::I32,
            SampleFormat::FLOAT32LE => coreaudio::audio_unit::SampleFormat::F32,
            _ => {
                let msg = format!("Sample format '{}' not supported!", sample_format);
                return Err(ConfigError::new(&msg).into());
            }
        };

        let physical_stream_format = StreamFormat {
            sample_rate: samplerate as f64,
            sample_format: phys_format,
            flags: LinearPcmFlags::empty(),
            channels: channels as u32,
        };

        if let Some(phys_asbd) = find_matching_physical_format(device_id, physical_stream_format) {
            debug!("Set phys playback stream format");
            set_device_physical_stream_format(device_id, phys_asbd).map_err(|_| {
                ConfigError::new("Failed to find matching physical playback format")
            })?;
        } else {
            let msg = "Failed to find matching physical playback format";
            return Err(ConfigError::new(msg).into());
        }
    } else {
        set_device_sample_rate(device_id, samplerate as f64)
            .map_err(|e| ConfigError::new(&format!("{}", e)))?;
    }

    let stream_format = StreamFormat {
        sample_rate: samplerate as f64,
        sample_format: coreaudio::audio_unit::SampleFormat::F32,
        flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
        channels: channels as u32,
    };

    let id = kAudioUnitProperty_StreamFormat;
    let asbd = stream_format.to_asbd();
    audio_unit
        .set_property(id, Scope::Input, Element::Output, Some(&asbd))
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

    debug!("Opened CoreAudio playback device {}", devname);
    Ok((audio_unit, device_id))
}

fn open_coreaudio_capture(
    devname: &str,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
    change_format: bool,
) -> Res<(AudioUnit, AudioDeviceID)> {
    let device_id = if devname == "default" {
        match get_default_device_id(true) {
            Some(dev) => dev,
            None => {
                let msg = "Could not get default capture device".to_string();
                return Err(ConfigError::new(&msg).into());
            }
        }
    } else {
        match get_device_id_from_name(devname) {
            Some(dev) => dev,
            None => {
                let msg = format!("Could not find capture device '{}'", devname);
                return Err(ConfigError::new(&msg).into());
            }
        }
    };

    let mut audio_unit = audio_unit_from_device_id(device_id, true)
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

    if change_format {
        let phys_format = match *sample_format {
            SampleFormat::S16LE => coreaudio::audio_unit::SampleFormat::I16,
            SampleFormat::S24LE | SampleFormat::S24LE3 => coreaudio::audio_unit::SampleFormat::I24,
            SampleFormat::S32LE => coreaudio::audio_unit::SampleFormat::I32,
            SampleFormat::FLOAT32LE => coreaudio::audio_unit::SampleFormat::F32,
            _ => {
                let msg = format!("Sample format '{}' not supported!", sample_format);
                return Err(ConfigError::new(&msg).into());
            }
        };

        let physical_stream_format = StreamFormat {
            sample_rate: samplerate as f64,
            sample_format: phys_format,
            flags: LinearPcmFlags::empty(),
            channels: channels as u32,
        };

        if let Some(phys_asbd) = find_matching_physical_format(device_id, physical_stream_format) {
            debug!("Set phys capture stream format");
            set_device_physical_stream_format(device_id, phys_asbd)
                .map_err(|_| ConfigError::new("Failed to find matching physical capture format"))?;
        } else {
            let msg = "Failed to find matching physical capture format";
            return Err(ConfigError::new(msg).into());
        }
    } else {
        set_device_sample_rate(device_id, samplerate as f64)
            .map_err(|e| ConfigError::new(&format!("{}", e)))?;
    }

    debug!("Set capture stream format");
    let stream_format = StreamFormat {
        sample_rate: samplerate as f64,
        sample_format: coreaudio::audio_unit::SampleFormat::F32,
        flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
        channels: channels as u32,
    };

    let id = kAudioUnitProperty_StreamFormat;
    let asbd = stream_format.to_asbd();
    audio_unit
        .set_property(id, Scope::Output, Element::Input, Some(&asbd))
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

    debug!("Opened CoreAudio capture device {}", devname);
    Ok((audio_unit, device_id))
}

enum PlaybackDeviceMessage {
    Data(Vec<u8>),
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for CoreaudioPlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let sample_format = self.sample_format.clone();
        let change_format = self.change_format;
        let exclusive = self.exclusive;
        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let adjust = self.adjust_period > 0.0 && self.enable_rate_adjust;
        let handle = thread::Builder::new()
            .name("CoreaudioPlayback".to_string())
            .spawn(move || {
                // Devices typically request around 1000 frames per buffer, set a reasonable capacity for the channel
                let channel_capacity = 8 * 1024 / chunksize + 1;
                debug!(
                    "Using a playback channel capacity of {} chunks.",
                    channel_capacity
                );
                let (tx_dev, rx_dev) = bounded(channel_capacity);
                let buffer_fill = Arc::new(AtomicUsize::new(0));
                let buffer_fill_clone = buffer_fill.clone();
                let mut buffer_avg = countertimer::Averager::new();
                let mut timer = countertimer::Stopwatch::new();
                let mut chunk_stats;
                let blockalign = 4 * channels;
                // Rough guess of the number of frames per callback.
                let callback_frames = 512;
                // TODO check if always 512!
                //trace!("Estimated playback callback period to {} frames", callback_frames);

                trace!("Build output stream");
                let mut conversion_result;
                let mut sample_queue: VecDeque<u8> =
                    VecDeque::with_capacity(16 * chunksize * blockalign);

                let (mut audio_unit, device_id) = match open_coreaudio_playback(
                    &devname,
                    samplerate,
                    channels,
                    &sample_format,
                    change_format,
                    exclusive,
                ) {
                    Ok(audio_unit) => audio_unit,
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };

                type Args = render_callback::Args<data::InterleavedBytes<f32>>;

                let mut running = true;

                let callback_res = audio_unit.set_render_callback(move |args: Args| {
                    let Args {
                        num_frames, data, ..
                    } = args;
                    trace!("playback cb called with {} frames", num_frames);
                    while sample_queue.len() < (blockalign as usize * num_frames as usize) {
                        trace!("playback loop needs more samples, reading from channel");
                        match rx_dev.try_recv() {
                            Ok(PlaybackDeviceMessage::Data(chunk)) => {
                                trace!("got chunk");
                                for element in chunk.iter() {
                                    sample_queue.push_back(*element);
                                }
                                if !running {
                                    running = true;
                                    info!("Restarting playback after buffer underrun");
                                }
                            }
                            Err(_) => {
                                for _ in 0..((blockalign as usize * num_frames as usize)
                                    - sample_queue.len())
                                {
                                    sample_queue.push_back(0);
                                }
                                if running {
                                    running = false;
                                    warn!("Playback interrupted, no data available");
                                }
                            }
                        }
                    }
                    for bufferbyte in data.buffer.iter_mut() {
                        let byte = sample_queue.pop_front().unwrap_or(0);
                        *bufferbyte = byte;
                    }
                    let mut curr_buffer_fill =
                        sample_queue.len() / blockalign + rx_dev.len() * chunksize;
                    // Reduce the measured buffer fill by approximtely one callback size
                    // to force a larger.
                    if curr_buffer_fill > callback_frames {
                        curr_buffer_fill -= callback_frames;
                    } else {
                        curr_buffer_fill = 0;
                    }
                    buffer_fill_clone.store(curr_buffer_fill, Ordering::Relaxed);
                    Ok(())
                });
                match callback_res {
                    Ok(()) => {}
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
                        release_ownership(device_id).unwrap_or(());
                        barrier.wait();
                        return;
                    }
                }

                let mut alive_listener = AliveListener::new(device_id);
                if let Err(err) = alive_listener.register() {
                    warn!(
                        "Unable to register playback device alive listener, error: {}",
                        err
                    );
                }

                match status_channel.send(StatusMessage::PlaybackReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                debug!("Playback device ready and waiting");
                barrier.wait();
                debug!("Playback device starts now!");
                match audio_unit.start() {
                    Ok(()) => {}
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
                        release_ownership(device_id).unwrap_or(());
                        return;
                    }
                }
                'deviceloop: loop {
                    if !alive_listener.is_alive() {
                        error!("Playback device is no longer alive");
                        status_channel
                            .send(StatusMessage::PlaybackError(
                                "Playback device is no longer alive".to_string(),
                            ))
                            .unwrap_or(());
                        break 'deviceloop;
                    }
                    match channel.recv() {
                        Ok(AudioMessage::Audio(chunk)) => {
                            buffer_avg.add_value(buffer_fill.load(Ordering::Relaxed) as f64);
                            if adjust && timer.larger_than_millis((1000.0 * adjust_period) as u64) {
                                if let Some(av_delay) = buffer_avg.get_average() {
                                    let speed = calculate_speed(
                                        av_delay,
                                        target_level,
                                        adjust_period,
                                        samplerate as u32,
                                    );
                                    timer.restart();
                                    buffer_avg.restart();
                                    debug!(
                                        "Current buffer level {:.1}, set capture rate to {:.4}%",
                                        av_delay,
                                        100.0 * speed
                                    );
                                    status_channel
                                        .send(StatusMessage::SetSpeed(speed))
                                        .unwrap_or(());
                                    playback_status.write().unwrap().buffer_level =
                                        av_delay as usize;
                                }
                            }
                            chunk_stats = chunk.get_stats();
                            playback_status.write().unwrap().signal_rms = chunk_stats.rms_db();
                            playback_status.write().unwrap().signal_peak = chunk_stats.peak_db();
                            let mut buf = vec![
                                0u8;
                                channels
                                    * chunk.frames
                                    * SampleFormat::FLOAT32LE.bytes_per_sample()
                            ];
                            conversion_result = chunk_to_buffer_rawbytes(
                                &chunk,
                                &mut buf,
                                &SampleFormat::FLOAT32LE,
                            );
                            match tx_dev.send(PlaybackDeviceMessage::Data(buf)) {
                                Ok(_) => {}
                                Err(err) => {
                                    error!("Playback device channel error: {}", err);
                                    status_channel
                                        .send(StatusMessage::PlaybackError(err.to_string()))
                                        .unwrap_or(());
                                    break;
                                }
                            }
                            if conversion_result.1 > 0 {
                                playback_status.write().unwrap().clipped_samples +=
                                    conversion_result.1;
                            }
                        }
                        Ok(AudioMessage::Pause) => {
                            trace!("Pause message received");
                        }
                        Ok(AudioMessage::EndOfStream) => {
                            status_channel
                                .send(StatusMessage::PlaybackDone)
                                .unwrap_or(());
                            break;
                        }
                        Err(err) => {
                            error!("Message channel error: {}", err);
                            status_channel
                                .send(StatusMessage::PlaybackError(err.to_string()))
                                .unwrap_or(());
                            break;
                        }
                    }
                }
                release_ownership(device_id).unwrap_or(());
            })?;
        Ok(Box::new(handle))
    }
}

fn get_nbr_capture_frames(
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    capture_frames: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        #[cfg(feature = "debug")]
        trace!("Resampler needs {} frames", resampl.input_frames_next());
        resampl.input_frames_next()
    } else {
        capture_frames
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for CoreaudioCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let sample_format = self.sample_format.clone();
        let change_format = self.change_format;
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
        let async_src = resampler_is_async(&resampler_conf);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = (1000.0 * self.rate_measure_interval) as u64;
        let blockalign = 4 * channels;

        let handle = thread::Builder::new()
            .name("CoreaudioCapture".to_string())
            .spawn(move || {
                let mut resampler = if enable_resampling {
                    debug!("Creating resampler");
                    get_resampler(
                        &resampler_conf,
                        channels,
                        samplerate,
                        capture_samplerate,
                        chunksize,
                    )
                } else {
                    None
                };
                // Rough guess of the number of frames per callback. 
                //let callback_frames = samplerate / 85;
                let callback_frames = 512;
                // TODO check if always 512!
                //trace!("Estimated playback callback period to {} frames", callback_frames);
                let channel_capacity = 8*chunksize/callback_frames + 1;
                debug!("Using a capture channel capacity of {} buffers.", channel_capacity);
                let (tx_dev, rx_dev) = bounded(channel_capacity);
                let (tx_dev_free, rx_dev_free) = bounded(channel_capacity+2);
                for _ in 0..(channel_capacity+2) {
                    let data = vec![0u8; 4*callback_frames*blockalign];
                    tx_dev_free.send(data).unwrap();
                }

                // Semaphore used to wake up the waiting capture thread from the callback.
                let semaphore = Semaphore::new(0);
                let device_sph = semaphore.clone();

                trace!("Build input stream");
                let (mut audio_unit, device_id) = match open_coreaudio_capture(&devname, capture_samplerate, channels, &sample_format, change_format) {
                    Ok(audio_unit) => audio_unit,
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::CaptureError(err.to_string()))
                            .unwrap_or(());
                        barrier.wait();
                        return;
                    }
                };
                let mut chunk_counter = 0;

                type Args = render_callback::Args<data::InterleavedBytes<f32>>;

                // Vec used to store the saved buffer between callback iterations. 
                let mut saved_buffer: Vec<Vec<u8>> = Vec::new();

                let callback_res = audio_unit.set_input_callback(move |args: Args| {
                    let Args {
                        num_frames, data, ..
                    } = args;
                    trace!("capture call, read {} frames", num_frames);
                    let mut new_data = match saved_buffer.len() {
                        0 => rx_dev_free.recv().unwrap(),
                        _ => saved_buffer.pop().unwrap(),
                    };
                    let length_bytes = data.buffer.len();
                    if length_bytes > new_data.len() {
                        debug!("Buffer is too small, resizing from {} to {}", new_data.len(), length_bytes);
                        new_data.resize(length_bytes, 0);
                    }
                    for (databyte, bufferbyte) in data.buffer.iter().zip(new_data.iter_mut()) {
                        *bufferbyte = *databyte;
                    }
                    match tx_dev.try_send((chunk_counter, length_bytes, new_data)) {
                        Ok(()) => {
                            device_sph.signal();
                        },
                        Err(TrySendError::Full((nbr, length_bytes, buf))) => {
                            debug!("Dropping captured chunk {} with len {}", nbr, length_bytes);
                            saved_buffer.push(buf);
                        }
                        Err(_) => {
                            error!("Error sending, channel disconnected");
                        }
                    }
                    chunk_counter += 1;
                    Ok(())
                });

                match callback_res {
                    Ok(()) => {},
                    Err(err) => {
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel.send(StatusMessage::CaptureError(err.to_string() )).unwrap_or(());
                        barrier.wait();
                        return;
                    },
                }
                let (rate_tx, rate_rx) = mpsc::channel();
                let mut rate_listener = RateListener::new(device_id, Some(rate_tx));
                if let Err(err) = rate_listener.register() {
                    warn!("Unable to register capture rate listener, error: {}", err);
                }
                let mut alive_listener = AliveListener::new(device_id);
                if let Err(err) = alive_listener.register() {
                    warn!("Unable to register capture device alive listener, error: {}", err);
                }

                let chunksize_samples = channels * chunksize;
                let mut capture_frames = chunksize;
                capture_frames = get_nbr_capture_frames(
                    &resampler,
                    capture_frames,
                );
                let mut averager = countertimer::TimeAverage::new();
                let mut watcher_averager = countertimer::TimeAverage::new();
                let mut valuewatcher = countertimer::ValueWatcher::new(capture_samplerate as f32, RATE_CHANGE_THRESHOLD_VALUE, RATE_CHANGE_THRESHOLD_COUNT);
                let mut value_range = 0.0;
                let mut chunk_stats;
                let mut rate_adjust = 0.0;
                let mut silence_counter = countertimer::SilenceCounter::new(silence_threshold, silence_timeout, capture_samplerate, chunksize);
                let mut state = ProcessingState::Running;
                let blockalign = 4*channels;
                let mut data_queue: VecDeque<u8> = VecDeque::with_capacity(4 * blockalign * chunksize_samples );
                // TODO check if this ever needs to be resized
                let mut data_buffer = vec![0u8; 4 * blockalign * capture_frames];
                let mut expected_chunk_nbr = 0;
                let mut prev_len = 0;
                debug!("Capture device ready and waiting");
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                debug!("Capture device starts now!");
                match audio_unit.start() {
                    Ok(()) => {},
                    Err(err) => {
                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                        status_channel.send(StatusMessage::CaptureError(err.to_string() )).unwrap_or(());
                        return;
                    },
                }
                'deviceloop: loop {
                    match command_channel.try_recv() {
                        Ok(CommandMessage::Exit) => {
                            debug!("Exit message received, sending EndOfStream");
                            let msg = AudioMessage::EndOfStream;
                            channel.send(msg).unwrap_or(());
                            status_channel.send(StatusMessage::CaptureDone).unwrap_or(());
                            break;
                        }
                        Ok(CommandMessage::SetSpeed { speed }) => {
                            rate_adjust = speed;
                            debug!("Requested to adjust capture speed to {}", speed);
                            if let Some(resampl) = &mut resampler {
                                debug!("Adjusting resampler rate to {}", speed);
                                if async_src {
                                    if resampl.set_resample_ratio_relative(speed).is_err() {
                                        debug!("Failed to set resampling speed to {}", speed);
                                    }
                                }
                                else {
                                    warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                }
                            }
                        },
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => {
                            error!("Command channel was closed");
                            break;
                        }
                    }
                    match rate_rx.try_recv() {
                        Ok(rate) => {
                            debug!("Capture rate change event, new rate: {}",rate);
                            if rate as usize != capture_samplerate {
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                status_channel.send(StatusMessage::CaptureFormatChange(rate as usize)).unwrap_or(());
                                break;
                            }
                        },
                        Err(mpsc::TryRecvError::Empty) => {}
                        Err(mpsc::TryRecvError::Disconnected) => {
                            error!("Rate event queue closed!");
                            channel.send(AudioMessage::EndOfStream).unwrap_or(());
                            status_channel.send(StatusMessage::CaptureError("Rate listener channel closed".to_string())).unwrap_or(());
                            break;
                        }
                    }
                    if !alive_listener.is_alive() {
                        error!("Capture device is no longer alive");
                        status_channel.send(StatusMessage::CaptureError("Capture device is no longer alive".to_string())).unwrap_or(());
                        break 'deviceloop;
                    }
                    capture_frames = get_nbr_capture_frames(
                        &resampler,
                        capture_frames,
                    );
                    let capture_bytes = blockalign * capture_frames;
                    let mut tries = 0;
                    while data_queue.len() < (blockalign * capture_frames) && tries < 50 {
                        trace!("capture device needs more samples to make chunk, reading from channel");
                        let _ = semaphore.wait_timeout(Duration::from_millis(20));
                        match rx_dev.try_recv() {
                            Ok((chunk_nbr, length_bytes, data)) => {
                                trace!("got chunk, length {} bytes", length_bytes);
                                expected_chunk_nbr += 1;
                                if chunk_nbr > expected_chunk_nbr {
                                    warn!("Samples were dropped, missing {} buffers", chunk_nbr-expected_chunk_nbr);
                                    expected_chunk_nbr = chunk_nbr;
                                }
                                for element in data.iter().take(length_bytes) {
                                    data_queue.push_back(*element);
                                }
                                // Return the buffer to the queue
                                tx_dev_free.send(data).unwrap();
                            }
                            Err(TryRecvError::Empty) => {
                                trace!("No new data from inner capture thread, try {} of 50", tries);
                            }
                            Err(TryRecvError::Disconnected) => {
                                error!("Channel is closed");
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                status_channel.send(StatusMessage::CaptureError("Inner capture thread has exited".to_string())).unwrap_or(());
                                return;
                            }
                        }
                        tries += 1;
                    }
                    if data_queue.len() < (blockalign * capture_frames) {
                        let mut capture_status = capture_status.write().unwrap();
                        capture_status.measured_samplerate = 0;
                        capture_status.signal_range = 0.0;
                        capture_status.rate_adjust = 0.0;
                        capture_status.state = ProcessingState::Stalled;
                        let msg = AudioMessage::Pause;
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break;
                        }
                        continue;
                    }

                    for element in data_buffer.iter_mut().take(capture_bytes) {
                        *element = data_queue.pop_front().unwrap();
                    }
                    let mut chunk = buffer_to_chunk_rawbytes(
                        &data_buffer[0..capture_bytes],
                        channels,
                        &SampleFormat::FLOAT32LE,
                        capture_bytes,
                        &capture_status.read().unwrap().used_channels,
                    );
                    averager.add_value(capture_frames + data_queue.len()/blockalign - prev_len/blockalign);
                    if averager.larger_than_millis(capture_status.read().unwrap().update_interval as u64)
                    {
                        let samples_per_sec = averager.get_average();
                        averager.restart();
                        let measured_rate_f = samples_per_sec;
                        debug!(
                            "Measured sample rate is {} Hz",
                            measured_rate_f
                        );
                        let mut capture_status = capture_status.write().unwrap();
                        capture_status.measured_samplerate = measured_rate_f as usize;
                        capture_status.signal_range = value_range as f32;
                        capture_status.rate_adjust = rate_adjust as f32;
                        capture_status.state = state;
                    }
                    watcher_averager.add_value(capture_frames + data_queue.len()/blockalign - prev_len/blockalign);
                    if watcher_averager.larger_than_millis(rate_measure_interval)
                    {
                        let samples_per_sec = watcher_averager.get_average();
                        watcher_averager.restart();
                        let measured_rate_f = samples_per_sec;
                        debug!(
                            "Rate watcher, measured sample rate is {} Hz",
                            measured_rate_f
                        );
                        let changed = valuewatcher.check_value(measured_rate_f as f32);
                        if changed {
                            warn!("sample rate change detected, last rate was {} Hz", measured_rate_f);
                            if stop_on_rate_change {
                                let msg = AudioMessage::EndOfStream;
                                channel.send(msg).unwrap_or(());
                                status_channel.send(StatusMessage::CaptureFormatChange(measured_rate_f as usize)).unwrap_or(());
                                break;
                            }
                        }
                    }
                    prev_len = data_queue.len();
                    chunk_stats = chunk.get_stats();
                    //trace!("Capture rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
                    capture_status.write().unwrap().signal_rms = chunk_stats.rms_db();
                    capture_status.write().unwrap().signal_peak = chunk_stats.peak_db();
                    value_range = chunk.maxval - chunk.minval;
                    state = silence_counter.update(value_range);
                    if state == ProcessingState::Running {
                        if let Some(resampl) = &mut resampler {
                            let new_waves = resampl.process(&chunk.waveforms, None).unwrap();
                            let mut chunk_frames = new_waves.iter().map(|w| w.len()).max().unwrap();
                            if chunk_frames == 0 {
                                chunk_frames = chunksize;
                            }
                            chunk.frames = chunk_frames;
                            chunk.valid_frames = chunk.frames;
                            chunk.waveforms = new_waves;
                        }
                        let msg = AudioMessage::Audio(chunk);
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break;
                        }
                    }
                    else if state == ProcessingState::Paused {
                        let msg = AudioMessage::Pause;
                        if channel.send(msg).is_err() {
                            info!("Processing thread has already stopped.");
                            break;
                        }
                    }
                }
                let mut capt_stat = capture_status.write().unwrap();
                capt_stat.state = ProcessingState::Inactive;
            })?;
        Ok(Box::new(handle))
    }
}
