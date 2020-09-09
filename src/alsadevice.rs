extern crate alsa;
extern crate nix;
use alsa::ctl::{ElemId, ElemIface};
use alsa::ctl::{ElemType, ElemValue};
use alsa::hctl::HCtl;
use alsa::pcm::{Access, Format, HwParams, State};
use alsa::{Direction, ValueOr};
use audiodevice::*;
use config;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
    chunk_to_buffer_float_bytes,
};
use nix::errno::Errno;
use rubato::Resampler;
use std::ffi::CString;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::CaptureStatus;
use CommandMessage;
use PrcFmt;
use ProcessingState;
use Res;
use StatusMessage;

#[cfg(target_pointer_width = "64")]
pub type MachInt = i64;
#[cfg(not(target_pointer_width = "64"))]
pub type MachInt = i32;

pub struct AlsaPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

pub struct AlsaCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    //pub resampler: Option<Box<dyn Resampler<PrcFmt>>>,
    pub enable_resampling: bool,
    pub capture_samplerate: usize,
    pub resampler_conf: config::Resampler,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
}

struct CaptureChannels {
    audio: mpsc::SyncSender<AudioMessage>,
    status: mpsc::Sender<StatusMessage>,
    command: mpsc::Receiver<CommandMessage>,
}

struct PlaybackChannels {
    audio: mpsc::Receiver<AudioMessage>,
    status: mpsc::Sender<StatusMessage>,
}

struct CaptureParams {
    channels: usize,
    scalefactor: PrcFmt,
    silent_limit: usize,
    silence: PrcFmt,
    chunksize: usize,
    bits_per_sample: i32,
    store_bytes_per_sample: usize,
    floats: bool,
    samplerate: usize,
    capture_samplerate: usize,
    async_src: bool,
    capture_status: Arc<RwLock<CaptureStatus>>,
}

struct PlaybackParams {
    scalefactor: PrcFmt,
    target_level: usize,
    adjust_period: f32,
    adjust_enabled: bool,
    bits: i32,
    bytes_per_sample: usize,
    floats: bool,
}

enum CaptureResult {
    Normal,
    Timeout,
}

/// Play a buffer.
fn play_buffer(buffer: &[u8], pcmdevice: &alsa::PCM, io: &alsa::pcm::IO<u8>) -> Res<()> {
    let playback_state = pcmdevice.state();
    trace!("playback state {:?}", playback_state);
    if playback_state == State::XRun {
        warn!("Prepare playback");
        pcmdevice.prepare()?;
        let delay = Duration::from_millis(5);
        thread::sleep(delay);
    }
    let _frames = match io.writei(&buffer[..]) {
        Ok(frames) => frames,
        Err(err) => {
            warn!("Retrying playback, error: {}", err);
            pcmdevice.prepare()?;
            let delay = Duration::from_millis(5);
            thread::sleep(delay);
            io.writei(&buffer[..])?
        }
    };
    Ok(())
}

/// Play a buffer.
fn capture_buffer(
    buffer: &mut [u8],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
) -> Res<CaptureResult> {
    let capture_state = pcmdevice.state();
    if capture_state == State::XRun {
        warn!("prepare capture");
        pcmdevice.prepare()?;
    }
    let _frames = match io.readi(buffer) {
        Ok(frames) => frames,
        Err(err) => match err.nix_error() {
            nix::Error::Sys(Errno::EIO) => {
                warn!("Capture timed out, error: {}", err);
                return Ok(CaptureResult::Timeout);
            }
            nix::Error::Sys(Errno::EPIPE) => {
                warn!("Retrying capture, error: {}", err);
                pcmdevice.prepare()?;
                io.readi(buffer)?
            }
            _ => {
                warn!("Capture failed, error: {}", err);
                return Err(Box::new(err));
            }
        },
    };
    Ok(CaptureResult::Normal)
}

