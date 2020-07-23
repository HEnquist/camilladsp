use audiodevice::*;
use config;
use config::{ConfigError, SampleFormat};
use conversions::{
    chunk_to_queue_float, chunk_to_queue_int, queue_to_chunk_float, queue_to_chunk_int,
};
use cpal;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Device;
use cpal::{ChannelCount, HostId, SampleRate, StreamConfig, BufferSize};
use rubato::Resampler;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::SystemTime;

use crate::CaptureStatus;
use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

#[derive(Clone, Debug)]
pub enum CpalHost {
    #[cfg(target_os = "macos")]
    CoreAudio,
    #[cfg(target_os = "windows")]
    Wasapi,
    #[cfg(all(feature = "cpal-asio", target_os = "windows"))]
    Asio,
}

#[derive(Clone, Debug)]
pub struct CpalPlaybackDevice {
    pub devname: String,
    pub host: CpalHost,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub format: SampleFormat,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

#[derive(Clone, Debug)]
pub struct CpalCaptureDevice {
    pub devname: String,
    pub host: CpalHost,
    pub samplerate: usize,
    pub resampler_conf: config::Resampler,
    pub enable_resampling: bool,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
}

fn open_cpal_playback(
    host_cfg: CpalHost,
    devname: &str,
    samplerate: usize,
    channels: usize,
    format: &SampleFormat,
) -> Res<(Device, StreamConfig, cpal::SampleFormat)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(target_os = "windows")]
        CpalHost::Wasapi => HostId::Wasapi,
        #[cfg(all(feature = "cpal-asio", target_os = "windows"))]
        CpalHost::Asio => HostId::Asio,
    };
    let host = cpal::host_from_id(host_id)?;
    let mut devices = host.devices()?;
    let device = match devices.find(|dev| match dev.name() {
        Ok(n) => n == devname,
        _ => false,
    }) {
        Some(dev) => dev,
        None => {
            return Err(Box::new(ConfigError::new(&format!(
                "Could not find device: {}",
                devname
            ))))
        }
    };
    let sample_format = match format {
        SampleFormat::S16LE => cpal::SampleFormat::I16,
        SampleFormat::FLOAT32LE => cpal::SampleFormat::F32,
        _ => panic!("Unsupported sample format"),
    };
    let stream_config = StreamConfig {
        channels: channels as ChannelCount,
        sample_rate: SampleRate(samplerate as u32),
        buffer_size: BufferSize::Default,
    };
    //let event_loop = host.event_loop();
    //let stream_id = event_loop.build_output_stream(&device, &format)?;
    //event_loop.play_stream(stream_id.clone())?;
    debug!("Opened CPAL playback device {}", devname);
    Ok((device, stream_config, sample_format))
}

fn open_cpal_capture(
    host_cfg: CpalHost,
    devname: &str,
    samplerate: usize,
    channels: usize,
    format: &SampleFormat,
) -> Res<(Device, StreamConfig, cpal::SampleFormat)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(target_os = "windows")]
        CpalHost::Wasapi => HostId::Wasapi,
        #[cfg(all(feature = "cpal-asio", target_os = "windows"))]
        CpalHost::Asio => HostId::Asio,
    };
    let host = cpal::host_from_id(host_id)?;
    let mut devices = host.devices()?;
    let device = match devices.find(|dev| match dev.name() {
        Ok(n) => n == devname,
        _ => false,
    }) {
        Some(dev) => dev,
        None => {
            return Err(Box::new(ConfigError::new(&format!(
                "Could not find device: {}",
                devname
            ))))
        }
    };
    let sample_format = match format {
        SampleFormat::S16LE => cpal::SampleFormat::I16,
        SampleFormat::FLOAT32LE => cpal::SampleFormat::F32,
        _ => panic!("Unsupported sample format"),
    };
    let stream_config = StreamConfig {
        channels: channels as ChannelCount,
        sample_rate: SampleRate(samplerate as u32),
        buffer_size: BufferSize::Default,
    };
    //let stream_id = event_loop.build_input_stream(&device, &format)?;
    //event_loop.play_stream(stream_id.clone())?;
    debug!("Opened CPAL capture device {}", devname);
    Ok((device, stream_config, sample_format))
}

