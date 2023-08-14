extern crate alsa;
extern crate nix;
use crate::audiodevice::*;
use crate::config;
use crate::config::SampleFormat;
use crate::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::countertimer;
use alsa::ctl::{ElemId, ElemIface};
use alsa::ctl::{ElemType, ElemValue};
use alsa::hctl::{Elem, HCtl};
use alsa::pcm::{Access, Format, Frames, HwParams};
use alsa::{Direction, ValueOr, PCM};
use alsa_sys;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use rubato::VecResampler;
use std::ffi::CString;
use std::fmt::Debug;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use crate::alsadevice_buffermanager::{
    CaptureBufferManager, DeviceBufferManager, PlaybackBufferManager,
};
use crate::alsadevice_utils::{
    adjust_speed, list_channels_as_text, list_device_names, list_formats_as_text,
    list_samplerates_as_text, state_desc,
};
use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

lazy_static! {
    static ref ALSA_MUTEX: Mutex<()> = Mutex::new(());
}

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
    pub capture_samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
}

struct CaptureChannels {
    audio: mpsc::SyncSender<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
    command: mpsc::Receiver<CommandMessage>,
}

struct PlaybackChannels {
    audio: mpsc::Receiver<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
}

struct CaptureParams {
    channels: usize,
    sample_format: SampleFormat,
    silence_timeout: PrcFmt,
    silence_threshold: PrcFmt,
    chunksize: usize,
    store_bytes_per_sample: usize,
    bytes_per_frame: usize,
    samplerate: usize,
    capture_samplerate: usize,
    async_src: bool,
    capture_status: Arc<RwLock<CaptureStatus>>,
    stop_on_rate_change: bool,
    rate_measure_interval: f32,
}

struct PlaybackParams {
    channels: usize,
    target_level: usize,
    adjust_period: f32,
    adjust_enabled: bool,
    sample_format: SampleFormat,
    playback_status: Arc<RwLock<PlaybackStatus>>,
    bytes_per_frame: usize,
    samplerate: usize,
    chunksize: usize,
}

enum CaptureResult {
    Normal,
    Stalled,
}

#[derive(Debug)]
enum PlaybackResult {
    Normal,
    Stalled,
}

/// Play a buffer.
fn play_buffer(
    mut buffer: &[u8],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
    millis_per_frame: f32,
    bytes_per_frame: usize,
    buf_manager: &mut PlaybackBufferManager,
) -> Res<PlaybackResult> {
    let playback_state = pcmdevice.state_raw();
    //trace!("Playback state {:?}", playback_state);
    if playback_state < 0 {
        // This should never happen but sometimes does anyway,
        // for example if a USB device is unplugged.
        let nixerr = alsa::nix::errno::from_i32(-playback_state);
        error!(
            "PB: Alsa snd_pcm_state() of playback device returned an unexpected error: {}",
            nixerr
        );
        return Err(Box::new(nixerr));
    } else if playback_state == alsa_sys::SND_PCM_STATE_XRUN as i32 {
        warn!("PB: Prepare playback after buffer underrun");
        pcmdevice.prepare()?;
        buf_manager.sleep_for_target_delay(millis_per_frame);
    } else if playback_state == alsa_sys::SND_PCM_STATE_PREPARED as i32 {
        info!("PB: Starting playback from Prepared state");
        buf_manager.sleep_for_target_delay(millis_per_frame);
    } else if playback_state != alsa_sys::SND_PCM_STATE_RUNNING as i32 {
        warn!(
            "PB: device is in an unexpected state: {}",
            state_desc(playback_state as u32)
        );
    }

    let frames_to_write = buffer.len() / bytes_per_frame;
    let mut retry_count: usize = 0;
    loop {
        retry_count += 1;
        if retry_count >= 100 {
            warn!("PB: giving up after {} write attempts", retry_count);
            return Err(DeviceError::new("Aborting playback after too many write attempts").into());
        }
        let timeout_millis = (2.0 * millis_per_frame * frames_to_write as f32) as u32;
        trace!(
            "PB: write try {}, pcmdevice.wait with timeout {} ms",
            retry_count,
            timeout_millis
        );
        let start = if log_enabled!(log::Level::Trace) {
            Some(Instant::now())
        } else {
            None
        };
        match pcmdevice.wait(Some(timeout_millis)) {
            Ok(true) => {
                trace!(
                    "PB: device waited for {:?}, ready",
                    start.map(|s| s.elapsed())
                );
            }
            Ok(false) => {
                trace!("PB: Wait timed out, playback device takes too long to drain buffer");
                return Ok(PlaybackResult::Stalled);
            }
            Err(err) => {
                warn!(
                    "PB: device failed while waiting for available buffer space, error: {}",
                    err
                );
                return Err(Box::new(err));
            }
        }

        //trace!("Delay BEFORE writing {} is {:?} frames",  buffer.len() / bytes_per_frame, pcmdevice.status().ok().map(|status| status.get_delay()));
        match io.writei(buffer) {
            Ok(frames_written) => {
                let cur_frames_to_write = buffer.len() / bytes_per_frame;
                //trace!("Delay AFTER writing {} is {:?} frames", frames_written, pcmdevice.status().ok().map(|status| status.get_delay()));
                if frames_written == cur_frames_to_write {
                    trace!(
                        "PB: wrote {} frames to playback device as requested",
                        frames_written
                    );
                    break;
                } else {
                    trace!(
                        "PB: wrote {} instead of requested {}, trying again to write the rest",
                        frames_written,
                        cur_frames_to_write
                    );
                    buffer = &buffer[frames_written * bytes_per_frame..];
                    // repeat writing
                    continue;
                }
            }
            Err(err) => {
                if err.nix_error() == alsa::nix::errno::Errno::EAGAIN {
                    trace!("PB: encountered EAGAIN error on write, trying again");
                } else {
                    warn!("PB: write error, trying to recover. Error: {}", err);
                    trace!("snd_pcm_prepare");
                    // Would recover() be better than prepare()?
                    pcmdevice.prepare()?;
                    buf_manager.sleep_for_target_delay(millis_per_frame);
                    io.writei(buffer)?;
                    break;
                }
            }
        };
    }
    Ok(PlaybackResult::Normal)
}

