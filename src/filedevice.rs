use crate::audiodevice::*;
use crate::config;
use crate::config::SampleFormat;
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;

use std::error::Error;
use std::fs::File;
#[cfg(target_os = "linux")]
use std::fs::OpenOptions;
use std::io::{stdin, stdout, Write};
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use parking_lot::{RwLock, RwLockUpgradableReadGuard};
use rubato::VecResampler;

#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
use crate::filedevice_bluez;
#[cfg(not(target_os = "linux"))]
use crate::filereader::BlockingReader;
#[cfg(target_os = "linux")]
use crate::filereader_nonblock::NonBlockingReader;
use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

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
    #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
    BluezDBus(String, String),
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
    pub capture_samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub extra_samples: usize,
    pub skip_bytes: usize,
    pub read_bytes: usize,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

struct CaptureChannels {
    audio: mpsc::SyncSender<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
    command: mpsc::Receiver<CommandMessage>,
}

struct CaptureParams {
    channels: usize,
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
    stop_on_rate_change: bool,
    rate_measure_interval: f32,
}
#[derive(Debug)]
pub enum ReadResult {
    Complete(usize),
    Timeout(usize),
    EndOfFile(usize),
}

pub trait Reader {
    fn read(&mut self, data: &mut [u8]) -> Result<ReadResult, Box<dyn Error>>;
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for FilePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let destination = self.destination.clone();
        let chunksize = self.chunksize;
        let channels = self.channels;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format;
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
                        let mut chunk_stats = ChunkStats {
                            rms: vec![0.0; channels],
                            peak: vec![0.0; channels],
                        };
                        barrier.wait();
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes_per_sample];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    let (valid_bytes, nbr_clipped) = chunk_to_buffer_rawbytes(
                                        &chunk,
                                        &mut buffer,
                                        &sample_format,
                                    );
                                    let write_res = file.write_all(&buffer[0..valid_bytes]);
                                    match write_res {
                                        Ok(_) => {}
                                        Err(err) => {
                                            status_channel
                                                .send(StatusMessage::PlaybackError(err.to_string()))
                                                .unwrap_or(());
                                        }
                                    };
                                    chunk.update_stats(&mut chunk_stats);
                                    {
                                        let mut playback_status = playback_status.write();
                                        if nbr_clipped > 0 {
                                            playback_status.clipped_samples += nbr_clipped;
                                        }
                                        playback_status
                                            .signal_rms
                                            .add_record_squared(chunk_stats.rms_linear());
                                        playback_status
                                            .signal_peak
                                            .add_record(chunk_stats.peak_linear());
                                    }
                                    trace!(
                                        "Playback signal RMS: {:?}, peak: {:?}",
                                        chunk_stats.rms_db(),
                                        chunk_stats.peak_db()
                                    );
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