fn write_data_to_device<T>(output: &mut [T], queue: &mut VecDeque<T>)
where
    T: cpal::Sample,
{
    trace!("Write data to device");
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
        status_channel: mpsc::Sender<StatusMessage>,
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

        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S24LE3 => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let format = self.format.clone();
        let handle = thread::Builder::new()
            .name("CpalPlayback".to_string())
            .spawn(move || {
                match open_cpal_playback(host_cfg, &devname, samplerate, channels, &format) {
                    Ok((device, stream_config, _sample_format)) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);

                        let (tx_dev, rx_dev) = mpsc::sync_channel(1);
                        let buffer_fill = Arc::new(AtomicUsize::new(0));
                        let buffer_fill_clone = buffer_fill.clone();
                        let mut start = SystemTime::now();
                        let mut now;
                        let mut delay = 0;
                        let mut ndelays = 0;
                        let mut speed;
                        let mut diff: isize;

                        let stream = match format {
                            SampleFormat::S16LE => {
                                trace!("Build i16 output stream");
                                let mut sample_queue: VecDeque<i16> =
                                    VecDeque::with_capacity(4 * chunksize_clone * channels_clone);
                                let stream = device.build_output_stream(
                                    &stream_config,
                                    move |mut buffer: &mut [i16], _: &cpal::OutputCallbackInfo| {
                                        trace!("Playback device requests {} samples", buffer.len());
                                        while sample_queue.len() < buffer.len() {
                                            trace!("Convert chunk to device format");
                                            let chunk = rx_dev.recv().unwrap();
                                            chunk_to_queue_int(
                                                chunk,
                                                &mut sample_queue,
                                                scalefactor,
                                            );
                                        }
                                        write_data_to_device(&mut buffer, &mut sample_queue);
                                        buffer_fill_clone
                                            .store(sample_queue.len(), Ordering::Relaxed);
                                    },
                                    move |err| error!("an error occurred on stream: {}", err),
                                );
                                //stream.play().unwrap();
                                trace!("i16 output stream ready");
                                stream
                            }
                            SampleFormat::FLOAT32LE => {
                                trace!("Build f32 output stream");
                                let mut sample_queue: VecDeque<f32> =
                                    VecDeque::with_capacity(4 * chunksize_clone * channels_clone);
                                let stream = device.build_output_stream(
                                    &stream_config,
                                    move |mut buffer: &mut [f32], _: &cpal::OutputCallbackInfo| {
                                        trace!("Playback device requests {} samples", buffer.len());
                                        while sample_queue.len() < buffer.len() {
                                            trace!("Convert chunk to device format");
                                            let chunk = rx_dev.recv().unwrap();
                                            chunk_to_queue_float(chunk, &mut sample_queue);
                                        }
                                        write_data_to_device(&mut buffer, &mut sample_queue);
                                        buffer_fill_clone
                                            .store(sample_queue.len(), Ordering::Relaxed);
                                    },
                                    move |err| error!("an error occurred on stream: {}", err),
                                );
                                //stream.play().unwrap();
                                trace!("f32 output stream ready");
                                stream
                            }
                            _ => panic!("Unsupported sample format!"),
                        };
                        if let Err(err) = &stream {
                            status_channel
                                .send(StatusMessage::PlaybackError {
                                    message: format!("{}", err),
                                })
                                .unwrap();
                        }
                        barrier.wait();
                        if let Ok(strm) = &stream {
                            match strm.play() {
                                Ok(_) => debug!("Starting playback loop"),
                                Err(err) => status_channel
                                    .send(StatusMessage::PlaybackError {
                                        message: format!("{}", err),
                                    })
                                    .unwrap(),
                            }
                        }
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    now = SystemTime::now();
                                    delay += (buffer_fill.load(Ordering::Relaxed) / channels_clone)
                                        as isize;
                                    ndelays += 1;
                                    if adjust
                                        && (now.duration_since(start).unwrap().as_millis()
                                            > ((1000.0 * adjust_period) as u128))
                                    {
                                        let av_delay = delay / ndelays;
                                        diff = av_delay - target_level as isize;
                                        let rel_diff = (diff as f64) / (samplerate as f64);
                                        speed = 1.0 - 0.5 * rel_diff / adjust_period as f64;
                                        debug!(
                                            "Current buffer level {}, set capture rate to {}%",
                                            av_delay,
                                            100.0 * speed
                                        );
                                        start = now;
                                        delay = 0;
                                        ndelays = 0;
                                        status_channel
                                            .send(StatusMessage::SetSpeed { speed })
                                            .unwrap();
                                    }
                                    tx_dev.send(chunk).unwrap();
                                }
                                Ok(AudioMessage::EndOfStream) => {
                                    status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                    break;
                                }
                                Err(_) => {}
                            }
                        }
                    }
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError {
                                message: format!("{}", err),
                            })
                            .unwrap();
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