/// Capture a buffer.
fn capture_buffer(
    mut buffer: &mut [u8],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
    samplerate: usize,
    frames_to_read: usize,
    bytes_per_frame: usize,
) -> Res<CaptureResult> {
    let capture_state = pcmdevice.state_raw();
    if capture_state == alsa_sys::SND_PCM_STATE_XRUN as i32 {
        warn!("Prepare capture device");
        pcmdevice.prepare()?;
    } else if capture_state < 0 {
        // This should never happen but sometimes does anyway,
        // for example if a USB device is unplugged.
        let nixerr = alsa::nix::errno::from_i32(-capture_state);
        error!(
            "Alsa snd_pcm_state() of capture device returned an unexpected error: {}",
            capture_state
        );
        return Err(Box::new(nixerr));
    } else if capture_state != alsa_sys::SND_PCM_STATE_RUNNING as i32 {
        debug!(
            "Starting capture from state: {}",
            state_desc(capture_state as u32)
        );
        pcmdevice.start()?;
    }
    let millis_per_chunk = 1000 * frames_to_read / samplerate;

    loop {
        let mut timeout_millis = 4 * millis_per_chunk as u32;
        if timeout_millis < 10 {
            timeout_millis = 10;
        }
        let start = if log_enabled!(log::Level::Trace) {
            Some(Instant::now())
        } else {
            None
        };
        trace!("Capture pcmdevice.wait with timeout {} ms", timeout_millis);
        match pcmdevice.wait(Some(timeout_millis)) {
            Ok(true) => {
                trace!("Capture waited for {:?}, ready", start.map(|s| s.elapsed()));
            }
            Ok(false) => {
                trace!("Wait timed out, capture device takes too long to capture frames");
                return Ok(CaptureResult::Stalled);
            }
            Err(err) => {
                warn!(
                    "Capture device failed while waiting for available frames, error: {}",
                    err
                );
                return Err(Box::new(err));
            }
        }
        match io.readi(buffer) {
            Ok(frames_read) => {
                let frames_req = buffer.len() / bytes_per_frame;
                if frames_read == frames_req {
                    trace!("Capture read {} frames as requested", frames_read);
                    return Ok(CaptureResult::Normal);
                } else {
                    warn!(
                        "Capture read {} frames instead of the requested {}",
                        frames_read, frames_req
                    );
                    buffer = &mut buffer[frames_read * bytes_per_frame..];
                    // repeat reading
                    continue;
                }
            }
            Err(err) => match err.nix_error() {
                alsa::nix::errno::Errno::EIO => {
                    warn!("Capture failed with error: {}", err);
                    return Err(Box::new(err));
                }
                // TODO: do we need separate handling of xruns that happen in the tiny
                // window between state() and readi()?
                alsa::nix::errno::Errno::EPIPE => {
                    warn!("Capture failed, error: {}", err);
                    return Err(Box::new(err));
                }
                _ => {
                    warn!("Capture failed, error: {}", err);
                    return Err(Box::new(err));
                }
            },
        };
    }
}

