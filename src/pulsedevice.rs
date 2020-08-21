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
use rubato::Resampler;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::SystemTime;

use crate::CaptureStatus;
use CommandMessage;
use PrcFmt;
use ProcessingState;
use Res;
use StatusMessage;

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
        SampleFormat::S16LE => sample::SAMPLE_S16NE,
        SampleFormat::S24LE => sample::SAMPLE_S24_32NE,
        SampleFormat::S24LE3 => sample::SAMPLE_S24NE,
        SampleFormat::S32LE => sample::SAMPLE_S32NE,
        SampleFormat::FLOAT32LE => sample::SAMPLE_FLOAT32NE,
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

    let pulsedev = Simple::new(
        None,           // Use the default server
        "CamillaDSP",   // Our application’s name
        dir,            // We want a playback stream
        Some(&devname), // Use the default device
        "ToDSP",        // Description of our stream
        &spec,          // Our sample format
        None,           // Use default channel map
        Some(&attr),    // Use default buffering attributes
    )
    .unwrap();
    Ok(pulsedev)
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for PulsePlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
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
                //let delay = time::Duration::from_millis((4*1000*chunksize/samplerate) as u64);
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
                        //let scalefactor = (1<<bits-1) as PrcFmt;
                        let scalefactor = (2.0 as PrcFmt).powi(bits_per_sample - 1);
                        barrier.wait();
                        //thread::sleep(delay);
                        debug!("starting playback loop");
                        let mut buffer = vec![0u8; chunksize * channels * store_bytes_per_sample];
                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    match sample_format {
                                        SampleFormat::S16LE
                                        | SampleFormat::S24LE
                                        | SampleFormat::S32LE => {
                                            chunk_to_buffer_bytes(
                                                chunk,
                                                &mut buffer,
                                                scalefactor,
                                                bits_per_sample,
                                                store_bytes_per_sample,
                                            );
                                        }
                                        SampleFormat::FLOAT32LE => {
                                            chunk_to_buffer_float_bytes(
                                                chunk,
                                                &mut buffer,
                                                bits_per_sample,
                                            );
                                        }
                                        _ => panic!("Unsupported sample format!"),
                                    };
                                    // let _frames = match io.writei(&buffer[..]) {
                                    let write_res = pulsedevice.write(&buffer);
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
    store_bytes_per_sample: usize,
) -> usize {
    if let Some(resampl) = &resampler {
        let new_capture_bytes = resampl.nbr_frames_needed() * channels * store_bytes_per_sample;
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
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
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
                        let scalefactor = (2.0 as PrcFmt).powi(bits_per_sample - 1);
                        let mut silent_nbr: usize = 0;
                        barrier.wait();
                        debug!("starting captureloop");
                        let mut buf = vec![0u8; buffer_bytes];
                        let chunksize_bytes = channels * chunksize * store_bytes_per_sample;
                        let mut capture_bytes = chunksize_bytes;
                        let mut start = SystemTime::now();
                        let mut now;
                        let mut bytes_counter = 0;
                        let mut value_range = 0.0;
                        let mut rate_adjust = 0.0;
                        let mut state = ProcessingState::Running;
                        loop {
                            match command_channel.try_recv() {
                                Ok(CommandMessage::Exit) => {
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
                            let read_res = pulsedevice.read(&mut buf[0..capture_bytes]);
                            match read_res {
                                Ok(()) => {
                                    now = SystemTime::now();
                                    bytes_counter += capture_bytes;
                                    if now.duration_since(start).unwrap().as_millis() as usize > capture_status.read().unwrap().update_interval {
                                        let meas_time = now.duration_since(start).unwrap().as_secs_f32();
                                        let bytes_per_sec = bytes_counter as f32 / meas_time;
                                        let measured_rate_f = bytes_per_sec / (channels * store_bytes_per_sample) as f32;
                                        trace!(
                                            "Measured sample rate is {} Hz",
                                            measured_rate_f
                                        );
                                        let mut capt_stat = capture_status.write().unwrap();
                                        capt_stat.measured_samplerate = measured_rate_f as usize;
                                        capt_stat.signal_range = value_range as f32;
                                        capt_stat.rate_adjust = rate_adjust as f32;
                                        capt_stat.state = state;
                                        start = now;
                                        bytes_counter = 0;
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
                            //let before = Instant::now();
                            let mut chunk = match sample_format {
                                SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S32LE => {
                                    buffer_to_chunk_bytes(
                                        &buf[0..capture_bytes],
                                        channels,
                                        scalefactor,
                                        store_bytes_per_sample,
                                        capture_bytes,
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
                            value_range = chunk.maxval - chunk.minval;
                            if (value_range) > silence {
                                if silent_nbr > silent_limit {
                                    state = ProcessingState::Running;
                                    debug!("Resuming processing");
                                }
                                silent_nbr = 0;
                            } else if silent_limit > 0 {
                                if silent_nbr == silent_limit {
                                    state = ProcessingState::Paused;
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
                        let mut capt_stat = capture_status.write().unwrap();
                        capt_stat.state = ProcessingState::Inactive;
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
