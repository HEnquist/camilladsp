use audiodevice::*;
use config;
use config::{ConfigError, SampleFormat};
use conversions::{
    queue_to_chunk_int, queue_to_chunk_float, chunk_to_queue_int, chunk_to_queue_float
};
use rubato::Resampler;
use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use cpal::traits::{DeviceTrait, EventLoopTrait, HostTrait};
use cpal::{Device, EventLoop, Host};
use cpal::{HostId, Sample};

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
}

#[derive(Clone, Debug)]
pub struct CpalPlaybackDevice {
    pub devname: String,
    pub host: CpalHost,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub format: SampleFormat,
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

fn open_cpal_playback(host_cfg: CpalHost, devname: &String, samplerate: usize, channels: usize, format: &SampleFormat) -> Res<(Host, Device, EventLoop)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(target_os = "windows")]
        CpalHost::Wasapi => HostId::Wasapi,
    };
    let host = cpal::host_from_id(host_id)?;
    let mut devices = host.devices()?;
    let device = match devices.find(|dev| match dev.name() {
            Ok(n) => &n == devname,
            _ => false,
        }) {
            Some(dev) => dev,
            None => return Err(Box::new(ConfigError::new(&format!("Could not find device: {}", devname)))),
        };
    //let device = devices.nth(dev_idx).unwrap();
    let format = device.default_output_format()?;
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_output_stream(&device, &format)?;
    event_loop.play_stream(stream_id.clone())?;
    debug!("Opened CPAL playback device {}", devname);
    Ok((host, device, event_loop))
}

fn open_cpal_capture(host_cfg: CpalHost, devname: &String, sampelrate: usize, format: SampleFormat) -> Res<(Host, Device, EventLoop)> {
    let host_id = match host_cfg {
        #[cfg(target_os = "macos")]
        CpalHost::CoreAudio => HostId::CoreAudio,
        #[cfg(target_os = "windows")]
        CpalHost::Wasapi => HostId::Wasapi,
    };
    let host = cpal::host_from_id(host_id)?;
    let mut devices = host.devices()?;
    let dev_idx = match devices.position(|dev| match dev.name() {
        Ok(n) => &n == devname,
        _ => false,
    }) {
        Some(idx) => idx,
        None => return Err(Box::new(ConfigError::new(&format!("Could not find device: {}", devname)))),
    };
    let device = devices.nth(dev_idx).unwrap();
    let format = device.default_input_format()?;
    let event_loop = host.event_loop();
    let stream_id = event_loop.build_input_stream(&device, &format)?;
    event_loop.play_stream(stream_id.clone())?;
    Ok((host, device, event_loop))
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
                    Ok((host, device, event_loop)) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                        barrier.wait();
                        debug!("starting playback loop");
                        match format {
                            SampleFormat::S16LE => {
                                let samples_needed = Arc::new(AtomicUsize::new(0));
                                let sample_queue: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::with_capacity(4*chunksize*channels)));
                                let mut sample_queue_clone = sample_queue.clone();
                                let samples_needed_clone = samples_needed.clone();
                                std::thread::spawn(move || {
                                    event_loop.run(move |id, result| {
                                        let data = match result {
                                            Ok(data) => data,
                                            Err(err) => {
                                                error!("an error occurred on stream {:?}: {}", id, err);
                                                return;
                                            }
                                        };
                                        trace!("Callback called");
                                        match data {
                                            cpal::StreamData::Output { buffer: cpal::UnknownTypeOutputBuffer::I16(mut buffer) } => {
                                                //let curr_len = sharedqueue.lock().unwrap().len();
                                                trace!("Notify number of samples, want {}", buffer.len());
                                                samples_needed_clone.store(buffer.len(), Ordering::Relaxed);
                                                trace!("Wait for lock");
                                                while sample_queue_clone.lock().unwrap().len() < buffer.len() {
                                                    std::thread::sleep(std::time::Duration::from_millis(1));
                                                }
                                                write_data_to_device(&mut buffer, &mut sample_queue_clone.lock().unwrap());
                                                samples_needed_clone.store(0, Ordering::Relaxed);
                                            },
                                            _ => (),
                                        };
                                    });
                                });
                                loop {
                                    match channel.recv() {
                                        Ok(AudioMessage::Audio(chunk)) => {
                                            trace!("Received chunk");
                                            while sample_queue.lock().unwrap().len() < samples_needed.load(Ordering::Relaxed) {
                                                std::thread::sleep(std::time::Duration::from_millis(1));
                                            }
                                            trace!("Convert chunk");
                                            chunk_to_queue_int(
                                                chunk,
                                                &mut sample_queue.lock().unwrap(),
                                                scalefactor,
                                            );
                                        }
                                        Ok(AudioMessage::EndOfStream) => {
                                            status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                            break;
                                        }
                                        Err(_) => {}
                                    }
                                }
                            },
                            SampleFormat::FLOAT32LE => {
                                let samples_needed = Arc::new(AtomicUsize::new(0));
                                let sample_queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(4*chunksize*channels)));
                                let mut sample_queue_clone = sample_queue.clone();
                                let samples_needed_clone = samples_needed.clone();
                                std::thread::spawn(move || {
                                    event_loop.run(move |id, result| {
                                        let data = match result {
                                            Ok(data) => data,
                                            Err(err) => {
                                                error!("an error occurred on stream {:?}: {}", id, err);
                                                return;
                                            }
                                        };
                                        trace!("Callback called");
                                        match data {
                                            cpal::StreamData::Output { buffer: cpal::UnknownTypeOutputBuffer::F32(mut buffer) } => {
                                                //let curr_len = sharedqueue.lock().unwrap().len();
                                                trace!("Notify number of samples, want {}", buffer.len());
                                                samples_needed_clone.store(buffer.len(), Ordering::Relaxed);
                                                trace!("Wait fort enough samples");
                                                while sample_queue_clone.lock().unwrap().len() < buffer.len() {
                                                    std::thread::sleep(std::time::Duration::from_millis(1));
                                                }
                                                trace!("Write the data");
                                                write_data_to_device(&mut buffer, &mut sample_queue_clone.lock().unwrap());
                                                samples_needed_clone.store(0, Ordering::Relaxed);
                                            },
                                            _ => (),
                                        };
                                    });
                                });
                                loop {
                                    match channel.recv() {
                                        Ok(AudioMessage::Audio(chunk)) => {
                                            trace!("Received chunk");
                                            //while sample_queue.lock().unwrap().len() < samples_needed.load(Ordering::Relaxed) {
                                            //    std::thread::sleep(std::time::Duration::from_millis(1));
                                            //}
                                            trace!("Convert chunk");
                                            chunk_to_queue_float(
                                                chunk,
                                                &mut sample_queue.lock().unwrap(),
                                            );
                                        }
                                        Ok(AudioMessage::EndOfStream) => {
                                            status_channel.send(StatusMessage::PlaybackDone).unwrap();
                                            break;
                                        }
                                        Err(_) => {}
                                    }
                                }
                            },
                            _ => panic!("Unsupported sample format!"),
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

