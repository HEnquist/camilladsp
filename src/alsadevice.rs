extern crate alsa;
extern crate num_traits;
//use std::{iter, error};
//use std::any::{Any, TypeId};
use alsa::ctl::{ElemId, ElemIface};
use alsa::ctl::{ElemType, ElemValue};
use alsa::hctl::HCtl;
use alsa::pcm::{Access, Format, HwParams, State};
use alsa::{Direction, ValueOr};
use rubato::Resampler;
use std::ffi::CString;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime};
//mod audiodevice;
use audiodevice::*;
// Sample format
use config;
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_float, buffer_to_chunk_int, chunk_to_buffer_float, chunk_to_buffer_int,
};

use CommandMessage;
use PrcFmt;
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
    pub format: SampleFormat,
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
    pub format: SampleFormat,
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
}

struct PlaybackParams {
    scalefactor: PrcFmt,
    target_level: usize,
    adjust_period: f32,
    adjust_enabled: bool,
}

/// Play a buffer.
fn play_buffer<T: std::marker::Copy>(
    buffer: &[T],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<T>,
) -> Res<()> {
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
        Err(_err) => {
            warn!("Retrying playback");
            pcmdevice.prepare()?;
            let delay = Duration::from_millis(5);
            thread::sleep(delay);
            io.writei(&buffer[..])?
        }
    };
    Ok(())
}

/// Play a buffer.
fn capture_buffer<T: std::marker::Copy>(
    buffer: &mut [T],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<T>,
) -> Res<()> {
    let capture_state = pcmdevice.state();
    if capture_state == State::XRun {
        warn!("prepare capture");
        pcmdevice.prepare()?;
    }
    let _frames = match io.readi(buffer) {
        Ok(frames) => frames,
        Err(_err) => {
            warn!("retrying capture");
            pcmdevice.prepare()?;
            io.readi(buffer)?
        }
    };
    Ok(())
}

