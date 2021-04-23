use psimple::Simple;
use pulse;
use pulse::sample;
use pulse::stream::Direction;

use audiodevice::*;
use config;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};
use countertimer;
use rubato::Resampler;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::{CaptureStatus, PlaybackStatus};
use CommandMessage;
use NewValue;
use PrcFmt;
use ProcessingState;
use Res;
use StatusMessage;

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
    pub resampler_conf: config::Resampler,
    pub enable_resampling: bool,
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
        status_channel: mpsc::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample() as i32;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format.clone();
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
                        let scalefactor = PrcFmt::new(2.0).powi(bits_per_sample - 1);
                        let mut conversion_result;
                        let mut chunk_stats;
                        let bytes_per_frame = channels * store_bytes_per_sample;
                        barrier.wait();
                        let mut last_instant = Instant::now();
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes_per_sample];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    match sample_format {
                                        SampleFormat::S16LE
                                        | SampleFormat::S24LE
                                        | SampleFormat::S32LE => {
                                            conversion_result = chunk_to_buffer_bytes(
                                                &chunk,
                                                &mut buffer,
                                                scalefactor,
                                                bits_per_sample,
                                                store_bytes_per_sample,
                                            );
                                        }
                                        SampleFormat::FLOAT32LE => {
                                            conversion_result = chunk_to_buffer_float_bytes(
                                                &chunk,
                                                &mut buffer,
                                                bits_per_sample,
                                            );
                                        }
                                        _ => panic!("Unsupported sample format!"),
                                    };
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
                                        Err(msg) => {
                                            status_channel
                                                .send(StatusMessage::PlaybackError {
                                                    message: format!("{}", msg),
                                                })
                                                .unwrap();
                                        }
                                    };
                                    if conversion_result.1 > 0 {
                                        playback_status.write().unwrap().clipped_samples +=
                                            conversion_result.1;
                                    }
                                    chunk_stats = chunk.get_stats();
                                    playback_status.write().unwrap().signal_rms =
                                        chunk_stats.rms_db();
                                    playback_status.write().unwrap().signal_peak =
                                        chunk_stats.peak_db();
                                    //trace!(
                                    //    "Playback signal RMS: {:?}, peak: {:?}",
                                    //    chunk_stats.rms_db(),
                                    //    chunk_stats.peak_db()
                                    //);
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
                        let send_result = status_channel.send(StatusMessage::PlaybackError {
                            message: format!("{}", err),
                        });
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

fn get_nbr_capture_bytes(
    resampler: &Option<Box<dyn Resampler<PrcFmt>>>,
    capture_bytes: usize,
    channels: usize,
    store_bytes_per_sample: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        //let new_capture_bytes = resampl.nbr_frames_needed() * channels * store_bytes_per_sample;
        //trace!(
        //    "Resampler needs {} frames, will read {} bytes",
        //    resampl.nbr_frames_needed(),
        //    new_capture_bytes
        //);
        //new_capture_bytes
        resampl.nbr_frames_needed() * channels * store_bytes_per_sample
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
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample() as i32;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let buffer_bytes = 2.0f32.powf(
            (capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
                .log2()
                .ceil(),
        ) as usize
            * 2
            * channels
            * store_bytes_per_sample;
        let sample_format = self.sample_format.clone();
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
        let async_src = resampler_is_async(&resampler_conf);
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let handle = thread::Builder::new()
            .name("PulseCapture".to_string())
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
                        let scalefactor = PrcFmt::new(2.0).powi(bits_per_sample - 1);
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
                        let mut chunk_stats;
                        let bytes_per_frame = channels * store_bytes_per_sample;
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
                            capture_bytes = get_nbr_capture_bytes(
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
                                    if averager.larger_than_millis(capture_status.read().unwrap().update_interval as u64) {
                                        let bytes_per_sec = averager.get_average();
                                        averager.restart();
                                        let measured_rate_f = bytes_per_sec / (channels * store_bytes_per_sample) as f64;
                                        trace!(
                                            "Measured sample rate is {} Hz, signal RMS is {:?}",
                                            measured_rate_f,
                                            capture_status.read().unwrap().signal_rms,
                                        );
                                        let mut capt_stat = capture_status.write().unwrap();
                                        capt_stat.measured_samplerate = measured_rate_f as usize;
                                        capt_stat.signal_range = value_range as f32;
                                        capt_stat.rate_adjust = rate_adjust as f32;
                                        capt_stat.state = state;
                                    }
                                }
                                Err(msg) => {
                                    status_channel
                                        .send(StatusMessage::CaptureError {
                                            message: format!("{}", msg),
                                        })
                                        .unwrap();
                                }
                            };
                            let mut chunk = match sample_format {
                                SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S32LE => {
                                    buffer_to_chunk_bytes(
                                        &buf[0..capture_bytes],
                                        channels,
                                        scalefactor,
                                        bits_per_sample,
                                        store_bytes_per_sample,
                                        capture_bytes,
                                        &capture_status.read().unwrap().used_channels,
                                    )
                                }
                                SampleFormat::FLOAT32LE => buffer_to_chunk_float_bytes(
                                    &buf[0..capture_bytes],
                                    channels,
                                    bits_per_sample,
                                    capture_bytes,
                                ),
                                _ => panic!("Unsupported sample format"),
                            };
                            chunk_stats = chunk.get_stats();
                            //trace!("Capture signal rms {:?}, peak {:?}", chunk_stats.rms_db(), chunk_stats.peak_db());
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
                                channel.send(msg).unwrap();
                            }
                            capture_status.write().unwrap().signal_rms = chunk_stats.rms_db();
                            capture_status.write().unwrap().signal_peak = chunk_stats.peak_db();
                        }
                        capture_status.write().unwrap().state = ProcessingState::Inactive;
                    }
                    Err(err) => {
                        let send_result = status_channel
                            .send(StatusMessage::CaptureError {
                                message: format!("{}", err),
                            });
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
