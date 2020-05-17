extern crate num_traits;
//use std::{iter, error};

use audiodevice::*;
use config;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};
use std::fs::File;
use std::io::ErrorKind;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;

use rubato::Resampler;

use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

pub struct FilePlaybackDevice {
    pub filename: String,
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

pub struct FileCaptureDevice {
    pub filename: String,
    pub chunksize: usize,
    pub samplerate: usize,
    pub enable_resampling: bool,
    pub capture_samplerate: usize,
    pub resampler_conf: config::Resampler,
    pub channels: usize,
    pub format: SampleFormat,
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

//struct PlaybackChannels {
//    audio: mpsc::Receiver<AudioMessage>,
//    status: mpsc::Sender<StatusMessage>,
//}

struct CaptureParams {
    channels: usize,
    bits: i32,
    bytes_per_sample: usize,
    format: SampleFormat,
    store_bytes: usize,
    extra_bytes: usize,
    buffer_bytes: usize,
    silent_limit: usize,
    silence: PrcFmt,
    chunksize: usize,
    resampling_ratio: f32,
    read_bytes: usize,
    async_src: bool,
}

//struct PlaybackParams {
//    scalefactor: PrcFmt,
//    target_level: usize,
//    adjust_period: f32,
//    adjust_enabled: bool,
//}
//
/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for FilePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let filename = self.filename.clone();
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
        let store_bytes = match self.format {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE => 4,
            SampleFormat::S24LE3 => 3,
            SampleFormat::S32LE => 4,
            SampleFormat::FLOAT32LE => 4,
            SampleFormat::FLOAT64LE => 8,
        };
        let format = self.format.clone();
        let handle = thread::Builder::new()
            .name("FilePlayback".to_string())
            .spawn(move || {
                //let delay = time::Duration::from_millis((4*1000*chunksize/samplerate) as u64);
                match File::create(filename) {
                    Ok(mut file) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        //let scalefactor = (1<<bits-1) as PrcFmt;
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                        barrier.wait();
                        //thread::sleep(delay);
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    let bytes = match format {
                                        SampleFormat::S16LE
                                        | SampleFormat::S24LE
                                        | SampleFormat::S24LE3
                                        | SampleFormat::S32LE => chunk_to_buffer_bytes(
                                            chunk,
                                            &mut buffer,
                                            scalefactor,
                                            bits,
                                            store_bytes,
                                        ),
                                        SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE => {
                                            chunk_to_buffer_float_bytes(chunk, &mut buffer, bits)
                                        }
                                    };
                                    let write_res = file.write(&buffer[0..bytes]);
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

fn get_nbr_capture_bytes(
    resampler: &Option<Box<dyn Resampler<PrcFmt>>>,
    capture_bytes: usize,
    channels: usize,
    store_bytes: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        let new_capture_bytes = resampl.nbr_frames_needed() * channels * store_bytes;
        trace!(
            "Resampler needs {} frames, will read {} bytes",
            resampl.nbr_frames_needed(),
            new_capture_bytes
        );
        new_capture_bytes
    } else {
        capture_bytes
    }
}

fn build_chunk(
    buf: &[u8],
    format: &SampleFormat,
    channels: usize,
    bits: i32,
    bytes_per_sample: usize,
    bytes_read: usize,
    scalefactor: PrcFmt,
) -> AudioChunk {
    match format {
        SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S24LE3 | SampleFormat::S32LE => {
            buffer_to_chunk_bytes(&buf, channels, scalefactor, bytes_per_sample, bytes_read)
        }
        SampleFormat::FLOAT32LE | SampleFormat::FLOAT64LE => {
            buffer_to_chunk_float_bytes(&buf, channels, bits, bytes_read)
        }
    }
}

fn get_capture_bytes(bytes_to_read: usize, nbr_bytes_read: usize, capture_bytes: usize) -> usize {
    if bytes_to_read == 0
        || (bytes_to_read > 0 && (nbr_bytes_read + capture_bytes) <= bytes_to_read)
    {
        capture_bytes
    } else {
        debug!("Stopping capture, reached read_bytes limit");
        bytes_to_read - nbr_bytes_read
    }
}

fn capture_loop(
    mut file: File,
    params: CaptureParams,
    msg_channels: CaptureChannels,
    mut resampler: Option<Box<dyn Resampler<PrcFmt>>>,
) {
    debug!("starting captureloop");
    let scalefactor = (2.0 as PrcFmt).powi(params.bits - 1);
    let mut silent_nbr: usize = 0;
    let chunksize_bytes = params.channels * params.chunksize * params.store_bytes;
    let mut buf = vec![0u8; params.buffer_bytes];
    let mut bytes_read = 0;
    let mut capture_bytes = chunksize_bytes;
    let mut capture_bytes_temp;
    let mut extra_bytes_left = params.extra_bytes;
    let mut nbr_bytes_read = 0;
    loop {
        match msg_channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                let msg = AudioMessage::EndOfStream;
                msg_channels.audio.send(msg).unwrap();
                msg_channels
                    .status
                    .send(StatusMessage::CaptureDone)
                    .unwrap();
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                if let Some(resampl) = &mut resampler {
                    if !params.async_src {
                        warn!("Adjusting rate of Sync type resampler. Switch to Async for much improved quality");
                    }
                    if resampl.set_resample_ratio_relative(speed).is_err() {
                        debug!("Failed to set resampling speed to {}", speed);
                    }
                }
            }
            Err(_) => {}
        };
        capture_bytes = get_nbr_capture_bytes(
            &resampler,
            capture_bytes,
            params.channels,
            params.store_bytes,
        );
        capture_bytes_temp = get_capture_bytes(params.read_bytes, nbr_bytes_read, capture_bytes);
        let read_res = read_retry(&mut file, &mut buf[0..capture_bytes_temp]);
        match read_res {
            Ok(bytes) => {
                trace!("Captured {} bytes", bytes);
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
                } else if bytes == 0 {
                    debug!("Reached end of file");
                    let extra_samples = extra_bytes_left / params.store_bytes / params.channels;
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
        //let before = Instant::now();
        let mut chunk = build_chunk(
            &buf[0..capture_bytes],
            &params.format,
            params.channels,
            params.bits,
            params.bytes_per_sample,
            bytes_read,
            scalefactor,
        );
        if (chunk.maxval - chunk.minval) > params.silence {
            if silent_nbr > params.silent_limit {
                debug!("Resuming processing");
            }
            silent_nbr = 0;
        } else if params.silent_limit > 0 {
            if silent_nbr == params.silent_limit {
                debug!("Pausing processing");
            }
            silent_nbr += 1;
        }
        if silent_nbr <= params.silent_limit {
            if let Some(resampl) = &mut resampler {
                let new_waves = resampl.process(&chunk.waveforms).unwrap();
                chunk.frames = new_waves[0].len();
                chunk.valid_frames = (new_waves[0].len() as f32
                    * (bytes_read as f32 / capture_bytes as f32))
                    as usize;
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            msg_channels.audio.send(msg).unwrap();
        }
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for FileCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let filename = self.filename.clone();
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let capture_samplerate = self.capture_samplerate;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S24LE3 => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let store_bytes = match self.format {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE => 4,
            SampleFormat::S24LE3 => 3,
            SampleFormat::S32LE => 4,
            SampleFormat::FLOAT32LE => 4,
            SampleFormat::FLOAT64LE => 8,
        };
        let buffer_bytes = 2.0f32.powf(
            (capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
                .log2()
                .ceil(),
        ) as usize
            * 2
            * channels
            * store_bytes;
        let format = self.format.clone();
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
        let async_src = resampler_is_async(&resampler_conf);
        let extra_bytes = self.extra_samples * store_bytes * channels;
        let skip_bytes = self.skip_bytes;
        let read_bytes = self.read_bytes;
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
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
                match File::open(filename) {
                    Ok(mut file) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        barrier.wait();
                        let params = CaptureParams {
                            channels,
                            bits,
                            bytes_per_sample: store_bytes,
                            format,
                            store_bytes,
                            extra_bytes,
                            buffer_bytes,
                            silent_limit,
                            silence,
                            chunksize,
                            resampling_ratio: samplerate as f32 / capture_samplerate as f32,
                            read_bytes,
                            async_src,
                        };
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
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, chunk_samples);
        let msg = AudioMessage::Audio(chunk);
        debug!("Sending extra chunk of {} frames", chunk_samples);
        audio_channel.send(msg).unwrap();
        samples_left -= chunk_samples;
    }
}

fn read_retry(file: &mut File, mut buf: &mut [u8]) -> Res<usize> {
    let requested = buf.len();
    while !buf.is_empty() {
        match file.read(buf) {
            Ok(0) => break,
            Ok(n) => {
                let tmp = buf;
                buf = &mut tmp[n..];
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(Box::new(e)),
        }
    }
    if !buf.is_empty() {
        Ok(requested - buf.len())
    } else {
        Ok(requested)
    }
}