fn get_nbr_capture_samples(
    resampler: &Option<Box<dyn Resampler<PrcFmt>>>,
    capture_samples: usize,
    channels: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        let new_capture_samples = resampl.nbr_frames_needed() * channels;
        trace!(
            "Resampler needs {} frames, will read {} samples",
            resampl.nbr_frames_needed(),
            new_capture_samples
        );
        new_capture_samples
    } else {
        capture_samples
    }
}

fn write_data_from_device<T>(data: &[T], queue: &mut VecDeque<T>)
where
    T: cpal::Sample,
{
    trace!("Write data to device");
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
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let host_cfg = self.host.clone();
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S24LE3 => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let format = self.format.clone();
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
        let async_src = resampler_is_async(&resampler_conf);
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
        let handle = thread::Builder::new()
            .name("CpalCapture".to_string())
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
                match open_cpal_capture(host_cfg, &devname, capture_samplerate, channels, &format) {
                    Ok((device, stream_config, _sample_format)) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                        let mut silent_nbr: usize = 0;
                        let (tx_dev_i, rx_dev_i) = mpsc::sync_channel(1);
                        let (tx_dev_f, rx_dev_f) = mpsc::sync_channel(1);
                        let stream = match format {
                            SampleFormat::S16LE => {
                                trace!("Build i16 input stream");
                                let stream = device.build_input_stream(
                                    &stream_config,
                                    move |buffer: &[i16], _: &cpal::InputCallbackInfo| {
                                        trace!(
                                            "Playback device requests {} samples",
                                            buffer.len()
                                        );
                                        trace!("Capture device provides {} samples", buffer.len());
                                        let mut buffer_copy = Vec::new();
                                        buffer_copy.extend_from_slice(&buffer);
                                        tx_dev_i.send(buffer_copy).unwrap();
                                    },
                                    move |err| error!("an error occurred on stream: {}", err)
                                );
                                //stream.play().unwrap();
                                trace!("i16 input stream ready");
                                stream
                            },
                            SampleFormat::FLOAT32LE => {
                                trace!("Build f32 input stream");
                                let stream = device.build_input_stream(
                                    &stream_config,
                                    move |buffer: &[f32], _: &cpal::InputCallbackInfo| {
                                        trace!(
                                            "Playback device requests {} samples",
                                            buffer.len()
                                        );
                                        trace!("Capture device provides {} samples", buffer.len());
                                        let mut buffer_copy = Vec::new();
                                        buffer_copy.extend_from_slice(&buffer);
                                        tx_dev_f.send(buffer_copy).unwrap();
                                    },
                                    move |err| error!("an error occurred on stream: {}", err)
                                );
                                //stream.play().unwrap();
                                trace!("f32 input stream ready");
                                stream
                            },
                            _ => panic!("Unsupported sample format!"),
                        };
                        if let Err(err) = &stream {
                            status_channel
                                .send(StatusMessage::CaptureError {
                                    message: format!("{}", err),
                                })
                                .unwrap();
                        }
                        barrier.wait();
                        if let Ok(strm) = &stream {
                            match strm.play() {
                                Ok(_) => debug!("Starting capture loop"),
                                Err(err) => status_channel
                                    .send(StatusMessage::CaptureError {
                                        message: format!("{}", err),
                                    })
                                    .unwrap(),
                            }
                        }
                        let chunksize_samples = channels * chunksize;
                        let mut capture_samples = chunksize_samples;
                        let mut sample_queue_i: VecDeque<i16> = VecDeque::with_capacity(2*chunksize*channels);
                        let mut sample_queue_f: VecDeque<f32> = VecDeque::with_capacity(2*chunksize*channels);
                        let mut start = SystemTime::now();
                        let mut now;
                        let mut sample_counter = 0;
                        let mut value_range = 0.0;
                        loop {
                            match command_channel.try_recv() {
                                Ok(CommandMessage::Exit) => {
                                    let msg = AudioMessage::EndOfStream;
                                    channel.send(msg).unwrap();
                                    status_channel.send(StatusMessage::CaptureDone).unwrap();
                                    break;
                                }
                                Ok(CommandMessage::SetSpeed { speed }) => {
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
                                }
                                Err(_) => {}
                            };
                            capture_samples = get_nbr_capture_samples(
                                &resampler,
                                capture_samples,
                                channels,
                            );

                            let mut chunk = match format {
                                SampleFormat::S16LE => {
                                    while sample_queue_i.len() < capture_samples {
                                        trace!("Read message to fill capture buffer");
                                        match rx_dev_i.recv() {
                                            Ok(buf) => {
                                                write_data_from_device(&buf, &mut sample_queue_i);
                                            }
                                            Err(msg) => {
                                                status_channel
                                                    .send(StatusMessage::CaptureError {
                                                        message: format!("{}", msg),
                                                    })
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
                                        trace!("Read message to fill capture buffer");
                                        match rx_dev_f.recv() {
                                            Ok(buf) => {
                                                write_data_from_device(&buf, &mut sample_queue_f);
                                            }
                                            Err(msg) => {
                                                status_channel
                                                    .send(StatusMessage::CaptureError {
                                                        message: format!("{}", msg),
                                                    })
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
                            now = SystemTime::now();
                            sample_counter += capture_samples;
                            if now.duration_since(start).unwrap().as_millis() as usize > capture_status.read().unwrap().update_interval
                            {
                                let meas_time = now.duration_since(start).unwrap().as_secs_f32();
                                let samples_per_sec = sample_counter as f32 / meas_time;
                                let measured_rate_f = samples_per_sec / channels as f32;
                                trace!(
                                    "Measured sample rate is {} Hz",
                                    measured_rate_f
                                );
                                let mut capt_stat = capture_status.write().unwrap();
                                capt_stat.measured_samplerate = measured_rate_f as usize;
                                capt_stat.signal_range = value_range;
                                start = now;
                                sample_counter = 0;
                            }
                            value_range = chunk.maxval - chunk.minval;
                            if (value_range) > silence {
                                if silent_nbr > silent_limit {
                                    debug!("Resuming processing");
                                }
                                silent_nbr = 0;
                            } else if silent_limit > 0 {
                                if silent_nbr == silent_limit {
                                    debug!("Pausing processing");
                                }
                                silent_nbr += 1;
                            }
                            if silent_nbr <= silent_limit {
                                if let Some(resampl) = &mut resampler {
                                    let new_waves = resampl.process(&chunk.waveforms).unwrap();
                                    chunk.frames = new_waves[0].len();
                                    chunk.valid_frames = new_waves[0].len();
                                    chunk.waveforms = new_waves;
                                }
                                let msg = AudioMessage::Audio(chunk);
                                channel.send(msg).unwrap();
                            }
                        }
                    }
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::CaptureError {
                                message: format!("{}", err),
                            })
                            .unwrap();
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