/// Open an Alsa PCM device
fn open_pcm(
    devname: String,
    samplerate: u32,
    bufsize: MachInt,
    channels: u32,
    format: &SampleFormat,
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
        match format {
            SampleFormat::S16LE => hwp.set_format(Format::s16())?,
            SampleFormat::S24LE => hwp.set_format(Format::s24())?,
            SampleFormat::S32LE => hwp.set_format(Format::s32())?,
            SampleFormat::FLOAT32LE => hwp.set_format(Format::float())?,
            SampleFormat::FLOAT64LE => hwp.set_format(Format::float64())?,
        }

        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_buffer_size(2 * bufsize)?;
        hwp.set_period_size(bufsize / 4, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let (_rate, _act_bufsize) = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        swp.set_start_threshold(act_bufsize / 2 - act_periodsize)?;
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

fn playback_loop_int<T: num_traits::NumCast + std::marker::Copy>(
    channels: PlaybackChannels,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
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
                chunk_to_buffer_int(chunk, &mut buffer, params.scalefactor);
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
                    let rel_diff = (diff as f32) / (srate as f32);
                    speed = 1.0 + 0.5 * rel_diff / params.adjust_period;
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

fn playback_loop_float<T: num_traits::NumCast + std::marker::Copy>(
    channels: PlaybackChannels,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
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
                chunk_to_buffer_float(chunk, &mut buffer);
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
                    let rel_diff = (diff as f32) / (srate as f32);
                    speed = 1.0 + 0.5 * rel_diff / params.adjust_period;
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

                let playback_res = play_buffer(&buffer, &pcmdevice, &io);
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

fn capture_loop_int<
    T: num_traits::NumCast + std::marker::Copy + num_traits::AsPrimitive<PrcFmt>,
>(
    channels: CaptureChannels,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
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
    }
    let mut capture_samples = params.chunksize * params.channels;
    loop {
        match channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                let msg = AudioMessage::EndOfStream;
                channels.audio.send(msg).unwrap();
                channels.status.send(StatusMessage::CaptureDone).unwrap();
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                if let Some(elem) = &element {
                    elval.set_integer(0, (100_000.0 * speed) as i32).unwrap();
                    elem.write(&elval).unwrap();
                } else if let Some(resampl) = &mut resampler {
                    resampl.set_resample_ratio_relative(speed).unwrap();
                }
            }
            Err(_) => {}
        };
        if let Some(resampl) = &mut resampler {
            capture_samples = resampl.nbr_frames_needed() * params.channels;
            trace!("Resamper needs {} frames", resampl.nbr_frames_needed());
        }
        let capture_res = capture_buffer(&mut buffer[0..capture_samples], pcmdevice, &io);
        match capture_res {
            Ok(_) => {
                trace!("Captured {} samples", capture_samples);
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
        let mut chunk = buffer_to_chunk_int(
            &buffer[0..capture_samples],
            params.channels,
            params.scalefactor,
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
                chunk.valid_frames = new_waves[0].len();
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            channels.audio.send(msg).unwrap();
        }
    }
}

fn capture_loop_float<
    T: num_traits::NumCast + std::marker::Copy + num_traits::AsPrimitive<PrcFmt>,
>(
    channels: CaptureChannels,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
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
        debug!("Capure device supports rate adjust");
    }
    let mut capture_samples = params.chunksize * params.channels;
    loop {
        match channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                let msg = AudioMessage::EndOfStream;
                channels.audio.send(msg).unwrap();
                channels.status.send(StatusMessage::CaptureDone).unwrap();
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                if let Some(elem) = &element {
                    elval.set_integer(0, (100_000.0 * speed) as i32).unwrap();
                    elem.write(&elval).unwrap();
                } else if let Some(resampl) = &mut resampler {
                    resampl.set_resample_ratio_relative(speed).unwrap();
                }
            }
            Err(_) => {}
        };
        if let Some(resampl) = &mut resampler {
            capture_samples = resampl.nbr_frames_needed() * params.channels;
            trace!("Resamper needs {} frames", resampl.nbr_frames_needed());
        }
        let capture_res = capture_buffer(&mut buffer[0..capture_samples], pcmdevice, &io);
        match capture_res {
            Ok(_) => {
                trace!("Captured {} samples", capture_samples);
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
        let mut chunk = buffer_to_chunk_float(&buffer[0..capture_samples], params.channels);
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
                chunk.valid_frames = new_waves[0].len();
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            channels.audio.send(msg).unwrap();
        }
    }
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
        let bits: i32 = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let format = self.format.clone();
        let handle = thread::Builder::new()
            .name("AlsaPlayback".to_string())
            .spawn(move || {
                //let delay = time::Duration::from_millis((4*1000*chunksize/samplerate) as u64);
                match open_pcm(
                    devname,
                    samplerate as u32,
                    chunksize as MachInt,
                    channels as u32,
                    &format,
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
                        };
                        let pb_channels = PlaybackChannels {
                            audio: channel,
                            status: status_channel,
                        };
                        match format {
                            SampleFormat::S16LE => {
                                let io = pcmdevice.io_i16().unwrap();
                                let buffer = vec![0i16; chunksize * channels];
                                playback_loop_int(pb_channels, buffer, &pcmdevice, io, pb_params);
                            }
                            SampleFormat::S24LE | SampleFormat::S32LE => {
                                let io = pcmdevice.io_i32().unwrap();
                                let buffer = vec![0i32; chunksize * channels];
                                playback_loop_int(pb_channels, buffer, &pcmdevice, io, pb_params);
                            }
                            SampleFormat::FLOAT32LE => {
                                let io = pcmdevice.io_f32().unwrap();
                                let buffer = vec![0f32; chunksize * channels];

                                playback_loop_float(pb_channels, buffer, &pcmdevice, io, pb_params);
                            }
                            SampleFormat::FLOAT64LE => {
                                let io = pcmdevice.io_f64().unwrap();
                                let buffer = vec![0f64; chunksize * channels];
                                playback_loop_float(pb_channels, buffer, &pcmdevice, io, pb_params);
                            }
                        };
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

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
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
        let buffer_frames = 2.0f32.powf(
            (capture_samplerate as f32 / samplerate as f32 * chunksize as f32)
                .log2()
                .ceil(),
        ) as usize
            * 2;
        println!("Buffer frames {}", buffer_frames);
        let channels = self.channels;
        let bits: i32 = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit = (self.silence_timeout * ((samplerate / chunksize) as PrcFmt)) as usize;
        let format = self.format.clone();
        let enable_resampling = self.enable_resampling;
        let resampler_conf = self.resampler_conf.clone();
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
                    &format,
                    true,
                ) {
                    Ok(pcmdevice) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
                        barrier.wait();
                        debug!("Starting captureloop");
                        let cap_params = CaptureParams {
                            channels,
                            scalefactor,
                            silent_limit,
                            silence,
                            chunksize,
                        };
                        let cap_channels = CaptureChannels {
                            audio: channel,
                            status: status_channel,
                            command: command_channel,
                        };
                        match format {
                            SampleFormat::S16LE => {
                                let io = pcmdevice.io_i16().unwrap();
                                let buffer = vec![0i16; channels * buffer_frames];
                                capture_loop_int(
                                    cap_channels,
                                    buffer,
                                    &pcmdevice,
                                    io,
                                    cap_params,
                                    resampler,
                                );
                            }
                            SampleFormat::S24LE | SampleFormat::S32LE => {
                                let io = pcmdevice.io_i32().unwrap();
                                let buffer = vec![0i32; channels * buffer_frames];
                                capture_loop_int(
                                    cap_channels,
                                    buffer,
                                    &pcmdevice,
                                    io,
                                    cap_params,
                                    resampler,
                                );
                            }
                            SampleFormat::FLOAT32LE => {
                                let io = pcmdevice.io_f32().unwrap();
                                let buffer = vec![0f32; channels * buffer_frames];
                                capture_loop_float(
                                    cap_channels,
                                    buffer,
                                    &pcmdevice,
                                    io,
                                    cap_params,
                                    resampler,
                                );
                            }
                            SampleFormat::FLOAT64LE => {
                                let io = pcmdevice.io_f64().unwrap();
                                let buffer = vec![0f64; channels * buffer_frames];
                                capture_loop_float(
                                    cap_channels,
                                    buffer,
                                    &pcmdevice,
                                    io,
                                    cap_params,
                                    resampler,
                                );
                            }
                        };
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