fn capture_bytes(
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
    mut file: Box<dyn Reader>,
    params: CaptureParams,
    msg_channels: CaptureChannels,
    mut resampler: Option<Box<dyn VecResampler<PrcFmt>>>,
) {
    debug!("starting captureloop");
    let chunksize_bytes = params.channels * params.chunksize * params.store_bytes_per_sample;
    let bytes_per_frame = params.channels * params.store_bytes_per_sample;
    let mut buf = vec![0u8; params.buffer_bytes];
    let mut bytes_read = 0;
    let mut bytes_to_capture = chunksize_bytes;
    let mut bytes_to_capture_tmp;
    let mut extra_bytes_left = params.extra_bytes;
    let mut nbr_bytes_read = 0;
    let rate_measure_interval_ms = (1000.0 * params.rate_measure_interval) as u64;
    let mut averager = countertimer::TimeAverage::new();
    let mut watcher_averager = countertimer::TimeAverage::new();
    let mut valuewatcher = countertimer::ValueWatcher::new(
        params.capture_samplerate as f32,
        RATE_CHANGE_THRESHOLD_VALUE,
        RATE_CHANGE_THRESHOLD_COUNT,
    );
    let mut silence_counter = countertimer::SilenceCounter::new(
        params.silence_threshold,
        params.silence_timeout,
        params.capture_samplerate,
        params.chunksize,
    );

    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut value_range = 0.0;
    let mut rate_adjust = 0.0;
    let mut state = ProcessingState::Running;
    let mut prev_state = ProcessingState::Running;
    let mut stalled = false;
    let mut channel_mask = vec![true; params.channels];
    loop {
        match msg_channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                debug!("Exit message received, sending EndOfStream");
                let msg = AudioMessage::EndOfStream;
                msg_channels.audio.send(msg).unwrap_or(());
                msg_channels
                    .status
                    .send(StatusMessage::CaptureDone)
                    .unwrap_or(());
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                rate_adjust = speed;
                if let Some(resampl) = &mut resampler {
                    if params.async_src {
                        if resampl.set_resample_ratio_relative(speed, true).is_err() {
                            debug!("Failed to set resampling speed to {}", speed);
                        }
                    } else {
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
        bytes_to_capture = nbr_capture_bytes(
            &resampler,
            bytes_to_capture,
            params.channels,
            params.store_bytes_per_sample,
        );
        bytes_to_capture_tmp = capture_bytes(
            params.read_bytes,
            nbr_bytes_read,
            bytes_to_capture,
            &mut buf,
        );
        //let read_res = read_retry(&mut file, &mut buf[0..capture_bytes_temp]);
        let read_res = file.read(&mut buf[0..bytes_to_capture_tmp]);
        match read_res {
            Ok(ReadResult::EndOfFile(bytes)) => {
                bytes_read = bytes;
                nbr_bytes_read += bytes;
                if bytes > 0 {
                    for item in buf.iter_mut().take(bytes_to_capture).skip(bytes) {
                        *item = 0;
                    }
                    debug!(
                        "End of file, read only {} of {} bytes",
                        bytes, bytes_to_capture
                    );
                    let missing =
                        ((bytes_to_capture - bytes) as f32 * params.resampling_ratio) as usize;
                    if extra_bytes_left > missing {
                        bytes_read = bytes_to_capture;
                        extra_bytes_left -= missing;
                    } else {
                        bytes_read += (extra_bytes_left as f32 / params.resampling_ratio) as usize;
                        extra_bytes_left = 0;
                    }
                } else {
                    debug!("Reached end of file");
                    let extra_samples =
                        extra_bytes_left / params.store_bytes_per_sample / params.channels;
                    send_silence(
                        extra_samples,
                        params.channels,
                        params.chunksize,
                        &msg_channels.audio,
                        &mut resampler,
                    );
                    let msg = AudioMessage::EndOfStream;
                    msg_channels.audio.send(msg).unwrap_or(());
                    msg_channels
                        .status
                        .send(StatusMessage::CaptureDone)
                        .unwrap_or(());
                    break;
                }
            }
            Ok(ReadResult::Timeout(bytes)) => {
                bytes_read = bytes;
                nbr_bytes_read += bytes;
                if bytes > 0 {
                    for item in buf.iter_mut().take(bytes_to_capture).skip(bytes) {
                        *item = 0;
                    }
                    debug!(
                        "Timed out after reading {} of {} bytes",
                        bytes, bytes_to_capture
                    );
                    let missing =
                        ((bytes_to_capture - bytes) as f32 * params.resampling_ratio) as usize;
                    if extra_bytes_left > missing {
                        bytes_read = bytes_to_capture;
                        extra_bytes_left -= missing;
                    } else {
                        bytes_read += (extra_bytes_left as f32 / params.resampling_ratio) as usize;
                        extra_bytes_left = 0;
                    }
                } else {
                    trace!("Read timed out");
                    let msg = AudioMessage::Pause;
                    msg_channels.audio.send(msg).unwrap_or(());

                    if !stalled {
                        debug!("Entering stalled state");
                        stalled = true;
                        prev_state = state;
                        state = ProcessingState::Stalled;
                        params.capture_status.write().state = ProcessingState::Stalled;
                    }
                    continue;
                }
            }
            Ok(ReadResult::Complete(bytes)) => {
                if stalled {
                    debug!("Leaving stalled state, resuming processing");
                    stalled = false;
                    state = prev_state;
                    params.capture_status.write().state = state;
                }
                bytes_read = bytes;
                nbr_bytes_read += bytes;
                averager.add_value(bytes);

                {
                    let capture_status = params.capture_status.upgradable_read();
                    if averager.larger_than_millis(capture_status.update_interval as u64) {
                        let bytes_per_sec = averager.average();
                        averager.restart();
                        let measured_rate_f = bytes_per_sec
                            / (params.channels * params.store_bytes_per_sample) as f64;
                        trace!("Measured sample rate is {:.1} Hz", measured_rate_f);
                        let mut capture_status = RwLockUpgradableReadGuard::upgrade(capture_status); // to write lock
                        capture_status.measured_samplerate = measured_rate_f as usize;
                        capture_status.signal_range = value_range as f32;
                        capture_status.rate_adjust = rate_adjust as f32;
                        capture_status.state = state;
                    }
                }
                watcher_averager.add_value(bytes);
                if watcher_averager.larger_than_millis(rate_measure_interval_ms) {
                    let bytes_per_sec = watcher_averager.average();
                    watcher_averager.restart();
                    let measured_rate_f =
                        bytes_per_sec / (params.channels * params.store_bytes_per_sample) as f64;
                    let changed = valuewatcher.check_value(measured_rate_f as f32);
                    if changed {
                        warn!(
                            "sample rate change detected, last rate was {} Hz",
                            measured_rate_f
                        );
                        if params.stop_on_rate_change {
                            let msg = AudioMessage::EndOfStream;
                            msg_channels.audio.send(msg).unwrap_or(());
                            msg_channels
                                .status
                                .send(StatusMessage::CaptureFormatChange(measured_rate_f as usize))
                                .unwrap_or(());
                            break;
                        }
                    }
                    trace!("Measured sample rate is {:.1} Hz", measured_rate_f);
                }
            }
            Err(err) => {
                debug!("Encountered a read error");
                msg_channels
                    .status
                    .send(StatusMessage::CaptureError(err.to_string()))
                    .unwrap_or(());
            }
        };
        let mut chunk = buffer_to_chunk_rawbytes(
            &buf[0..bytes_to_capture],
            params.channels,
            &params.sample_format,
            bytes_read,
            &params.capture_status.read().used_channels,
        );
        chunk.update_stats(&mut chunk_stats);
        //trace!(
        //    "Capture rms {:?}, peak {:?}",
        //    chunk_stats.rms_db(),
        //    chunk_stats.peak_db()
        //);
        {
            let mut capture_status = params.capture_status.write();
            capture_status
                .signal_rms
                .add_record_squared(chunk_stats.rms_linear());
            capture_status
                .signal_peak
                .add_record(chunk_stats.peak_linear());
        }
        value_range = chunk.maxval - chunk.minval;
        state = silence_counter.update(value_range);
        if state == ProcessingState::Running {
            if let Some(resampl) = &mut resampler {
                chunk.update_channel_mask(&mut channel_mask);
                let new_waves = resampl
                    .process(&chunk.waveforms, Some(&channel_mask))
                    .unwrap();
                let mut chunk_frames = new_waves.iter().map(|w| w.len()).max().unwrap();
                if chunk_frames == 0 {
                    chunk_frames = params.chunksize;
                }
                chunk.frames = chunk_frames;
                chunk.valid_frames =
                    (chunk.frames as f32 * (bytes_read as f32 / bytes_to_capture as f32)) as usize;
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            if msg_channels.audio.send(msg).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
        } else if state == ProcessingState::Paused {
            let msg = AudioMessage::Pause;
            if msg_channels.audio.send(msg).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
            sleep_until_next(bytes_per_frame, params.capture_samplerate, bytes_to_capture);
        } else {
            sleep_until_next(bytes_per_frame, params.capture_samplerate, bytes_to_capture);
        }
    }
    params.capture_status.write().state = ProcessingState::Inactive;
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for FileCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let source = self.source.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let capture_samplerate = self.capture_samplerate;
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
        let extra_bytes = self.extra_samples * store_bytes_per_sample * channels;
        let skip_bytes = self.skip_bytes;
        let read_bytes = self.read_bytes;
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = self.rate_measure_interval;
        let handle = thread::Builder::new()
            .name("FileCapture".to_string())
            .spawn(move || {
                let resampler = new_resampler(
                    &resampler_config,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                );
                let params = CaptureParams {
                    channels,
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
                    stop_on_rate_change,
                    rate_measure_interval,
                };
                #[cfg(not(target_os = "linux"))]
                let file_res: Result<Box<dyn Reader>, std::io::Error> = match source {
                    CaptureSource::Filename(filename) => File::open(filename)
                        .map(|f| Box::new(BlockingReader::new(f)) as Box<dyn Reader>),
                    CaptureSource::Stdin => Ok(Box::new(BlockingReader::new(stdin()))),
                };
                #[cfg(target_os = "linux")]
                let file_res: Result<Box<dyn Reader>, Box<dyn Error>> = match source {
                    CaptureSource::Filename(filename) => OpenOptions::new()
                        .read(true)
                        .custom_flags(nix::libc::O_NONBLOCK)
                        .open(filename)
                        .map(|f| {
                            Box::new(NonBlockingReader::new(
                                f,
                                2 * 1000 * chunksize as u64 / samplerate as u64,
                            )) as Box<dyn Reader>
                        })
                        .map_err(|e| e.into()),
                    CaptureSource::Stdin => Ok(Box::new(NonBlockingReader::new(
                        stdin(),
                        2 * 1000 * chunksize as u64 / samplerate as u64,
                    ))),
                    #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
                    CaptureSource::BluezDBus(service, path) => {
                        filedevice_bluez::open_bluez_dbus_fd(service, path, chunksize, samplerate)
                            .map(|r| r as Box<dyn Reader>)
                            .map_err(|e| e.into())
                    }
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
                            let _ = file.read(&mut tempbuf);
                        }

                        debug!("starting captureloop");
                        capture_loop(file, params, msg_channels, resampler);
                    }
                    Err(err) => {
                        let send_result =
                            status_channel.send(StatusMessage::CaptureError(err.to_string()));
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
    resampler: &mut Option<Box<dyn VecResampler<PrcFmt>>>,
) {
    let mut samples_left = samples;
    while samples_left > 0 {
        let chunk_samples = if samples_left > chunksize {
            chunksize
        } else {
            samples_left
        };
        let waveforms = if let Some(resamp) = resampler {
            resamp.process_partial(None, None).unwrap()
        } else {
            vec![vec![0.0; chunksize]; channels]
        };
        // Take a shortcut and set maxval = minval = 0 because we are anyway very near the end.
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, chunksize, chunk_samples);
        let msg = AudioMessage::Audio(chunk);
        debug!("Sending extra chunk of {} frames", chunk_samples);
        audio_channel.send(msg).unwrap_or(());
        samples_left -= chunk_samples;
    }
}

fn sleep_until_next(bytes_per_frame: usize, samplerate: usize, nbr_bytes: usize) {
    let io_duration =
        Duration::from_millis((1000 * nbr_bytes) as u64 / (bytes_per_frame * samplerate) as u64);
    if io_duration > Duration::from_millis(2) {
        thread::sleep(io_duration - Duration::from_millis(2));
    }
}