/// Open an Alsa PCM device
fn open_pcm(
    devname: String,
    samplerate: u32,
    bufsize: MachInt,
    channels: u32,
    sample_format: &SampleFormat,
    capture: bool,
) -> Res<alsa::PCM> {
    // Open the device
    let pcmdev;
    if capture {
        pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;
    } else {
        pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;
    }
    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;
        hwp.set_channels(channels)?;
        hwp.set_rate(samplerate, ValueOr::Nearest)?;
        match sample_format {
            SampleFormat::S16LE => hwp.set_format(Format::s16())?,
            SampleFormat::S24LE => hwp.set_format(Format::s24())?,
            SampleFormat::S24LE3 => hwp.set_format(Format::S243LE)?,
            SampleFormat::S32LE => hwp.set_format(Format::s32())?,
            SampleFormat::FLOAT32LE => hwp.set_format(Format::float())?,
            SampleFormat::FLOAT64LE => hwp.set_format(Format::float64())?,
        }

        hwp.set_access(Access::RWInterleaved)?;
        let _bufsize = hwp.set_buffer_size_near(2 * bufsize)?;
        let _period = hwp.set_period_size_near(bufsize / 4, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let (_rate, _act_bufsize) = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        if capture {
            swp.set_start_threshold(0)?;
        } else {
            swp.set_start_threshold(act_bufsize / 2 - act_periodsize)?;
        }
        //swp.set_avail_min(periodsize)?;
        pcmdev.sw_params(&swp)?;
        debug!(
            "Opened audio device {:?} with parameters: {:?}, {:?}",
            devname, hwp, swp
        );
        (hwp.get_rate()?, act_bufsize)
    };
    Ok(pcmdev)
}

fn playback_loop_bytes(
    channels: PlaybackChannels,
    mut buffer: Vec<u8>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<u8>,
    params: PlaybackParams,
) {
    let srate = pcmdevice.hw_params_current().unwrap().get_rate().unwrap();
    let mut start = SystemTime::now();
    let mut now;
    let mut delay = 0;
    let mut ndelays = 0;
    let mut speed;
    let mut diff: isize;
    let adjust = params.adjust_period > 0.0 && params.adjust_enabled;
    loop {
        match channels.audio.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                if params.floats {
                    chunk_to_buffer_float_bytes(chunk, &mut buffer, params.bits);
                } else {
                    chunk_to_buffer_bytes(
                        chunk,
                        &mut buffer,
                        params.scalefactor,
                        params.bits as i32,
                        params.bytes_per_sample,
                    );
                }
                now = SystemTime::now();
                if let Ok(status) = pcmdevice.status() {
                    delay += status.get_delay() as isize;
                    ndelays += 1;
                }
                if adjust
                    && (now.duration_since(start).unwrap().as_millis()
                        > ((1000.0 * params.adjust_period) as u128))
                {
                    let av_delay = delay / ndelays;
                    diff = av_delay - params.target_level as isize;
                    let rel_diff = (diff as f64) / (srate as f64);
                    speed = 1.0 - 0.5 * rel_diff / params.adjust_period as f64;
                    debug!(
                        "Current buffer level {}, set capture rate to {}%",
                        av_delay,
                        100.0 * speed
                    );
                    start = now;
                    delay = 0;
                    ndelays = 0;
                    channels
                        .status
                        .send(StatusMessage::SetSpeed { speed })
                        .unwrap();
                }

                let playback_res = play_buffer(&buffer, pcmdevice, &io);
                match playback_res {
                    Ok(_) => {}
                    Err(msg) => {
                        channels
                            .status
                            .send(StatusMessage::PlaybackError {
                                message: format!("{}", msg),
                            })
                            .unwrap();
                    }
                };
            }
            Ok(AudioMessage::EndOfStream) => {
                channels.status.send(StatusMessage::PlaybackDone).unwrap();
                break;
            }
            _ => {}
        }
    }
}