/// Open an Alsa PCM device
fn open_pcm(
    devname: String,
    samplerate: u32,
    channels: u32,
    sample_format: &SampleFormat,
    buf_manager: &mut dyn DeviceBufferManager,
    capture: bool,
) -> Res<alsa::PCM> {
    let direction = if capture { "Capture" } else { "Playback" };
    debug!(
        "Available {} devices: {:?}",
        direction,
        list_device_names(capture)
    );
    // Acquire the lock
    let _lock = ALSA_MUTEX.lock();
    // Open the device
    let pcmdev = if capture {
        alsa::PCM::new(&devname, Direction::Capture, true)?
    } else {
        alsa::PCM::new(&devname, Direction::Playback, true)?
    };
    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;

        // Set number of channels
        debug!("{}: {}", direction, list_channels_as_text(&hwp));
        debug!("{}: setting channels to {}", direction, channels);
        hwp.set_channels(channels)?;

        // Set samplerate
        debug!("{}: {}", direction, list_samplerates_as_text(&hwp));
        debug!("{}: setting rate to {}", direction, samplerate);
        hwp.set_rate(samplerate, ValueOr::Nearest)?;

        // Set sample format
        debug!("{}: {}", direction, list_formats_as_text(&hwp));
        debug!("{}: setting format to {}", direction, sample_format);
        match sample_format {
            SampleFormat::S16LE => hwp.set_format(Format::s16())?,
            SampleFormat::S24LE => hwp.set_format(Format::s24())?,
            SampleFormat::S24LE3 => hwp.set_format(Format::s24_3())?,
            SampleFormat::S32LE => hwp.set_format(Format::s32())?,
            SampleFormat::FLOAT32LE => hwp.set_format(Format::float())?,
            SampleFormat::FLOAT64LE => hwp.set_format(Format::float64())?,
        }

        // Set access mode, buffersize and periods
        hwp.set_access(Access::RWInterleaved)?;
        buf_manager.apply_buffer_size(&hwp)?;
        buf_manager.apply_period_size(&hwp)?;

        // Apply
        pcmdev.hw_params(&hwp)?;
    }
    {
        // Set software parameters
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        buf_manager.apply_start_threshold(&swp)?;
        buf_manager.apply_avail_min(&swp)?;
        debug!(
            "Opening {} device \"{}\" with parameters: {:?}, {:?}",
            direction, devname, hwp, swp
        );
        pcmdev.sw_params(&swp)?;
        debug!("{} device \"{}\" successfully opened", direction, devname);
    }
    Ok(pcmdev)
}

