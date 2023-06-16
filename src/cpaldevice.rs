use crate::audiodevice::*;
use crate::config;
use crate::config::{ConfigError, SampleFormat};
use crate::conversions::{
    chunk_to_queue_float, chunk_to_queue_int, queue_to_chunk_float, queue_to_chunk_int,
};
use crate::countertimer;
use cpal;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Device;
use cpal::{BufferSize, ChannelCount, HostId, SampleRate, StreamConfig};
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rubato::VecResampler;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time;

use crate::CommandMessage;
use crate::NewValue;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

#[derive(Clone, Debug)]
pub enum CpalHost {
    #[cfg(target_os = "macos")]
    CoreAudio,
    #[cfg(all(
        feature = "jack-backend",
        any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd"
        )
    ))]
    Jack,
}

#[derive(Clone, Debug)]
pub struct CpalPlaybackDevice {
    pub devname: String,
    pub host: CpalHost,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

#[derive(Clone, Debug)]
pub struct CpalCaptureDevice {
    pub devname: String,
    pub host: CpalHost,
    pub samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

fn open_cpal_playback(
    host_cfg: CpalHost,
    devname: &str,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
) -> Res<(Device, StreamConfig, cpal::SampleFormat)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(all(
            feature = "jack-backend",
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd"
            )
        ))]
        CpalHost::Jack => HostId::Jack,
    };
    let host = cpal::host_from_id(host_id)?;
    let device = if devname == "default" {
        match host.default_output_device() {
            Some(dev) => dev,
            None => {
                let msg = "Could not get default playback device".to_string();
                return Err(ConfigError::new(&msg).into());
            }
        }
    } else {
        let mut devices = host.devices()?;
        match devices.find(|dev| match dev.name() {
            Ok(n) => n == devname,
            _ => false,
        }) {
            Some(dev) => dev,
            None => {
                let msg = format!("Could not find playback device '{devname}'");
                return Err(ConfigError::new(&msg).into());
            }
        }
    };
    let cpal_format = match sample_format {
        SampleFormat::S16LE => cpal::SampleFormat::I16,
        SampleFormat::FLOAT32LE => cpal::SampleFormat::F32,
        _ => panic!("Unsupported sample format"),
    };
    let stream_config = StreamConfig {
        channels: channels as ChannelCount,
        sample_rate: SampleRate(samplerate as u32),
        buffer_size: BufferSize::Default,
    };
    debug!("Opened CPAL playback device {}", devname);
    Ok((device, stream_config, cpal_format))
}

fn open_cpal_capture(
    host_cfg: CpalHost,
    devname: &str,
    samplerate: usize,
    channels: usize,
    sample_format: &SampleFormat,
) -> Res<(Device, StreamConfig, cpal::SampleFormat)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(all(
            feature = "jack-backend",
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd"
            )
        ))]
        CpalHost::Jack => HostId::Jack,
    };
    let host = cpal::host_from_id(host_id)?;
    let device = if devname == "default" {
        match host.default_input_device() {
            Some(dev) => dev,
            None => {
                let msg = "Could not get default capture device".to_string();
                return Err(ConfigError::new(&msg).into());
            }
        }
    } else {
        let mut devices = host.devices()?;
        match devices.find(|dev| match dev.name() {
            Ok(n) => n == devname,
            _ => false,
        }) {
            Some(dev) => dev,
            None => {
                let msg = format!("Could not find capture device '{devname}'");
                return Err(ConfigError::new(&msg).into());
            }
        }
    };

    let cpal_format = match sample_format {
        SampleFormat::S16LE => cpal::SampleFormat::I16,
        SampleFormat::FLOAT32LE => cpal::SampleFormat::F32,
        _ => panic!("Unsupported sample format"),
    };
    let stream_config = StreamConfig {
        channels: channels as ChannelCount,
        sample_rate: SampleRate(samplerate as u32),
        buffer_size: BufferSize::Default,
    };
    debug!("Opened CPAL capture device {}", devname);
    Ok((device, stream_config, cpal_format))
}