// Start a capture thread providing AudioMessages via a channel
//impl CaptureDevice for CpalCaptureDevice {
//    fn start(
//        &mut self,
//        channel: mpsc::SyncSender<AudioMessage>,
//        barrier: Arc<Barrier>,
//        status_channel: mpsc::Sender<StatusMessage>,
//        command_channel: mpsc::Receiver<CommandMessage>,
//    ) -> Res<Box<thread::JoinHandle<()>>> {
//        let devname = self.devname.clone();
//        let samplerate = self.samplerate;
//        let capture_samplerate = self.capture_samplerate;
//        let chunksize = self.chunksize;
//        let channels = self.channels;
//        let bits = match self.format {
//            SampleFormat::S16LE => 16,
//            SampleFormat::S24LE => 24,
//            SampleFormat::S24LE3 => 24,
//            SampleFormat::S32LE => 32,
//            SampleFormat::FLOAT32LE => 32,
//            SampleFormat::FLOAT64LE => 64,
//        };
//        let store_bytes = match self.format {
//            SampleFormat::S16LE => 2,
//            SampleFormat::S24LE3 => 3,
//            SampleFormat::S24LE => 4,
//            SampleFormat::S32LE => 4,
//            SampleFormat::FLOAT32LE => 4,
//            SampleFormat::FLOAT64LE => 8,
//        };
//        let buffer_bytes = 2.0f32.powf(
//            (capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
//                .log2()
//                .ceil(),
//        ) as usize
//            * 2
//            * channels
//            * store_bytes;
//        let format = self.format.clone();
//        let enable_resampling = self.enable_resampling;
//        let resampler_conf = self.resampler_conf.clone();
//        let async_src = resampler_is_async(&resampler_conf);
//        let mut silence: PrcFmt = 10.0;
//        silence = silence.powf(self.silence_threshold / 20.0);
//        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
//        let handle = thread::Builder::new()
//            .name("PulseCapture".to_string())
//            .spawn(move || {
//                let mut resampler = if enable_resampling {
//                    debug!("Creating resampler");
//                    get_resampler(
//                        &resampler_conf,
//                        channels,
//                        samplerate,
//                        capture_samplerate,
//                        chunksize,
//                    )
//                } else {
//                    None
//                };
//                match open_pulse(
//                    devname,
//                    capture_samplerate as u32,
//                    channels as u8,
//                    &format,
//                    true,
//                ) {
//                    Ok(pulsedevice) => {
//                        match status_channel.send(StatusMessage::CaptureReady) {
//                            Ok(()) => {}
//                            Err(_err) => {}
//                        }
//                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
//                        let mut silent_nbr: usize = 0;
//                        barrier.wait();
//                        debug!("starting captureloop");
//                        let mut buf = vec![0u8; buffer_bytes];
//                        let chunksize_bytes = channels * chunksize * store_bytes;
//                        let mut capture_bytes = chunksize_bytes;
//                        loop {
//                            match command_channel.try_recv() {
//                                Ok(CommandMessage::Exit) => {
//                                    let msg = AudioMessage::EndOfStream;
//                                    channel.send(msg).unwrap();
//                                    status_channel.send(StatusMessage::CaptureDone).unwrap();
//                                    break;
//                                }
//                                Ok(CommandMessage::SetSpeed { speed }) => {
//                                    if let Some(resampl) = &mut resampler {
//                                        if async_src {
//                                            if resampl.set_resample_ratio_relative(speed).is_err() {
//                                                debug!("Failed to set resampling speed to {}", speed);
//                                            }
//                                        }
//                                        else {
//                                            warn!("Requested rate adjust of synchronous resampler. Ignoring request.");
//                                        }
//                                    }
//                                }
//                                Err(_) => {}
//                            };
//                            capture_bytes = get_nbr_capture_bytes(
//                                &resampler,
//                                capture_bytes,
//                                channels,
//                                store_bytes,
//                            );
//                            if capture_bytes > buf.len() {
//                                debug!("Capture buffer too small, extending");
//                                buf.append(&mut vec![0u8; capture_bytes - buf.len()]);
//                            }
//                            let read_res = pulsedevice.read(&mut buf[0..capture_bytes]);
//                            match read_res {
//                                Ok(()) => {}
//                                Err(msg) => {
//                                    status_channel
//                                        .send(StatusMessage::CaptureError {
//                                            message: format!("{}", msg),
//                                        })
//                                        .unwrap();
//                                }
//                            };
//                            //let before = Instant::now();
//                            let mut chunk = match format {
//                                SampleFormat::S16LE | SampleFormat::S24LE | SampleFormat::S32LE => {
//                                    buffer_to_chunk_bytes(
//                                        &buf[0..capture_bytes],
//                                        channels,
//                                        scalefactor,
//                                        store_bytes,
//                                        capture_bytes,
//                                    )
//                                }
//                                SampleFormat::FLOAT32LE => buffer_to_chunk_float_bytes(
//                                    &buf[0..capture_bytes],
//                                    channels,
//                                    bits,
//                                    capture_bytes,
//                                ),
//                                _ => panic!("Unsupported sample format"),
//                            };
//                            if (chunk.maxval - chunk.minval) > silence {
//                                if silent_nbr > silent_limit {
//                                    debug!("Resuming processing");
//                                }
//                                silent_nbr = 0;
//                            } else if silent_limit > 0 {
//                                if silent_nbr == silent_limit {
//                                    debug!("Pausing processing");
//                                }
//                                silent_nbr += 1;
//                            }
//                            if silent_nbr <= silent_limit {
//                                if let Some(resampl) = &mut resampler {
//                                    let new_waves = resampl.process(&chunk.waveforms).unwrap();
//                                    chunk.frames = new_waves[0].len();
//                                    chunk.valid_frames = new_waves[0].len();
//                                    chunk.waveforms = new_waves;
//                                }
//                                let msg = AudioMessage::Audio(chunk);
//                                channel.send(msg).unwrap();
//                            }
//                        }
//                    }
//                    Err(err) => {
//                        status_channel
//                            .send(StatusMessage::CaptureError {
//                                message: format!("{}", err),
//                            })
//                            .unwrap();
//                    }
//                }
//            })
//            .unwrap();
//        Ok(Box::new(handle))
//    }
//}
//