fn playback_loop_bytes(
    channels: PlaybackChannels,
    pcmdevice: &alsa::PCM,
    params: PlaybackParams,
    buf_manager: &mut PlaybackBufferManager,
) {
    let mut timer = countertimer::Stopwatch::new();
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut buffer_avg = countertimer::Averager::new();
    let mut conversion_result;
    let adjust = params.adjust_period > 0.0 && params.adjust_enabled;
    let millis_per_frame: f32 = 1000.0 / params.samplerate as f32;
    let mut device_stalled = false;

    let io = pcmdevice.io_bytes();
    debug!("Playback loop uses a buffer of {} frames", params.chunksize);
    let mut buffer = vec![0u8; params.chunksize * params.bytes_per_frame];
    let pcminfo = pcmdevice.info().unwrap();
    let card = pcminfo.get_card();
    let device = pcminfo.get_device();
    let subdevice = pcminfo.get_subdevice();
    let mut element_uac2_gadget: Option<Elem> = None;
    // Virtual devices such as pcm plugins don't have a hw card ID
    // Only try to create the HCtl when the device has an ID
    let h = (card >= 0).then(|| HCtl::new(&format!("hw:{}", card), false).unwrap());
    if let Some(h) = &h {
        h.load().unwrap();
        let mut elid_uac2_gadget = ElemId::new(ElemIface::PCM);
        elid_uac2_gadget.set_device(device);
        elid_uac2_gadget.set_subdevice(subdevice);
        elid_uac2_gadget.set_name(&CString::new("Playback Pitch 1000000").unwrap());
        element_uac2_gadget = h.find_elem(&elid_uac2_gadget);
    }
    if element_uac2_gadget.is_some() {
        info!("Playback device supports rate adjust");
    }
    let mut capture_speed: f64 = 1.0;
    let mut prev_delay_diff: Option<f64> = None;
    loop {
        let eos_in_drain = if device_stalled {
            drain_check_eos(&channels.audio)
        } else {
            None
        };
        let msg = match eos_in_drain {
            Some(eos) => Ok(eos),
            None => {
                /* waiting for a new message */
                //trace!("PB: delay BEFORE chunk recv: {:?} frames", pcmdevice.status().ok().map(|status| status.get_delay()));
                channels.audio.recv()
            } /* waiting for a new message */
        };
        match msg {
            Ok(AudioMessage::Audio(chunk)) => {
                // measure delay only on running non-stalled device
                let delay_at_chunk_recvd = if !device_stalled
                    && pcmdevice.state_raw() == alsa_sys::SND_PCM_STATE_RUNNING as i32
                {
                    pcmdevice.status().ok().map(|status| status.get_delay())
                } else {
                    None
                };
                //trace!("PB: Delay at chunk rcvd: {:?}", delay_at_chunk_recvd);

                conversion_result =
                    chunk_to_buffer_rawbytes(&chunk, &mut buffer, &params.sample_format);

                trace!("PB: {:?}", buf_manager);
                let playback_res = play_buffer(
                    &buffer,
                    pcmdevice,
                    &io,
                    millis_per_frame,
                    params.bytes_per_frame,
                    buf_manager,
                );
                device_stalled = match playback_res {
                    Ok(PlaybackResult::Normal) => {
                        if device_stalled {
                            info!("PB: device resumed normal operation");
                            timer.restart();
                            buffer_avg.restart();
                        }
                        false
                    }
                    Ok(PlaybackResult::Stalled) => {
                        if !device_stalled {
                            // first stall detected
                            info!("PB: device stalled");
                            // restarting the device to drop outdated samples
                            pcmdevice
                                .drop()
                                .unwrap_or_else(|err| warn!("PB: Playback error {:?}", err));
                            pcmdevice
                                .prepare()
                                .unwrap_or_else(|err| warn!("PB: Playback error {:?}", err));
                            // writing zeros to be able to check for un-stalling in pcmdevice.wait
                            let zero_buf = vec![
                                0u8;
                                buf_manager.frames_to_stall() as usize
                                    * params.bytes_per_frame
                            ];
                            match io.writei(&zero_buf) {
                                Ok(frames) => {
                                    trace!("PB: Wrote {} zero frames", frames);
                                }
                                Err(err) => {
                                    warn!("PB: Writing stall-check zeros failed with {:?}", err);
                                }
                            };
                        }
                        true
                    }
                    Err(msg) => {
                        channels
                            .status
                            .send(StatusMessage::PlaybackError(msg.to_string()))
                            .unwrap_or(());
                        device_stalled
                    }
                };
                if !device_stalled {
                    // updates only for non-stalled device
                    chunk.update_stats(&mut chunk_stats);
                    {
                        let mut playback_status = params.playback_status.write();
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
                    if let Some(delay) = delay_at_chunk_recvd {
                        if delay != 0 {
                            buffer_avg.add_value(delay as f64);
                        }
                    }
                    if timer.larger_than_millis((1000.0 * params.adjust_period) as u64) {
                        if let Some(avg_delay) = buffer_avg.average() {
                            timer.restart();
                            buffer_avg.restart();
                            if adjust {
                                let (new_capture_speed, new_delay_diff) = adjust_speed(
                                    avg_delay,
                                    params.target_level,
                                    prev_delay_diff,
                                    capture_speed,
                                );
                                if prev_delay_diff.is_some() {
                                    // not first cycle
                                    capture_speed = new_capture_speed;
                                    if let Some(elem_uac2_gadget) = &element_uac2_gadget {
                                        let mut elval = ElemValue::new(ElemType::Integer).unwrap();
                                        // speed is reciprocal on playback side
                                        elval
                                            .set_integer(0, (1_000_000.0 / capture_speed) as i32)
                                            .unwrap();
                                        elem_uac2_gadget.write(&elval).unwrap();
                                    } else {
                                        channels
                                            .status
                                            .send(StatusMessage::SetSpeed(capture_speed))
                                            .unwrap_or(());
                                    }
                                }
                                prev_delay_diff = Some(new_delay_diff);
                            }
                            let mut playback_status = params.playback_status.write();
                            playback_status.buffer_level = avg_delay as usize;
                            debug!(
                                "PB: buffer level: {:.1}, signal rms: {:?}",
                                avg_delay,
                                playback_status.signal_rms.last()
                            );
                        }
                    }
                }
            }
            Ok(AudioMessage::Pause) => {
                trace!("PB: Pause message received");
            }
            Ok(AudioMessage::EndOfStream) => {
                channels
                    .status
                    .send(StatusMessage::PlaybackDone)
                    .unwrap_or(());
                break;
            }
            Err(err) => {
                error!("PB: Message channel error: {}", err);
                channels
                    .status
                    .send(StatusMessage::PlaybackError(err.to_string()))
                    .unwrap_or(());
                break;
            }
        }
    }
}

fn drain_check_eos(audio: &Receiver<AudioMessage>) -> Option<AudioMessage> {
    let mut eos: Option<AudioMessage> = None;
    while let Some(msg) = audio.try_iter().next() {
        if let AudioMessage::EndOfStream = msg {
            eos = Some(msg);
        }
    }
    eos
}

fn capture_loop_bytes(
    channels: CaptureChannels,
    pcmdevice: &alsa::PCM,
    params: CaptureParams,
    mut resampler: Option<Box<dyn VecResampler<PrcFmt>>>,
    buf_manager: &mut CaptureBufferManager,
) {
    let io = pcmdevice.io_bytes();
    let pcminfo = pcmdevice.info().unwrap();
    let card = pcminfo.get_card();
    let device = pcminfo.get_device();
    let subdevice = pcminfo.get_subdevice();

    let mut element_loopback: Option<Elem> = None;
    let mut element_uac2_gadget: Option<Elem> = None;
    // Virtual devices such as pcm plugins don't have a hw card ID
    // Only try to create the HCtl when the device has an ID
    let h = (card >= 0).then(|| HCtl::new(&format!("hw:{}", card), false).unwrap());
    if let Some(h) = &h {
        h.load().unwrap();
        let mut elid_loopback = ElemId::new(ElemIface::PCM);
        elid_loopback.set_device(device);
        elid_loopback.set_subdevice(subdevice);
        elid_loopback.set_name(&CString::new("PCM Rate Shift 100000").unwrap());
        element_loopback = h.find_elem(&elid_loopback);

        let mut elid_uac2_gadget = ElemId::new(ElemIface::PCM);
        elid_uac2_gadget.set_device(device);
        elid_uac2_gadget.set_subdevice(subdevice);
        elid_uac2_gadget.set_name(&CString::new("Capture Pitch 1000000").unwrap());
        element_uac2_gadget = h.find_elem(&elid_uac2_gadget);
    }

    if element_loopback.is_some() || element_uac2_gadget.is_some() {
        info!("Capture device supports rate adjust");
        if params.samplerate == params.capture_samplerate && resampler.is_some() {
            warn!("Needless 1:1 sample rate conversion active. Not needed since capture device supports rate adjust");
        } else if params.async_src && resampler.is_some() {
            warn!("Async resampler is used but not needed since capture device supports rate adjust. Consider switching to Synchronous type to save CPU time.");
        }
    }

    let buffer_frames = buf_manager.data().buffersize() as usize;
    debug!("Capture loop uses a buffer of {} frames", buffer_frames);
    let mut buffer = vec![0u8; buffer_frames * params.bytes_per_frame];

    let mut capture_bytes = params.chunksize * params.channels * params.store_bytes_per_sample;
    let mut capture_frames = params.chunksize as Frames;
    let mut averager = countertimer::TimeAverage::new();
    let mut watcher_averager = countertimer::TimeAverage::new();
    let mut valuewatcher = countertimer::ValueWatcher::new(
        params.capture_samplerate as f32,
        RATE_CHANGE_THRESHOLD_VALUE,
        RATE_CHANGE_THRESHOLD_COUNT,
    );
    let rate_measure_interval_ms = (1000.0 * params.rate_measure_interval) as u64;
    let mut rate_adjust = 0.0;
    let mut silence_counter = countertimer::SilenceCounter::new(
        params.silence_threshold,
        params.silence_timeout,
        params.capture_samplerate,
        params.chunksize,
    );
    let mut state = ProcessingState::Running;
    let mut value_range = 0.0;
    let mut device_stalled = false;
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut channel_mask = vec![true; params.channels];
    loop {
        match channels.command.try_recv() {
            Ok(CommandMessage::Exit) => {
                debug!("Exit message received, sending EndOfStream");
                let msg = AudioMessage::EndOfStream;
                channels.audio.send(msg).unwrap_or(());
                channels
                    .status
                    .send(StatusMessage::CaptureDone)
                    .unwrap_or(());
                break;
            }
            Ok(CommandMessage::SetSpeed { speed }) => {
                let mut elval = ElemValue::new(ElemType::Integer).unwrap();
                rate_adjust = speed;
                if let Some(elem_loopback) = &element_loopback {
                    elval.set_integer(0, (100_000.0 / speed) as i32).unwrap();
                    elem_loopback.write(&elval).unwrap();
                } else if let Some(elem_uac2_gadget) = &element_uac2_gadget {
                    elval.set_integer(0, (speed * 1_000_000.0) as i32).unwrap();
                    elem_uac2_gadget.write(&elval).unwrap();
                } else if let Some(resampl) = &mut resampler {
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
        let (new_capture_bytes, new_capture_frames) = nbr_capture_bytes_and_frames(
            capture_bytes,
            capture_frames,
            &resampler,
            &params,
            &mut buffer,
        );
        if new_capture_bytes != capture_bytes {
            trace!(
                "Updating capture bytes from {} to {}, and frames from {} to {}",
                capture_bytes,
                new_capture_bytes,
                capture_frames,
                new_capture_frames
            );
            capture_bytes = new_capture_bytes;
            capture_frames = new_capture_frames;
            // updating sw avail_min for snd_pcm_delay threshold
            update_avail_min(pcmdevice, new_capture_frames, buf_manager).unwrap_or(());
        }
        trace!("Capture: {:?}", buf_manager);
        let capture_res = capture_buffer(
            &mut buffer[0..capture_bytes],
            pcmdevice,
            &io,
            params.capture_samplerate,
            capture_frames as usize,
            params.bytes_per_frame,
        );
        match capture_res {
            Ok(CaptureResult::Normal) => {
                //trace!("Captured {} bytes", capture_bytes);
                averager.add_value(capture_bytes);
                {
                    let capture_status = params.capture_status.upgradable_read();
                    if averager.larger_than_millis(capture_status.update_interval as u64) {
                        device_stalled = false;
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
                watcher_averager.add_value(capture_bytes);
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
                            channels.audio.send(msg).unwrap_or(());
                            channels
                                .status
                                .send(StatusMessage::CaptureFormatChange(measured_rate_f as usize))
                                .unwrap_or(());
                            break;
                        }
                    }
                    trace!("Measured sample rate is {:.1} Hz", measured_rate_f);
                }
            }
            Ok(CaptureResult::Stalled) => {
                // only the first time
                if !device_stalled {
                    info!("Capture device is stalled, processing is stalled");
                    device_stalled = true;
                    // restarting the device to drop outdated samples
                    pcmdevice
                        .drop()
                        .unwrap_or_else(|err| warn!("Capture error {:?}", err));
                    pcmdevice
                        .prepare()
                        .unwrap_or_else(|err| warn!("Capture error {:?}", err));
                    params.capture_status.write().state = ProcessingState::Stalled;
                }
            }
            Err(msg) => {
                channels
                    .status
                    .send(StatusMessage::CaptureError(msg.to_string()))
                    .unwrap_or(());
                let msg = AudioMessage::EndOfStream;
                channels.audio.send(msg).unwrap_or(());
                return;
            }
        };
        let mut chunk = buffer_to_chunk_rawbytes(
            &buffer[0..capture_bytes],
            params.channels,
            &params.sample_format,
            capture_bytes,
            &params.capture_status.read().used_channels,
        );
        chunk.update_stats(&mut chunk_stats);
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
        if device_stalled {
            state = ProcessingState::Stalled;
        } else {
            state = silence_counter.update(value_range);
        }
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
                chunk.valid_frames = chunk.frames;
                chunk.waveforms = new_waves;
            }
            let msg = AudioMessage::Audio(chunk);
            if channels.audio.send(msg).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
        } else if state == ProcessingState::Paused || state == ProcessingState::Stalled {
            let msg = AudioMessage::Pause;
            if channels.audio.send(msg).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
        }
    }
    params.capture_status.write().state = ProcessingState::Inactive;
}

fn update_avail_min(
    pcmdevice: &PCM,
    frames: Frames,
    buf_manager: &mut dyn DeviceBufferManager,
) -> Res<()> {
    let swp = pcmdevice.sw_params_current()?;
    buf_manager.update_io_size(&swp, frames)?;
    pcmdevice.sw_params(&swp)?;
    Ok(())
}

fn nbr_capture_bytes_and_frames(
    capture_bytes: usize,
    capture_frames: Frames,
    resampler: &Option<Box<dyn VecResampler<PrcFmt>>>,
    params: &CaptureParams,
    buf: &mut Vec<u8>,
) -> (usize, Frames) {
    let (capture_bytes_new, capture_frames_new) = if let Some(resampl) = &resampler {
        //trace!("Resampler needs {} frames", resampl.input_frames_next());
        let frames = resampl.input_frames_next();
        (
            frames * params.channels * params.store_bytes_per_sample,
            frames as Frames,
        )
    } else {
        (capture_bytes, capture_frames)
    };
    if capture_bytes_new > buf.len() {
        debug!("Capture buffer too small, extending");
        buf.append(&mut vec![0u8; capture_bytes_new - buf.len()]);
    }
    (capture_bytes_new, capture_frames_new)
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
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
        let bytes_per_sample = self.sample_format.bytes_per_sample();
        let sample_format = self.sample_format;
        let mut buf_manager =
            PlaybackBufferManager::new(chunksize as Frames, target_level as Frames);
        let handle = thread::Builder::new()
            .name("AlsaPlayback".to_string())
            .spawn(move || {
                match open_pcm(
                    devname,
                    samplerate as u32,
                    channels as u32,
                    &sample_format,
                    &mut buf_manager,
                    false,
                ) {
                    Ok(pcmdevice) => {
                        match status_channel.send(StatusMessage::PlaybackReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }

                        barrier.wait();
                        debug!("Starting playback loop");
                        let pb_params = PlaybackParams {
                            channels,
                            target_level,
                            adjust_period,
                            adjust_enabled,
                            sample_format,
                            playback_status,
                            bytes_per_frame: channels * bytes_per_sample,
                            samplerate,
                            chunksize,
                        };
                        let pb_channels = PlaybackChannels {
                            audio: channel,
                            status: status_channel,
                        };
                        playback_loop_bytes(pb_channels, &pcmdevice, pb_params, &mut buf_manager);
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

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
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
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let sample_format = self.sample_format;
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = self.rate_measure_interval;
        let mut buf_manager = CaptureBufferManager::new(
            chunksize as Frames,
            samplerate as f32 / capture_samplerate as f32,
        );

        let handle = thread::Builder::new()
            .name("AlsaCapture".to_string())
            .spawn(move || {
                let resampler = new_resampler(
                    &resampler_config,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                );
                match open_pcm(
                    devname,
                    capture_samplerate as u32,
                    channels as u32,
                    &sample_format,
                    &mut buf_manager,
                    true,
                ) {
                    Ok(pcmdevice) => {
                        match status_channel.send(StatusMessage::CaptureReady) {
                            Ok(()) => {}
                            Err(_err) => {}
                        }
                        barrier.wait();
                        debug!("Starting captureloop");
                        let cap_params = CaptureParams {
                            channels,
                            sample_format,
                            silence_timeout,
                            silence_threshold,
                            chunksize,
                            store_bytes_per_sample,
                            bytes_per_frame: channels * store_bytes_per_sample,
                            samplerate,
                            capture_samplerate,
                            async_src,
                            capture_status,
                            stop_on_rate_change,
                            rate_measure_interval,
                        };
                        let cap_channels = CaptureChannels {
                            audio: channel,
                            status: status_channel,
                            command: command_channel,
                        };
                        capture_loop_bytes(
                            cap_channels,
                            &pcmdevice,
                            cap_params,
                            resampler,
                            &mut buf_manager,
                        );
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