fn capture_loop_bytes(
    channels: CaptureChannels,
    mut buffer: Vec<u8>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<u8>,
    params: CaptureParams,
    mut resampler: Option<Box<dyn Resampler<PrcFmt>>>,
) {
    let mut silent_nbr: usize = 0;
    let pcminfo = pcmdevice.info().unwrap();
    let card = pcminfo.get_card();
    let device = pcminfo.get_device();
    let subdevice = pcminfo.get_subdevice();
    let mut elid = ElemId::new(ElemIface::PCM);
    elid.set_device(device);
    elid.set_subdevice(subdevice);
    elid.set_name(&CString::new("PCM Rate Shift 100000").unwrap());
    let h = HCtl::new(&format!("hw:{}", card), false).unwrap();
    h.load().unwrap();
    let element = h.find_elem(&elid);
    let mut elval = ElemValue::new(ElemType::Integer).unwrap();
    if element.is_some() {
        info!("Capture device supports rate adjust");
        if params.samplerate == params.capture_samplerate && resampler.is_some() {
            warn!("Needless 1:1 sample rate conversion active. Not needed since capture device supports rate adjust");
        } else if params.async_src && resampler.is_some() {
            warn!("Async resampler not needed since capture device supports rate adjust. Switch to Sync type to save CPU time.");
        }
    }
    let mut capture_bytes = params.chunksize * params.channels * params.store_bytes_per_sample;
    let mut start = SystemTime::now();
    let mut now;
    let mut bytes_counter = 0;
    let mut value_range = 0.0;
    let mut rate_adjust = 0.0;
    let mut state = ProcessingState::Running;
    let mut card_inactive = false;
    loop {
        match channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                let msg = AudioMessage::EndOfStream;
                channels.audio.send(msg).unwrap();
                channels.status.send(StatusMessage::CaptureDone).unwrap();
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                rate_adjust = speed;
                if let Some(elem) = &element {
                    elval.set_integer(0, (100_000.0 / speed) as i32).unwrap();
                    elem.write(&elval).unwrap();
                } else if let Some(resampl) = &mut resampler {
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
        capture_bytes = get_nbr_capture_bytes(capture_bytes, &resampler, &params, &mut buffer);
        let capture_res = capture_buffer(&mut buffer[0..capture_bytes], pcmdevice, &io);
        match capture_res {
            Ok(CaptureResult::Normal) => {
                trace!("Captured {} bytes", capture_bytes);
                now = SystemTime::now();
                card_inactive = false;
                bytes_counter += capture_bytes;
                if now.duration_since(start).unwrap().as_millis() as usize
                    > params.capture_status.read().unwrap().update_interval
                {
                    let meas_time = now.duration_since(start).unwrap().as_secs_f32();
                    let bytes_per_sec = bytes_counter as f32 / meas_time;
                    let measured_rate_f =
                        bytes_per_sec / (params.channels * params.store_bytes_per_sample) as f32;
                    trace!("Measured sample rate is {} Hz", measured_rate_f);
                    let mut capt_stat = params.capture_status.write().unwrap();
                    capt_stat.measured_samplerate = measured_rate_f as usize;
                    capt_stat.signal_range = value_range as f32;
                    capt_stat.rate_adjust = rate_adjust as f32;
                    capt_stat.state = state;
                    start = now;
                    bytes_counter = 0;
                }
            }
            Ok(CaptureResult::Timeout) => {
                card_inactive = true;
            }
            Err(msg) => {
                channels
                    .status
                    .send(StatusMessage::CaptureError {
                        message: format!("{}", msg),
                    })
                    .unwrap();
            }
        };
        let mut chunk = if params.floats {
            buffer_to_chunk_float_bytes(
                &buffer[0..capture_bytes],
                params.channels,
                params.bits_per_sample,
                capture_bytes,
            )
        } else {
            buffer_to_chunk_bytes(
                &buffer[0..capture_bytes],
                params.channels,
                params.scalefactor,
                params.store_bytes_per_sample,
                capture_bytes,
            )
        };
        value_range = chunk.maxval - chunk.minval;
        if card_inactive {
            state = ProcessingState::Paused;
            debug!("Source inactive, pausing");
        } else if value_range > params.silence {
            if silent_nbr > params.silent_limit {
                debug!("Resuming processing");
            }
            state = ProcessingState::Running;
            silent_nbr = 0;
        } else if params.silent_limit > 0 {
            if silent_nbr == params.silent_limit {
                state = ProcessingState::Paused;
                debug!("Pausing processing");
            }
            silent_nbr += 1;
        }
        if state == ProcessingState::Running {
            if let Some(resampl) = &mut resampler {
                let new_waves = resampl.process(&chunk.waveforms).unwrap();
                chunk.frames = new_waves[0].len();
                chunk.valid_frames = new_waves[0].len();
                chunk.waveforms = new_waves;
            }
            trace!("sending chunk");
            let msg = AudioMessage::Audio(chunk);
            channels.audio.send(msg).unwrap();
        }
    }
    let mut capt_stat = params.capture_status.write().unwrap();
    capt_stat.state = ProcessingState::Inactive;
}

fn get_nbr_capture_bytes(
    capture_bytes: usize,
    resampler: &Option<Box<dyn Resampler<PrcFmt>>>,
    params: &CaptureParams,
    buf: &mut Vec<u8>,
) -> usize {
    let capture_bytes_new = if let Some(resampl) = &resampler {
        trace!("Resampler needs {} frames", resampl.nbr_frames_needed());
        resampl.nbr_frames_needed() * params.channels * params.store_bytes_per_sample
    } else {
        capture_bytes
    };
    if capture_bytes > buf.len() {
        debug!("Capture buffer too small, extending");
        buf.append(&mut vec![0u8; capture_bytes_new - buf.len()]);
    }
    capture_bytes_new
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let target_level = if self.target_level > 0 {
            self.target_level
        } else {
            self.chunksize
        };
        let adjust_period = self.adjust_period;
        let adjust_enabled = self.enable_rate_adjust;
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let bits = self.sample_format.bits_per_sample() as i32;
        let bytes_per_sample = self.sample_format.bytes_per_sample();
        let floats = self.sample_format.is_float();
        let sample_format = self.sample_format.clone();
        let handle = thread::Builder::new()
            .name("AlsaPlayback".to_string())
            .spawn(move || {
                //let delay = time::Duration::from_millis((4*1000*chunksize/samplerate) as u64);
                match open_pcm(
                    devname,
                    samplerate as u32,
                    chunksize as MachInt,
                    channels as u32,
                    &sample_format,
                    false,
                ) {
                    Ok(pcmdevice) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        //let scalefactor = (1<<bits-1) as PrcFmt;
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);

                        barrier.wait();
                        //thread::sleep(delay);
                        debug!("Starting playback loop");
                        let pb_params = PlaybackParams {
                            scalefactor,
                            target_level,
                            adjust_period,
                            adjust_enabled,
                            bits,
                            bytes_per_sample,
                            floats,
                        };
                        let pb_channels = PlaybackChannels {
                            audio: channel,
                            status: status_channel,
                        };

                        let io = pcmdevice.io();
                        let buffer = vec![0u8; chunksize * channels * bytes_per_sample];
                        playback_loop_bytes(pb_channels, buffer, &pcmdevice, io, pb_params);
                    }
                    Err(err) => {
                        let send_result = status_channel.send(StatusMessage::PlaybackError {
                            message: format!("{}", err),
                        });
                        if send_result.is_err() {
                            error!("Playback error: {}", err);
                        }
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
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
        let buffer_frames = 2.0f32.powf(
            (1.2 * capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
                .log2()
                .ceil(),
        ) as usize;
        println!("Buffer frames {}", buffer_frames);
        let channels = self.channels;
        let bits_per_sample = self.sample_format.bits_per_sample() as i32;
        let store_bytes_per_sample = self.sample_format.bytes_per_sample();
        let floats = self.sample_format.is_float();
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
        let sample_format = self.sample_format.clone();
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
        let async_src = resampler_is_async(&resampler_conf);
        let handle = thread::Builder::new()
            .name("AlsaCapture".to_string())
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
                match open_pcm(
                    devname,
                    capture_samplerate as u32,
                    buffer_frames as MachInt,
                    channels as u32,
                    &sample_format,
                    true,
                ) {
                    Ok(pcmdevice) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits_per_sample - 1);
                        barrier.wait();
                        debug!("Starting captureloop");
                        let cap_params = CaptureParams {
                            channels,
                            scalefactor,
                            silent_limit,
                            silence,
                            chunksize,
                            bits_per_sample,
                            store_bytes_per_sample,
                            floats,
                            samplerate,
                            capture_samplerate,
                            async_src,
                            capture_status,
                        };
                        let cap_channels = CaptureChannels {
                            audio: channel,
                            status: status_channel,
                            command: command_channel,
                        };
                        let io = pcmdevice.io();
                        let buffer = vec![0u8; channels * buffer_frames * store_bytes_per_sample];
                        capture_loop_bytes(
                            cap_channels,
                            buffer,
                            &pcmdevice,
                            io,
                            cap_params,
                            resampler,
                        );
                    }
                    Err(err) => {
                        let send_result = status_channel.send(StatusMessage::CaptureError {
                            message: format!("{}", err),
                        });
                        if send_result.is_err() {
                            error!("Capture error: {}", err);
                        }
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
