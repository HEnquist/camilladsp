use audiodevice::*;
use config;
use config::{ConfigError, SampleFormat};
use conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use countertimer;
use crossbeam_channel::{bounded, TryRecvError, TrySendError};
use rubato::VecResampler;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Duration;

use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{
    audio_unit_from_device_id, get_default_device_id, get_device_id_from_name,
    set_device_sample_rate,
};
use coreaudio::audio_unit::{AliveListener, AudioUnit, Element, RateListener, Scope, StreamFormat};
use coreaudio::sys::*;

use crate::{CaptureStatus, PlaybackStatus};
use CommandMessage;
use PrcFmt;
use ProcessingState;
use Res;
use StatusMessage;

#[derive(Clone, Debug)]
pub struct CoreaudioPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
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
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

fn open_coreaudio_playback(
    devname: &str,
    samplerate: usize,
    channels: usize,
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

    set_device_sample_rate(device_id, samplerate as f64)
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

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

    set_device_sample_rate(device_id, samplerate as f64)
        .map_err(|e| ConfigError::new(&format!("{}", e)))?;

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

                trace!("Build output stream");
                let mut conversion_result;
                let mut sample_queue: VecDeque<u8> =
                    VecDeque::with_capacity(16 * chunksize * blockalign);

                let (mut audio_unit, device_id) =
                    match open_coreaudio_playback(&devname, samplerate, channels) {
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
                    let curr_buffer_fill =
                        sample_queue.len() / blockalign + rx_dev.len() * chunksize;
                    buffer_fill_clone.store(curr_buffer_fill, Ordering::Relaxed);
                    Ok(())
                });
                match callback_res {
                    Ok(()) => {}
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
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
                                        "Current buffer level {}, set capture rate to {}%",
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
        trace!("Resampler needs {} frames", resampl.nbr_frames_needed());
        resampl.nbr_frames_needed()
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
                // Devices typically give around 1000 frames per buffer, set a reasonable capacity for the channel
                let channel_capacity = 8*chunksize/1024 + 1;
                debug!("Using a capture channel capacity of {} buffers.", channel_capacity);
                let (tx_dev, rx_dev) = bounded(channel_capacity);

                trace!("Build input stream");
                let (mut audio_unit, device_id) = match open_coreaudio_capture(&devname, samplerate, channels) {
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

                let callback_res = audio_unit.set_input_callback(move |args: Args| {
                    let Args {
                        num_frames, data, ..
                    } = args;
                    trace!("capture call, read {} frames", num_frames);
                    let mut new_data = vec![0u8; num_frames as usize * blockalign as usize];
                    for (databyte, bufferbyte) in data.buffer.iter().zip(new_data.iter_mut()) {
                        *bufferbyte = *databyte;
                    }

                    match tx_dev.try_send((chunk_counter, new_data)) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
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
                        Err(_) => {},
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
                        Err(_) => {
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
                    while data_queue.len() < (blockalign * capture_frames) && tries < 10 {
                        trace!("capture device needs more samples to make chunk, reading from channel");
                        match rx_dev.try_recv() {
                            Ok((chunk_nbr, data)) => {
                                trace!("got chunk, length {} bytes", data.len());
                                expected_chunk_nbr += 1;
                                if chunk_nbr > expected_chunk_nbr {
                                    warn!("Samples were dropped, missing {} buffers", chunk_nbr-expected_chunk_nbr);
                                    expected_chunk_nbr = chunk_nbr;
                                }
                                for element in data.iter() {
                                    data_queue.push_back(*element);
                                }
                            }
                            Err(TryRecvError::Empty) => {
                                thread::sleep(Duration::from_millis(50));
                            }
                            Err(err) => {
                                error!("Channel is closed");
                                channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                status_channel.send(StatusMessage::CaptureError(err.to_string())).unwrap_or(());
                                return;
                            }
                        }
                        tries += 1;
                    }
                    if data_queue.len() < (blockalign * capture_frames) {
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
                    averager.add_value(capture_frames);
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
                    watcher_averager.add_value(capture_frames);
                    if watcher_averager.larger_than_millis(rate_measure_interval)
                    {
                        let samples_per_sec = watcher_averager.get_average();
                        watcher_averager.restart();
                        let measured_rate_f = samples_per_sec;
                        debug!(
                            "Measured sample rate is {} Hz",
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
                    chunk_stats = chunk.get_stats();
                    //trace!("Capture rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
                    capture_status.write().unwrap().signal_rms = chunk_stats.rms_db();
                    capture_status.write().unwrap().signal_peak = chunk_stats.peak_db();
                    value_range = chunk.maxval - chunk.minval;
                    state = silence_counter.update(value_range);
                    if state == ProcessingState::Running {
                        if let Some(resampl) = &mut resampler {
                            let new_waves = resampl.process(&chunk.waveforms).unwrap();
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