fn write_data_to_device<T>(output: &mut [T], queue: &mut VecDeque<T>)
where
    T: cpal::Sample,
{
    for sample in output.iter_mut() {
        *sample = queue.pop_front().unwrap();
    }
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for CpalPlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let host_cfg = self.host.clone();
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
        let chunksize_clone = chunksize;
        let channels_clone = channels;

        let bits_per_sample = self.sample_format.bits_per_sample() as i32;
        let sample_format = self.sample_format;
        let playback_status_clone = playback_status.clone();
        let handle = thread::Builder::new()
            .name("CpalPlayback".to_string())
            .spawn(move || {
                match open_cpal_playback(host_cfg, &devname, samplerate, channels, &sample_format) {
                    Ok((device, stream_config, _sample_format)) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = PrcFmt::coerce(2.0).powi(bits_per_sample - 1);

                        let (tx_dev, rx_dev) = mpsc::sync_channel(1);
                        let buffer_fill = Arc::new(AtomicUsize::new(0));
                        let buffer_fill_clone = buffer_fill.clone();
                        let mut buffer_avg = countertimer::Averager::new();
                        let mut timer = countertimer::Stopwatch::new();
                        let mut chunk_stats = ChunkStats{rms: vec![0.0; channels], peak: vec![0.0; channels]};

                        let stream = match sample_format {
                            SampleFormat::S16LE => {
                                trace!("Build i16 output stream");
                                let mut clipped = 0;
                                let mut running = true;
                                let mut sample_queue: VecDeque<i16> =
                                    VecDeque::with_capacity(4 * chunksize_clone * channels_clone);
                                let stream = device.build_output_stream(
                                    &stream_config,
                                    move |buffer: &mut [i16], _: &cpal::OutputCallbackInfo| {
                                        #[cfg(feature = "debug")]
                                        trace!("Playback device requests {} samples", buffer.len());
                                        while sample_queue.len() < buffer.len() {
                                            //trace!("Convert chunk to device format");
                                            match rx_dev.recv_timeout(time::Duration::from_micros(
                                                (1000000 * buffer.len()
                                                    / (samplerate * channels_clone))
                                                    as u64,
                                            )) {
                                                Ok(chunk) => {
                                                    if !running {
                                                        running = true;
                                                        info!("Restarting playback after buffer underrun");
                                                    }
                                                    clipped = chunk_to_queue_int(
                                                        &chunk,
                                                        &mut sample_queue,
                                                        scalefactor,
                                                    )
                                                }
                                                Err(_) => {
                                                    if running {
                                                        running = false;
                                                        warn!("Playback interrupted, no data available");
                                                    }
                                                    // Something went wrong, lets just clear the buffer
                                                    for sample in buffer.iter_mut() {
                                                        *sample = 0;
                                                    }
                                                    return;
                                                }
                                            }
                                        }
                                        write_data_to_device(buffer, &mut sample_queue);
                                        buffer_fill_clone
                                            .store(sample_queue.len(), Ordering::Relaxed);
                                        if clipped > 0 {
                                            playback_status_clone
                                                .write()
                                                .clipped_samples += clipped;
                                        }
                                    },
                                    move |err| error!("an error occurred on stream: {}", err),
                                );
                                trace!("i16 output stream ready");
                                stream
                            }
                            SampleFormat::FLOAT32LE => {
                                trace!("Build f32 output stream");
                                let mut clipped = 0;
                                let mut running = true;
                                let mut sample_queue: VecDeque<f32> =
                                    VecDeque::with_capacity(4 * chunksize_clone * channels_clone);
                                let stream = device.build_output_stream(
                                    &stream_config,
                                    move |buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
                                        #[cfg(feature = "debug")]
                                        trace!("Playback device requests {} samples", buffer.len());
                                        while sample_queue.len() < buffer.len() {
                                            //trace!("Convert chunk to device format");
                                            match rx_dev.recv_timeout(time::Duration::from_micros(
                                                (1000000 * buffer.len()
                                                    / (samplerate * channels_clone))
                                                    as u64,
                                            )) {
                                                Ok(chunk) => {
                                                    if !running {
                                                        running = true;
                                                        info!("Restarting playback after buffer underrun");
                                                    }
                                                    clipped = chunk_to_queue_float(
                                                        &chunk,
                                                        &mut sample_queue,
                                                    )
                                                }
                                                Err(_) => {
                                                    if running {
                                                        running = false;
                                                        warn!("Playback interrupted, no data available");
                                                    }
                                                    // Something went wrong, lets just clear the buffer
                                                    for sample in buffer.iter_mut() {
                                                        *sample = 0.0;
                                                    }
                                                    return;
                                                }
                                            }
                                        }
                                        write_data_to_device(buffer, &mut sample_queue);
                                        buffer_fill_clone
                                            .store(sample_queue.len(), Ordering::Relaxed);
                                        if clipped > 0 {
                                            playback_status_clone
                                                .write()
                                                .clipped_samples += clipped;
                                        }
                                    },
                                    move |err| error!("an error occurred on stream: {}", err),
                                );
                                trace!("f32 output stream ready");
                                stream
                            }
                            _ => panic!("Unsupported sample format!"),
                        };
                        if let Err(err) = &stream {
                            status_channel
                                .send(StatusMessage::PlaybackError(err.to_string()))
                                .unwrap();
                        }
                        barrier.wait();
                        if let Ok(strm) = &stream {
                            match strm.play() {
                                Ok(_) => debug!("Starting playback loop"),
                                Err(err) => status_channel
                                    .send(StatusMessage::PlaybackError(err.to_string()))
                                    .unwrap(),
                            }
                        }
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    chunk.update_stats(&mut chunk_stats);
                                    {
                                        let mut playback_status = playback_status.write();
                                        playback_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                                        playback_status.signal_peak.add_record(chunk_stats.peak_linear());
                                    }
                                    buffer_avg.add_value(
                                        (buffer_fill.load(Ordering::Relaxed) / channels_clone)
                                            as f64,
                                    );
                                    if adjust
                                        && timer.larger_than_millis((1000.0 * adjust_period) as u64)
                                    {
                                        if let Some(av_delay) = buffer_avg.average() {
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
                                                .unwrap();
                                            playback_status.write().buffer_level =
                                                av_delay as usize;
                                        }
                                    }

                                    tx_dev.send(chunk).unwrap();
                                }
                                Ok(AudioMessage::Pause) => {
                                    trace!("Pause message received");
                                }
                                Ok(AudioMessage::EndOfStream) => {
                                    status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                    break;
                                }
                                Err(err) => {
                                    error!("Message channel error: {}", err);
                                    status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                    break;
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let send_result = status_channel.send(StatusMessage::PlaybackError(err.to_string()));
                        if send_result.is_err() {
                            error!("Playback error: {}", err);
                        }
                        barrier.wait();
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

fn nbr_capture_samples(
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    capture_samples: usize,
    channels: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        #[cfg(feature = "debug")]
        trace!(
            "Resampler needs {} frames, will read {} samples",
            resampl.input_frames_next(),
            resampl.input_frames_next() * channels,
        );
        resampl.input_frames_next() * channels
    } else {
        capture_samples
    }
}

fn write_data_from_device<T>(data: &[T], queue: &mut VecDeque<T>)
where
    T: cpal::Sample,
{
    //trace!("Write data to device");
    for sample in data.iter() {
        queue.push_back(*sample);
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for CpalCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let host_cfg = self.host.clone();
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample() as i32;
        let sample_format = self.sample_format;
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = self.rate_measure_interval;
        let handle = thread::Builder::new()
            .name("CpalCapture".to_string())
            .spawn(move || {
                let mut resampler = new_resampler(
                        &resampler_config,
                        channels,
                        samplerate,
                        capture_samplerate,
                        chunksize,
                    );
                match open_cpal_capture(host_cfg, &devname, capture_samplerate, channels, &sample_format) {
                    Ok((device, stream_config, _sample_format)) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = PrcFmt::coerce(2.0).powi(bits_per_sample - 1);
                        let (tx_dev_i, rx_dev_i) = mpsc::sync_channel(1);
                        let (tx_dev_f, rx_dev_f) = mpsc::sync_channel(1);
                        let stream = match sample_format {
                            SampleFormat::S16LE => {
                                trace!("Build i16 input stream");
                                let stream = device.build_input_stream(
                                    &stream_config,
                                    move |buffer: &[i16], _: &cpal::InputCallbackInfo| {
                                        #[cfg(feature = "debug")]
                                        trace!("Capture device provides {} samples", buffer.len());
                                        let buffer_copy = Vec::from(buffer);
                                        tx_dev_i.send(buffer_copy).unwrap();
                                    },
                                    move |err| error!("an error occurred on stream: {}", err)
                                );
                                trace!("i16 input stream ready");
                                stream
                            },
                            SampleFormat::FLOAT32LE => {
                                trace!("Build f32 input stream");
                                let stream = device.build_input_stream(
                                    &stream_config,
                                    move |buffer: &[f32], _: &cpal::InputCallbackInfo| {
                                        #[cfg(feature = "debug")]
                                        trace!("Capture device provides {} samples", buffer.len());
                                        let buffer_copy = Vec::from(buffer);
                                        tx_dev_f.send(buffer_copy).unwrap();
                                    },
                                    move |err| error!("an error occurred on stream: {}", err)
                                );
                                trace!("f32 input stream ready");
                                stream
                            },
                            _ => panic!("Unsupported sample format!"),
                        };
                        if let Err(err) = &stream {
                            status_channel
                                .send(StatusMessage::CaptureError(err.to_string()))
                                .unwrap();
                        }
                        barrier.wait();
                        if let Ok(strm) = &stream {
                            match strm.play() {
                                Ok(_) => debug!("Starting capture loop"),
                                Err(err) => status_channel
                                    .send(StatusMessage::CaptureError(err.to_string()))
                                    .unwrap(),
                            }
                        }
                        let chunksize_samples = channels * chunksize;
                        let mut capture_samples = chunksize_samples;
                        let mut sample_queue_i: VecDeque<i16> = VecDeque::with_capacity(2*chunksize*channels);
                        let mut sample_queue_f: VecDeque<f32> = VecDeque::with_capacity(2*chunksize*channels);
                        let mut averager = countertimer::TimeAverage::new();
                        let mut watcher_averager = countertimer::TimeAverage::new();
                        let mut valuewatcher =
                            countertimer::ValueWatcher::new(capture_samplerate as f32, RATE_CHANGE_THRESHOLD_VALUE, RATE_CHANGE_THRESHOLD_COUNT);
                        let rate_measure_interval_ms = (1000.0 * rate_measure_interval) as u64;
                        let mut value_range = 0.0;
                        let mut chunk_stats = ChunkStats{rms: vec![0.0; channels], peak: vec![0.0; channels]};
                        let mut rate_adjust = 0.0;
                        let mut silence_counter = countertimer::SilenceCounter::new(silence_threshold, silence_timeout, capture_samplerate, chunksize);
                        let mut state = ProcessingState::Running;
                        let mut channel_mask = vec![true; channels];
                        loop {
                            match command_channel.try_recv() {
                                Ok(CommandMessage::Exit) => {
                                    debug!("Exit message received, sending EndOfStream");
                                    let msg = AudioMessage::EndOfStream;
                                    channel.send(msg).unwrap();
                                    status_channel.send(StatusMessage::CaptureDone).unwrap();
                                    break;
                                }
                                Ok(CommandMessage::SetSpeed { speed }) => {
                                    rate_adjust = speed;
                                    if let Some(resampl) = &mut resampler {
                                        debug!("Adjusting resampler rate to {}", speed);
                                        if async_src {
                                            if resampl.set_resample_ratio_relative(speed, true).is_err() {
                                                debug!("Failed to set resampling speed to {}", speed);
                                            }
                                        }
                                        else {
                                            warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                                        }
                                    }
                                }
                                Err(mpsc::TryRecvError::Empty) => {}
                                Err(mpsc::TryRecvError::Disconnected) => {
                                    error!("Command channel was closed");
                                    break;
                                }
                            };
                            capture_samples = nbr_capture_samples(
                                &resampler,
                                capture_samples,
                                channels,
                            );

                            let mut chunk = match sample_format {
                                SampleFormat::S16LE => {
                                    while sample_queue_i.len() < capture_samples {
                                        //trace!("Read message to fill capture buffer");
                                        match rx_dev_i.recv() {
                                            Ok(buf) => {
                                                write_data_from_device(&buf, &mut sample_queue_i);
                                            }
                                            Err(err) => {
                                                status_channel
                                                    .send(StatusMessage::CaptureError(err.to_string()))
                                                    .unwrap();
                                            }
                                        }
                                    }
                                    queue_to_chunk_int(
                                        &mut sample_queue_i,
                                        capture_samples/channels,
                                        channels,
                                        scalefactor,
                                    )
                                },
                                SampleFormat::FLOAT32LE => {
                                    while sample_queue_f.len() < capture_samples {
                                        //trace!("Read message to fill capture buffer");
                                        match rx_dev_f.recv() {
                                            Ok(buf) => {
                                                write_data_from_device(&buf, &mut sample_queue_f);
                                            }
                                            Err(err) => {
                                                status_channel
                                                    .send(StatusMessage::CaptureError(err.to_string()))
                                                    .unwrap();
                                            }
                                        }
                                    }
                                    queue_to_chunk_float(
                                        &mut sample_queue_f,
                                        capture_samples/channels,
                                        channels,
                                    )
                                },
                                _ => panic!("Unsupported sample format"),
                            };
                            averager.add_value(capture_samples);
                            {
                                let capture_status = capture_status.upgradable_read();
                                if averager.larger_than_millis(capture_status.update_interval as u64)
                                {
                                    let samples_per_sec = averager.average();
                                    averager.restart();
                                    let measured_rate_f = samples_per_sec / channels as f64;
                                    trace!(
                                        "Measured sample rate is {:.1} Hz",
                                        measured_rate_f
                                    );
                                    let mut capture_status = RwLockUpgradableReadGuard::upgrade(capture_status); // to write lock
                                    capture_status.measured_samplerate = measured_rate_f as usize;
                                    capture_status.signal_range = value_range as f32;
                                    capture_status.rate_adjust = rate_adjust as f32;
                                    capture_status.state = state;
                                }
                            }
                            watcher_averager.add_value(capture_samples);
                            if watcher_averager.larger_than_millis(rate_measure_interval_ms) {
                                let samples_per_sec = watcher_averager.average();
                                watcher_averager.restart();
                                let measured_rate_f = samples_per_sec / channels as f64;
                                let changed = valuewatcher.check_value(measured_rate_f as f32);
                                if changed {
                                    warn!(
                                        "sample rate change detected, last rate was {} Hz",
                                        measured_rate_f
                                    );
                                    if stop_on_rate_change {
                                        let msg = AudioMessage::EndOfStream;
                                        channel.send(msg).unwrap_or(());
                                        status_channel
                                            .send(StatusMessage::CaptureFormatChange(measured_rate_f as usize))
                                            .unwrap_or(());
                                        break;
                                    }
                                }
                                trace!("Measured sample rate is {:.1} Hz", measured_rate_f);
                            }
                            chunk.update_stats(&mut chunk_stats);
                            //trace!("Capture rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
                            {
                                let mut capture_status = capture_status.write();
                                capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                                capture_status.signal_peak.add_record(chunk_stats.peak_linear());
                            }
                            value_range = chunk.maxval - chunk.minval;
                            state = silence_counter.update(value_range);
                            if state == ProcessingState::Running {
                                if let Some(resampl) = &mut resampler {
                                    chunk.update_channel_mask(&mut channel_mask);
                                    let new_waves = resampl.process(&chunk.waveforms, Some(&channel_mask)).unwrap();
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
                        capture_status.write().state = ProcessingState::Inactive;
                    }
                    Err(err) => {
                        let send_result = status_channel
                            .send(StatusMessage::CaptureError(err.to_string()));
                        if send_result.is_err() {
                            error!("Capture error: {}", err);
                        }
                        barrier.wait();
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
