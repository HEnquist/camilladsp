extern crate num_traits;
//use std::{iter, error};
use pulse;

use psimple::Simple;
use pulse::sample;
use pulse::stream::Direction;

use rubato::Resampler;

use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
//mod audiodevice;
use audiodevice::*;
// Sample format
use config;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};

use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

pub struct PulsePlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub format: SampleFormat,
}

pub struct PulseCaptureDevice {
    pub devname: String,
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

/// Open a PulseAudio device
fn open_pulse(
    devname: String,
    samplerate: u32,
    channels: u8,
    format: &SampleFormat,
    capture: bool,
) -> Res<Simple> {
    // Open the device
    let dir = if capture {
        Direction::Record
    } else {
        Direction::Playback
    };

    let pulse_format = match format {
        SampleFormat::S16LE => sample::SAMPLE_S16NE,
        SampleFormat::S24LE => sample::SAMPLE_S24_32NE,
        SampleFormat::S24LE3 => sample::SAMPLE_S24NE,
        SampleFormat::S32LE => sample::SAMPLE_S32NE,
        SampleFormat::FLOAT32LE => sample::SAMPLE_FLOAT32NE,
        _ => panic!("invalid format"),
    };

    let bytes = match format {
        SampleFormat::S16LE => 2,
        SampleFormat::S24LE => 4,
        SampleFormat::S24LE3 => 3,
        SampleFormat::S32LE => 4,
        SampleFormat::FLOAT32LE => 4,
        SampleFormat::FLOAT64LE => 8,
    };

    let spec = sample::Spec {
        format: pulse_format,
        channels,
        rate: samplerate,
    };
    //assert!(spec.is_valid());
    let attr = pulse::def::BufferAttr {
        maxlength: std::u32::MAX,
        tlength: std::u32::MAX,
        prebuf: bytes as u32,
        minreq: std::u32::MAX,
        fragsize: bytes as u32,
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
            .name("PulsePlayback".to_string())
            .spawn(move || {
                //let delay = time::Duration::from_millis((4*1000*chunksize/samplerate) as u64);
                match open_pulse(devname, samplerate as u32, channels as u8, &format, false) {
                    Ok(pulsedevice) => {
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
                                    match format {
                                        SampleFormat::S16LE
                                        | SampleFormat::S24LE
                                        | SampleFormat::S32LE => {
                                            chunk_to_buffer_bytes(
                                                chunk,
                                                &mut buffer,
                                                scalefactor,
                                                bits,
                                                store_bytes,
                                            );
                                        }
                                        SampleFormat::FLOAT32LE => {
                                            chunk_to_buffer_float_bytes(chunk, &mut buffer, bits);
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

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for PulseCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
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
        let store_bytes = match self.format {
            SampleFormat::S16LE => 2,
            SampleFormat::S24LE3 => 3,
            SampleFormat::S24LE => 4,
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
                    &format,
                    true,
                ) {
                    Ok(pulsedevice) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                        let mut silent_nbr: usize = 0;
                        barrier.wait();
                        debug!("starting captureloop");
                        let mut buf = vec![0u8; buffer_bytes];
                        let chunksize_bytes = channels * chunksize * store_bytes;
                        let mut capture_bytes = chunksize_bytes;
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
                                store_bytes,
                            );
                            if capture_bytes > buf.len() {
                                debug!("Capture buffer too small, extending");
                                buf.append(&mut vec![0u8; capture_bytes - buf.len()]);
                            }
                            let read_res = pulsedevice.read(&mut buf[0..capture_bytes]);
                            match read_res {
                                Ok(()) => {}
                                Err(msg) => {
                                    status_channel
                                        .send(StatusMessage::CaptureError {
                                            message: format!("{}", msg),
                                        })
                                        .unwrap();
                                }
                            };
                            //let before = Instant::now();
                            let mut chunk = match format {
                                SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S32LE => {
                                    buffer_to_chunk_bytes(
                                        &buf[0..capture_bytes],
                                        channels,
                                        scalefactor,
                                        store_bytes,
                                        capture_bytes,
                                    )
                                }
                                SampleFormat::FLOAT32LE => buffer_to_chunk_float_bytes(
                                    &buf[0..capture_bytes],
                                    channels,
                                    bits,
                                    capture_bytes,
                                ),
                                _ => panic!("Unsupported sample format"),
                            };
                            if (chunk.maxval - chunk.minval) > silence {
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
