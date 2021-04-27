use audiodevice::*;
use config;
use config::NumberFamily;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};
use countertimer;
use std::fs::File;
use std::io::ErrorKind;
use std::io::{stdin, stdout, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Duration;

use rubato::Resampler;

use crate::{CaptureStatus, PlaybackStatus};
use CommandMessage;
use NewValue;
use PrcFmt;
use ProcessingState;
use Res;
use StatusMessage;

pub struct FilePlaybackDevice {
    pub destination: PlaybackDest,
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
}

#[derive(Clone)]
pub enum CaptureSource {
    Filename(String),
    Stdin,
}

#[derive(Clone)]
pub enum PlaybackDest {
    Filename(String),
    Stdout,
}

pub struct FileCaptureDevice {
    pub source: CaptureSource,
    pub chunksize: usize,
    pub samplerate: usize,
    pub enable_resampling: bool,
    pub capture_samplerate: usize,
    pub resampler_conf: config::Resampler,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub extra_samples: usize,
    pub skip_bytes: usize,
    pub read_bytes: usize,
}

struct CaptureChannels {
    audio: mpsc::SyncSender<AudioMessage>,
    status: mpsc::Sender<StatusMessage>,
    command: mpsc::Receiver<CommandMessage>,
}

struct CaptureParams {
    channels: usize,
    bits_per_sample: i32,
    sample_format: SampleFormat,
    store_bytes_per_sample: usize,
    extra_bytes: usize,
    buffer_bytes: usize,
    capture_samplerate: usize,
    silence_timeout: PrcFmt,
    silence_threshold: PrcFmt,
    chunksize: usize,
    resampling_ratio: f32,
    read_bytes: usize,
    async_src: bool,
    capture_status: Arc<RwLock<CaptureStatus>>,
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for FilePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let destination = self.destination.clone();
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample();
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format.clone();
        let handle = thread::Builder::new()
            .name("FilePlayback".to_string())
            .spawn(move || {
                let file_res: Result<Box<dyn Write>, std::io::Error> = match destination {
                    PlaybackDest::Filename(filename) => {
                        File::create(filename).map(|f| Box::new(f) as Box<dyn Write>)
                    }
                    PlaybackDest::Stdout => Ok(Box::new(stdout())),
                };
                match file_res {
                    Ok(mut file) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = PrcFmt::new(2.0).powi(bits_per_sample as i32 - 1);
                        let mut chunk_stats;
                        barrier.wait();
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes_per_sample];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    let (valid_bytes, nbr_clipped) =
                                        match sample_format.number_family() {
                                            NumberFamily::Integer => chunk_to_buffer_bytes(
                                                &chunk,
                                                &mut buffer,
                                                scalefactor,
                                                bits_per_sample as i32,
                                                store_bytes_per_sample,
                                            ),
                                            NumberFamily::Float => chunk_to_buffer_float_bytes(
                                                &chunk,
                                                &mut buffer,
                                                bits_per_sample as i32,
                                            ),
                                        };
                                    let write_res = file.write_all(&buffer[0..valid_bytes]);
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
                                    if nbr_clipped > 0 {
                                        playback_status.write().unwrap().clipped_samples +=
                                            nbr_clipped;
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

//&params.sample_format,
//params.channels,
//params.bits_per_sample,
//params.store_bytes_per_sample,
//bytes_read,
//scalefactor,
//&params.capture_status.read().unwrap().used_channels,

fn build_chunk(
    buf: &[u8],
    params: &CaptureParams,
    //sample_format: &SampleFormat,
    //channels: usize,
    //bits_per_sample: i32,
    //store_bytes_per_sample: usize,
    bytes_read: usize,
    scalefactor: PrcFmt,
    //used_channels: &[bool],
) -> AudioChunk {
    match params.sample_format.number_family() {
        NumberFamily::Integer => buffer_to_chunk_bytes(
            &buf,
            params.channels,
            scalefactor,
            params.bits_per_sample,
            params.store_bytes_per_sample,
            bytes_read,
            &params.capture_status.read().unwrap().used_channels,
        ),
        NumberFamily::Float => {
            buffer_to_chunk_float_bytes(&buf, params.channels, params.bits_per_sample, bytes_read)
        }
    }
}

fn get_capture_bytes(
    bytes_to_read: usize,
    nbr_bytes_read: usize,
    capture_bytes: usize,
    buf: &mut Vec<u8>,
) -> usize {
    let capture_bytes = if bytes_to_read == 0
        || (bytes_to_read > 0 && (nbr_bytes_read + capture_bytes) <= bytes_to_read)
    {
        capture_bytes
    } else {
        debug!("Stopping capture, reached read_bytes limit");
        bytes_to_read - nbr_bytes_read
    };
    if capture_bytes > buf.len() {
        debug!("Capture buffer too small, extending");
        buf.append(&mut vec![0u8; capture_bytes - buf.len()]);
    }
    capture_bytes
}

fn capture_loop(
    mut file: Box<dyn Read>,
    params: CaptureParams,
    msg_channels: CaptureChannels,
    mut resampler: Option<Box<dyn Resampler<PrcFmt>>>,
) {
    debug!("starting captureloop");
    let scalefactor = PrcFmt::new(2.0).powi(params.bits_per_sample - 1);
    let chunksize_bytes = params.channels * params.chunksize * params.store_bytes_per_sample;
    let bytes_per_frame = params.channels * params.store_bytes_per_sample;
    let mut buf = vec![0u8; params.buffer_bytes];
    let mut bytes_read = 0;
    let mut capture_bytes = chunksize_bytes;
    let mut capture_bytes_temp;
    let mut extra_bytes_left = params.extra_bytes;
    let mut nbr_bytes_read = 0;
    let mut averager = countertimer::TimeAverage::new();
    let mut silence_counter = countertimer::SilenceCounter::new(
        params.silence_threshold,
        params.silence_timeout,
        params.capture_samplerate,
        params.chunksize,
    );
    let mut chunk_stats;
    let mut value_range = 0.0;
    let mut rate_adjust = 0.0;
    let mut state = ProcessingState::Running;
    loop {
        match msg_channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                debug!("Exit message received, sending EndOfStream");
                let msg = AudioMessage::EndOfStream;
                msg_channels.audio.send(msg).unwrap();
                msg_channels
                    .status
                    .send(StatusMessage::CaptureDone)
                    .unwrap();
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                rate_adjust = speed;
                if let Some(resampl) = &mut resampler {
                    if params.async_src {
                        if resampl.set_resample_ratio_relative(speed).is_err() {
                            debug!("Failed to set resampling speed to {}", speed);
                        }
                    } else {
                        warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
                    }
                }
            }
            Err(_) => {}
        };
        capture_bytes = get_nbr_capture_bytes(
            &resampler,
            capture_bytes,
            params.channels,
            params.store_bytes_per_sample,
        );
        capture_bytes_temp =
            get_capture_bytes(params.read_bytes, nbr_bytes_read, capture_bytes, &mut buf);
        let read_res = read_retry(&mut file, &mut buf[0..capture_bytes_temp]);
        match read_res {
            Ok(bytes) => {
                //trace!("Captured {} bytes", bytes);
                bytes_read = bytes;
                nbr_bytes_read += bytes;
                if bytes > 0 && bytes < capture_bytes {
                    for item in buf.iter_mut().take(capture_bytes).skip(bytes) {
                        *item = 0;
                    }
                    debug!(
                        "End of file, read only {} of {} bytes",
                        bytes, capture_bytes
                    );
                    let missing =
                        ((capture_bytes - bytes) as f32 * params.resampling_ratio) as usize;
                    if extra_bytes_left > missing {
                        bytes_read = capture_bytes;
                        extra_bytes_left -= missing;
                    } else {
                        bytes_read += (extra_bytes_left as f32 / params.resampling_ratio) as usize;
                        extra_bytes_left = 0;
                    }
                } else if bytes == 0 && capture_bytes > 0 {
                    debug!("Reached end of file");
                    let extra_samples =
                        extra_bytes_left / params.store_bytes_per_sample / params.channels;
                    send_silence(
                        extra_samples,
                        params.channels,
                        params.chunksize,
                        &msg_channels.audio,
                    );
                    let msg = AudioMessage::EndOfStream;
                    msg_channels.audio.send(msg).unwrap();
                    msg_channels
                        .status
                        .send(StatusMessage::CaptureDone)
                        .unwrap();
                    break;
                }
                averager.add_value(bytes);
                if averager.larger_than_millis(
                    params.capture_status.read().unwrap().update_interval as u64,
                ) {
                    let bytes_per_sec = averager.get_average();
                    averager.restart();
                    let measured_rate_f =
                        bytes_per_sec / (params.channels * params.store_bytes_per_sample) as f64;
                    trace!("Measured sample rate is {} Hz", measured_rate_f);
                    let mut capt_stat = params.capture_status.write().unwrap();
                    capt_stat.measured_samplerate = measured_rate_f as usize;
                    capt_stat.signal_range = value_range as f32;
                    capt_stat.rate_adjust = rate_adjust as f32;
                    capt_stat.state = state;
                }
            }
            Err(err) => {
                debug!("Encountered a read error");
                msg_channels
                    .status
                    .send(StatusMessage::CaptureError {
                        message: format!("{}", err),
                    })
                    .unwrap();
            }
        };
        let mut chunk = build_chunk(
            &buf[0..capture_bytes],
            &params,
            //&params.sample_format,
            //params.channels,
            //params.bits_per_sample,
            //params.store_bytes_per_sample,
            bytes_read,
            scalefactor,
            //&params.capture_status.read().unwrap().used_channels,
        );

        value_range = chunk.maxval - chunk.minval;
        chunk_stats = chunk.get_stats();
        //trace!(
        //    "Capture rms {:?}, peak {:?}",
        //    chunk_stats.rms_db(),
        //    chunk_stats.peak_db()
        //);
        params.capture_status.write().unwrap().signal_rms = chunk_stats.rms_db();
        params.capture_status.write().unwrap().signal_peak = chunk_stats.peak_db();
        state = silence_counter.update(value_range);
        if state == ProcessingState::Running {
            if let Some(resampl) = &mut resampler {
                let new_waves = resampl.process(&chunk.waveforms).unwrap();
                let mut chunk_frames = new_waves.iter().map(|w| w.len()).max().unwrap();
                if chunk_frames == 0 {
                    chunk_frames = params.chunksize;
                }
                chunk.frames = chunk_frames;
                chunk.valid_frames =
                    (chunk.frames as f32 * (bytes_read as f32 / capture_bytes as f32)) as usize;
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            msg_channels.audio.send(msg).unwrap();
        } else {
            sleep_until_next(bytes_per_frame, params.capture_samplerate, capture_bytes);
        }
    }
    let mut capt_stat = params.capture_status.write().unwrap();
    capt_stat.state = ProcessingState::Inactive;
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for FileCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let source = self.source.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let capture_samplerate = self.capture_samplerate;
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample();
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
        let extra_bytes = self.extra_samples * store_bytes_per_sample * channels;
        let skip_bytes = self.skip_bytes;
        let read_bytes = self.read_bytes;
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let handle = thread::Builder::new()
            .name("FileCapture".to_string())
            .spawn(move || {
                let resampler = if enable_resampling {
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
                let params = CaptureParams {
                    channels,
                    bits_per_sample: bits_per_sample as i32,
                    sample_format,
                    store_bytes_per_sample,
                    extra_bytes,
                    buffer_bytes,
                    silence_threshold,
                    silence_timeout,
                    chunksize,
                    resampling_ratio: samplerate as f32 / capture_samplerate as f32,
                    read_bytes,
                    async_src,
                    capture_status,
                    capture_samplerate,
                };
                let file_res: Result<Box<dyn Read>, std::io::Error> = match source {
                    CaptureSource::Filename(filename) => {
                        File::open(filename).map(|f| Box::new(f) as Box<dyn Read>)
                    }
                    CaptureSource::Stdin => Ok(Box::new(stdin())),
                };
                match file_res {
                    Ok(mut file) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        barrier.wait();
                        let msg_channels = CaptureChannels {
                            audio: channel,
                            status: status_channel,
                            command: command_channel,
                        };
                        if skip_bytes > 0 {
                            debug!("skipping the first {} bytes", skip_bytes);
                            let mut tempbuf = vec![0u8; skip_bytes];
                            let _ = file.read_exact(&mut tempbuf);
                        }
                        debug!("starting captureloop");
                        capture_loop(file, params, msg_channels, resampler);
                    }
                    Err(err) => {
                        let send_result = status_channel.send(StatusMessage::CaptureError {
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

fn send_silence(
    samples: usize,
    channels: usize,
    chunksize: usize,
    audio_channel: &mpsc::SyncSender<AudioMessage>,
) {
    let mut samples_left = samples;
    while samples_left > 0 {
        let chunk_samples = if samples_left > chunksize {
            chunksize
        } else {
            samples_left
        };
        let waveforms = vec![vec![0.0; chunksize]; channels];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, chunksize, chunk_samples);
        let msg = AudioMessage::Audio(chunk);
        debug!("Sending extra chunk of {} frames", chunk_samples);
        audio_channel.send(msg).unwrap();
        samples_left -= chunk_samples;
    }
}

fn read_retry(file: &mut dyn Read, mut buf: &mut [u8]) -> Res<usize> {
    let requested = buf.len();
    while !buf.is_empty() {
        match file.read(buf) {
            Ok(0) => break,
            Ok(n) => {
                let tmp = buf;
                buf = &mut tmp[n..];
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {
                debug!("got Interrupted");
                thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(Box::new(e)),
        }
    }
    if !buf.is_empty() {
        Ok(requested - buf.len())
    } else {
        Ok(requested)
    }
}

fn sleep_until_next(bytes_per_frame: usize, samplerate: usize, nbr_bytes: usize) {
    let io_duration =
        Duration::from_millis((1000 * nbr_bytes) as u64 / (bytes_per_frame * samplerate) as u64);
    if io_duration > Duration::from_millis(2) {
        thread::sleep(io_duration - Duration::from_millis(2));
    }
}
