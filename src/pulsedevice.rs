use psimple::Simple;
use pulse;
use pulse::sample;
use pulse::stream::Direction;

use crate::audiodevice::*;
use crate::config;
use crate::config::SampleFormat;
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rubato::VecResampler;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

#[derive(Debug)]
pub struct PulseError {
    desc: String,
}

impl std::fmt::Display for PulseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl std::error::Error for PulseError {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl PulseError {
    pub fn new(pa_error: &pulse::error::PAErr) -> Self {
        let msg = if let Some(desc) = pa_error.to_string() {
            desc
        } else {
            "Unknown error".to_string()
        };
        let desc = format!("PulseAudio error: {}, code: {}", msg, pa_error.0);
        PulseError { desc }
    }
}

pub struct PulsePlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
}

pub struct PulseCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub capture_samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
}

/// Open a PulseAudio device
fn open_pulse(
    devname: String,
    samplerate: u32,
    channels: u8,
    sample_format: &SampleFormat,
    capture: bool,
) -> Res<Simple> {
    // Open the device
    let dir = if capture {
        Direction::Record
    } else {
        Direction::Playback
    };

    let pulse_format = match sample_format {
        SampleFormat::S16LE => sample::Format::S16le,
        SampleFormat::S24LE => sample::Format::S24_32le,
        SampleFormat::S24LE3 => sample::Format::S24le,
        SampleFormat::S32LE => sample::Format::S32le,
        SampleFormat::FLOAT32LE => sample::Format::F32le,
        _ => panic!("invalid format"),
    };

    let bytes_per_sample = sample_format.bytes_per_sample();

    let spec = sample::Spec {
        format: pulse_format,
        channels,
        rate: samplerate,
    };
    //assert!(spec.is_valid());
    let attr = pulse::def::BufferAttr {
        maxlength: std::u32::MAX,
        tlength: std::u32::MAX,
        prebuf: bytes_per_sample as u32,
        minreq: std::u32::MAX,
        fragsize: bytes_per_sample as u32,
    };

    let pulsedev_res = Simple::new(
        None,           // Use the default server
        "CamillaDSP",   // Our application’s name
        dir,            // We want a playback stream
        Some(&devname), // Use the default device
        "ToDSP",        // Description of our stream
        &spec,          // Our sample format
        None,           // Use default channel map
        Some(&attr),    // Use default buffering attributes
    );
    match pulsedev_res {
        Err(err) => Err(PulseError::new(&err).into()),
        Ok(pulsedev) => Ok(pulsedev),
    }
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for PulsePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format;
        let handle = thread::Builder::new()
            .name("PulsePlayback".to_string())
            .spawn(move || {
                match open_pulse(
                    devname,
                    samplerate as u32,
                    channels as u8,
                    &sample_format,
                    false,
                ) {
                    Ok(pulsedevice) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let mut conversion_result;
                        let mut chunk_stats = ChunkStats {
                            rms: vec![0.0; channels],
                            peak: vec![0.0; channels],
                        };
                        let bytes_per_frame = channels * store_bytes_per_sample;
                        barrier.wait();
                        let mut last_instant = Instant::now();
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes_per_sample];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    conversion_result = chunk_to_buffer_rawbytes(
                                        &chunk,
                                        &mut buffer,
                                        &sample_format,
                                    );
                                    sleep_until_next(
                                        &last_instant,
                                        bytes_per_frame,
                                        samplerate,
                                        buffer.len(),
                                    );
                                    let write_res = pulsedevice.write(&buffer);
                                    last_instant = Instant::now();
                                    match write_res {
                                        Ok(_) => {}
                                        Err(err) => {
                                            status_channel
                                                .send(StatusMessage::PlaybackError(
                                                    err.to_string().unwrap_or(
                                                        "Unknown playback error".to_string(),
                                                    ),
                                                ))
                                                .unwrap();
                                        }
                                    };
                                    chunk.update_stats(&mut chunk_stats);
                                    {
                                        let mut playback_status = playback_status.write();
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
                                    //trace!(
                                    //    "Playback signal RMS: {:?}, peak: {:?}",
                                    //    chunk_stats.rms_db(),
                                    //    chunk_stats.peak_db()
                                    //);
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
                        let send_result =
                            status_channel.send(StatusMessage::PlaybackError(err.to_string()));
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

fn nbr_capture_bytes(
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    capture_bytes: usize,
    channels: usize,
    store_bytes_per_sample: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        //let new_capture_bytes = resampl.input_frames_next() * channels * store_bytes_per_sample;
        //trace!(
        //    "Resampler needs {} frames, will read {} bytes",
        //    resampl.input_frames_next(),
        //    new_capture_bytes
        //);
        //new_capture_bytes
        resampl.input_frames_next() * channels * store_bytes_per_sample
    } else {
        capture_bytes
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for PulseCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let buffer_bytes = 2.0f32.powf(
            (capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
                .log2()
                .ceil(),
        ) as usize
            * 2
            * channels
            * store_bytes_per_sample;
        let sample_format = self.sample_format;
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let handle = thread::Builder::new()
            .name("PulseCapture".to_string())
            .spawn(move || {
                let mut resampler = new_resampler(
                        &resampler_config,
                        channels,
                        samplerate,
                        capture_samplerate,
                        chunksize,
                    );
                match open_pulse(
                    devname,
                    capture_samplerate as u32,
                    channels as u8,
                    &sample_format,
                    true,
                ) {
                    Ok(pulsedevice) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        barrier.wait();
                        debug!("starting captureloop");
                        let mut buf = vec![0u8; buffer_bytes];
                        let chunksize_bytes = channels * chunksize * store_bytes_per_sample;
                        let mut capture_bytes = chunksize_bytes;
                        let mut averager = countertimer::TimeAverage::new();
                        let mut silence_counter = countertimer::SilenceCounter::new(silence_threshold, silence_timeout, capture_samplerate, chunksize);
                        let mut value_range = 0.0;
                        let mut rate_adjust = 0.0;
                        let mut state = ProcessingState::Running;
                        let mut chunk_stats = ChunkStats{rms: vec![0.0; channels], peak: vec![0.0; channels]};
                        let bytes_per_frame = channels * store_bytes_per_sample;
                        let mut channel_mask = vec![true; channels];
                        let mut last_instant = Instant::now();
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
                            capture_bytes = nbr_capture_bytes(
                                &resampler,
                                capture_bytes,
                                channels,
                                store_bytes_per_sample,
                            );
                            if capture_bytes > buf.len() {
                                debug!("Capture buffer too small, extending");
                                buf.append(&mut vec![0u8; capture_bytes - buf.len()]);
                            }
                            sleep_until_next(&last_instant, bytes_per_frame, samplerate, capture_bytes);
                            let read_res = pulsedevice.read(&mut buf[0..capture_bytes]);
                            last_instant = Instant::now();
                            match read_res {
                                Ok(()) => {
                                    averager.add_value(capture_bytes);
                                    let capture_status = capture_status.upgradable_read();
                                    if averager.larger_than_millis(capture_status.update_interval as u64) {
                                        let bytes_per_sec = averager.average();
                                        averager.restart();
                                        let measured_rate_f = bytes_per_sec / (channels * store_bytes_per_sample) as f64;
                                        trace!(
                                            "Measured sample rate is {:.1} Hz, signal RMS is {:?}",
                                            measured_rate_f,
                                            capture_status.signal_rms.last(),
                                        );
                                        let mut capture_status = RwLockUpgradableReadGuard::upgrade(capture_status); // to write lock
                                        capture_status.measured_samplerate = measured_rate_f as usize;
                                        capture_status.signal_range = value_range as f32;
                                        capture_status.rate_adjust = rate_adjust as f32;
                                        capture_status.state = state;
                                    }
                                }
                                Err(err) => {
                                    status_channel
                                        .send(StatusMessage::CaptureError(err.to_string().unwrap_or("Unknown capture error".to_string())))
                                        .unwrap();
                                }
                            };
                            let mut chunk = buffer_to_chunk_rawbytes(&buf[0..capture_bytes],channels, &sample_format, capture_bytes, &capture_status.read().used_channels);
                            chunk.update_stats(&mut chunk_stats);
                            {
                                let mut capture_status = capture_status.write();
                                capture_status.signal_rms.add_record_squared(chunk_stats.rms_linear());
                                capture_status.signal_peak.add_record(chunk_stats.peak_linear());
                            }
                            //trace!("Capture signal rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
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

fn sleep_until_next(
    last_instant: &Instant,
    bytes_per_frame: usize,
    samplerate: usize,
    nbr_bytes: usize,
) {
    let io_duration =
        Duration::from_millis((1000 * nbr_bytes) as u64 / (bytes_per_frame * samplerate) as u64);
    let time_spent = Instant::now().duration_since(*last_instant);
    if (time_spent + Duration::from_millis(5)) < io_duration {
        thread::sleep(io_duration - time_spent - Duration::from_millis(5));
    }
}
