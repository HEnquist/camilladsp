// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

extern crate alsa;
extern crate nix;
use crate::audiochunk::ChunkStats;
use crate::audiodevice::*;
use crate::config::{AlsaSampleFormat, BinarySampleFormat, Resampler};
use crate::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
use crate::utils::countertimer;
use alsa::ctl::{Ctl, ElemId, ElemIface, ElemType, ElemValue};
use alsa::hctl::HCtl;
use alsa::pcm::{Access, Format, Frames, HwParams};
use alsa::poll::Descriptors;
use alsa::{Direction, ValueOr};
use alsa_sys;
use audio_thread_priority::{
    demote_current_thread_from_real_time, promote_current_thread_to_real_time,
};
use crossbeam_channel;
use nix::errno::Errno;
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use ringbuf::{HeapRb, traits::*};
use std::ffi::CString;
use std::fmt::Debug;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::alsa_backend::threaded_buffermanager::{
    CaptureBufferManager, DeviceBufferManager, PlaybackBufferManager,
};
use crate::alsa_backend::utils::{
    CaptureElements, CaptureParams, CaptureResult, ElemData, FileDescriptors, find_elem,
    list_channels_as_text, list_device_names, list_formats_as_text, list_samplerates_as_text,
    pick_preferred_format, process_events, state_desc, sync_linked_controls,
};
use crate::utils::rate_controller::PIRateController;
use crate::utils::resampling::{ChunkResampler, new_resampler, resampler_is_async};
use crate::{CaptureStatus, PlaybackStatus, ProcessingParameters};

lazy_static! {
    static ref ALSA_MUTEX: Mutex<()> = Mutex::new(());
}

pub struct AlsaPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: Option<AlsaSampleFormat>,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
}

pub struct AlsaCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub capture_samplerate: usize,
    pub resampler_config: Option<Resampler>,
    pub chunksize: usize,
    pub channels: usize,
    pub sample_format: Option<AlsaSampleFormat>,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
    pub stop_on_inactive: bool,
    pub link_volume_control: Option<String>,
    pub link_mute_control: Option<String>,
}

#[derive(Debug)]
enum PlaybackResult {
    Normal(usize),
    Stalled,
}

fn prepare_playback_bytes(
    buffer: &mut Vec<u8>,
    write_remainder: &[u8],
    available_bytes: usize,
    bytes_per_frame: usize,
    mut pop_from_ring: impl FnMut(&mut [u8]),
) -> Option<usize> {
    if write_remainder.is_empty() {
        let aligned_bytes = available_bytes - (available_bytes % bytes_per_frame);
        if aligned_bytes == 0 {
            return None;
        }
        if aligned_bytes > buffer.len() {
            buffer.resize(aligned_bytes, 0);
        }
        pop_from_ring(&mut buffer[0..aligned_bytes]);
        Some(aligned_bytes)
    } else {
        let rem = write_remainder.len();
        if rem > buffer.len() {
            buffer.resize(rem, 0);
        }
        buffer[0..rem].copy_from_slice(write_remainder);
        Some(rem)
    }
}

fn apply_playback_write_result(
    playback_res: Res<PlaybackResult>,
    bytes_to_play: usize,
    buffer: &[u8],
    queued_bytes: &mut usize,
    write_remainder: &mut Vec<u8>,
    was_stalled: bool,
    status_channel: &crossbeam_channel::Sender<StatusMessage>,
    recover_on_first_stall: Option<(
        &alsa::PCM,
        &alsa::pcm::IO<u8>,
        &PlaybackBufferManager,
        usize,
        &[u8], // pre-allocated zero buffer for stall recovery
    )>,
) -> bool {
    match playback_res {
        Ok(PlaybackResult::Normal(bytes_written)) => {
            let bytes_written = bytes_written.min(bytes_to_play);
            *queued_bytes = queued_bytes.saturating_sub(bytes_written);
            if bytes_written < bytes_to_play {
                write_remainder.clear();
                write_remainder.extend_from_slice(&buffer[bytes_written..bytes_to_play]);
            } else {
                write_remainder.clear();
            }
            false
        }
        Ok(PlaybackResult::Stalled) => {
            if write_remainder.is_empty() {
                write_remainder.extend_from_slice(&buffer[0..bytes_to_play]);
            }
            if !was_stalled {
                warn!("PB: device stalled");
                if let Some((pcmdevice, io, buf_manager, bytes_per_frame, stall_zero_buf)) =
                    recover_on_first_stall
                {
                    pcmdevice
                        .drop()
                        .unwrap_or_else(|err| warn!("PB: Playback error {err:?}"));
                    pcmdevice
                        .prepare()
                        .unwrap_or_else(|err| warn!("PB: Playback error {err:?}"));
                    let stall_bytes = buf_manager.frames_to_stall() as usize * bytes_per_frame;
                    io.writei(&stall_zero_buf[..stall_bytes])
                        .unwrap_or_default();
                }
            }
            true
        }
        Err(msg) => {
            status_channel
                .send(StatusMessage::PlaybackError(msg.to_string()))
                .unwrap_or(());
            was_stalled
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_playback_inner_loop<C>(
    rx_play: &crossbeam_channel::Receiver<PlaybackDeviceMessage>,
    device_consumer: &mut C,
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
    buf_manager: &PlaybackBufferManager,
    status_channel_inner: &crossbeam_channel::Sender<StatusMessage>,
    buffer_fill: &Mutex<countertimer::DeviceBufferEstimator>,
    samplerate: usize,
    chunksize: usize,
    channels: usize,
    bytes_per_sample: usize,
) where
    C: Consumer<Item = u8> + Observer,
{
    let bytes_per_frame = channels * bytes_per_sample;
    let millis_per_frame: f32 = 1000.0 / samplerate as f32;
    let mut device_stalled = false;
    let mut pcm_paused = false;
    let can_pause = pcmdevice
        .hw_params_current()
        .map(|p| p.can_pause())
        .unwrap_or_default();
    // Pre-allocate all buffers to avoid heap allocations on the RT thread.
    // Size buffer generously: the ring buffer can hold several chunks, so
    // available_bytes in prepare_playback_bytes can exceed one chunksize.
    let mut buffer = vec![0u8; 4 * chunksize * channels * bytes_per_sample];
    let mut sample_queue_bytes = 0usize;
    let mut write_remainder: Vec<u8> = Vec::with_capacity(chunksize * channels * bytes_per_sample);
    let mut _pitch_hctl: Option<HCtl> = None;
    let mut pitch_elem = None;
    if let Ok(pcminfo) = pcmdevice.info() {
        let card = pcminfo.get_card();
        if card >= 0 {
            if let Ok(h) = HCtl::new(&format!("hw:{card}"), false) {
                h.load().unwrap_or_default();
                _pitch_hctl = Some(h);
            }
            if let Some(ref h) = _pitch_hctl {
                let mut elid_uac2_gadget = ElemId::new(ElemIface::PCM);
                elid_uac2_gadget.set_device(pcminfo.get_device());
                elid_uac2_gadget.set_subdevice(pcminfo.get_subdevice());
                if let Ok(name) = CString::new("Playback Pitch 1000000") {
                    elid_uac2_gadget.set_name(&name);
                    pitch_elem = h.find_elem(&elid_uac2_gadget);
                }
            }
        }
    }

    // Pre-allocate an ElemValue for pitch control writes, avoiding
    // snd_ctl_elem_value_malloc on the RT hot path.
    let mut pitch_elval = ElemValue::new(ElemType::Integer).ok();

    // Pre-allocate zero buffer for stall recovery writes, avoiding
    // heap allocation on the RT thread during error recovery.
    let stall_zero_buf = vec![0u8; buf_manager.frames_to_stall() as usize * bytes_per_frame];

    let mut end_of_stream = false;
    let mut channel_disconnected = false;
    let mut no_data_count = 0u32;

    loop {
        let available_bytes = sample_queue_bytes.min(device_consumer.occupied_len());
        let have_data = available_bytes >= bytes_per_frame || !write_remainder.is_empty();

        if have_data {
            no_data_count = 0;
            // Drain pending messages non-blocking to keep sample_queue_bytes up to date.
            // The device paces the loop via play_buffer's internal pcmdevice.wait(),
            // so we only need try_recv here - no blocking required.
            loop {
                match rx_play.try_recv() {
                    Ok(PlaybackDeviceMessage::Data(bytes)) => {
                        sample_queue_bytes = sample_queue_bytes.saturating_add(bytes);
                    }
                    Ok(PlaybackDeviceMessage::Pause) => {
                        if can_pause && !pcm_paused {
                            if pcmdevice.pause(true).is_ok() {
                                pcm_paused = true;
                            }
                        }
                    }
                    Ok(PlaybackDeviceMessage::SetPitch(speed)) => {
                        if let Some(elem_uac2_gadget) = &pitch_elem {
                            if let Some(ref mut elval) = pitch_elval {
                                elval.set_integer(0, (1_000_000.0 / speed) as i32).unwrap();
                                elem_uac2_gadget.write(elval).unwrap();
                            }
                        }
                    }
                    Ok(PlaybackDeviceMessage::EndOfStream) => {
                        end_of_stream = true;
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        channel_disconnected = true;
                        break;
                    }
                }
            }

            let available_bytes = sample_queue_bytes.min(device_consumer.occupied_len());
            let bytes_to_play = match prepare_playback_bytes(
                &mut buffer,
                &write_remainder,
                available_bytes,
                bytes_per_frame,
                |dst| {
                    device_consumer.pop_slice(dst);
                },
            ) {
                Some(n) => n,
                None => {
                    // Not enough data in the ring buffer for a full frame.
                    // Write silence to keep the device running, like WASAPI.
                    debug!("PB: Not enough data for a full frame, writing silence");
                    let silence_bytes = chunksize * bytes_per_frame;
                    buffer[..silence_bytes].fill(0);
                    silence_bytes
                }
            };

            let playback_res = play_buffer(
                &buffer[0..bytes_to_play],
                pcmdevice,
                io,
                millis_per_frame,
                bytes_per_frame,
                buf_manager,
            );
            pcm_paused = false;
            device_stalled = apply_playback_write_result(
                playback_res,
                bytes_to_play,
                &buffer,
                &mut sample_queue_bytes,
                &mut write_remainder,
                device_stalled,
                status_channel_inner,
                Some((pcmdevice, io, buf_manager, bytes_per_frame, &stall_zero_buf)),
            );

            // Update the buffer level estimator after each write (like WASAPI).
            // This lets the outer thread interpolate accurate fill levels between updates.
            if !device_stalled {
                if pcmdevice.state_raw() == alsa_sys::SND_PCM_STATE_RUNNING as i32 {
                    if let Some(avail) = pcmdevice.avail().ok() {
                        let delay = buf_manager.current_delay(avail) as usize;
                        let ring_frames = device_consumer.occupied_len() / bytes_per_frame;
                        let channel_frames = rx_play.len() * chunksize;
                        if let Some(mut est) = buffer_fill.try_lock() {
                            est.add(delay + ring_frames + channel_frames);
                        }
                    }
                }
            }
        } else if end_of_stream || channel_disconnected {
            // All queued data has been written to the device.
            if channel_disconnected {
                status_channel_inner
                    .send(StatusMessage::PlaybackError(
                        "Playback inner queue disconnected".to_string(),
                    ))
                    .unwrap_or(());
            } else {
                status_channel_inner
                    .send(StatusMessage::PlaybackDone)
                    .unwrap_or(());
            }
            if !pcm_paused {
                pcmdevice.drain().unwrap_or_default();
            }
            break;
        } else {
            // No data available yet - block on channel until new data arrives.
            let timeout_ms = ((1000.0 * chunksize as f32 / samplerate as f32) as u64).max(1);
            match rx_play.recv_timeout(Duration::from_millis(timeout_ms)) {
                Ok(PlaybackDeviceMessage::Data(bytes)) => {
                    sample_queue_bytes = sample_queue_bytes.saturating_add(bytes);
                }
                Ok(PlaybackDeviceMessage::Pause) => {
                    if can_pause && !pcm_paused {
                        if pcmdevice.pause(true).is_ok() {
                            pcm_paused = true;
                        }
                    }
                }
                Ok(PlaybackDeviceMessage::SetPitch(speed)) => {
                    if let Some(elem_uac2_gadget) = &pitch_elem {
                        if let Some(ref mut elval) = pitch_elval {
                            elval.set_integer(0, (1_000_000.0 / speed) as i32).unwrap();
                            elem_uac2_gadget.write(elval).unwrap();
                        }
                    }
                }
                Ok(PlaybackDeviceMessage::EndOfStream) => {
                    status_channel_inner
                        .send(StatusMessage::PlaybackDone)
                        .unwrap_or(());
                    if !pcm_paused {
                        pcmdevice.drain().unwrap_or_default();
                    }
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Only write silence if the device buffer is running low.
                    // The ALSA device buffer may still have plenty of valid audio
                    // even though the ring buffer is momentarily empty (the non-RT
                    // outer thread can be slightly late delivering data).
                    // Writing silence into a healthy buffer corrupts the audio stream
                    // with audible gaps.
                    let device_delay =
                        if pcmdevice.state_raw() == alsa_sys::SND_PCM_STATE_RUNNING as i32 {
                            pcmdevice
                                .avail()
                                .ok()
                                .map(|avail| buf_manager.current_delay(avail))
                        } else {
                            None
                        };
                    let buffer_low =
                        device_delay.map_or(true, |delay| (delay as usize) < chunksize);
                    if buffer_low {
                        no_data_count += 1;
                        if no_data_count == 4 {
                            warn!("PB: Playback interrupted, no data available");
                        }
                        let silence_bytes = chunksize * bytes_per_frame;
                        buffer[..silence_bytes].fill(0);
                        let _ = play_buffer(
                            &buffer[..silence_bytes],
                            pcmdevice,
                            io,
                            millis_per_frame,
                            bytes_per_frame,
                            buf_manager,
                        );
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    status_channel_inner
                        .send(StatusMessage::PlaybackError(
                            "Playback inner queue disconnected".to_string(),
                        ))
                        .unwrap_or(());
                    if !pcm_paused {
                        pcmdevice.drain().unwrap_or_default();
                    }
                    break;
                }
            }
        }
    }
}

/// Play a buffer.
fn play_buffer(
    buffer: &[u8],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
    millis_per_frame: f32,
    bytes_per_frame: usize,
    buf_manager: &PlaybackBufferManager,
) -> Res<PlaybackResult> {
    let playback_state = pcmdevice.state_raw();
    xtrace!("Playback state {:?}", playback_state);
    if playback_state < 0 {
        // This should never happen but sometimes does anyway,
        // for example if a USB device is unplugged.
        let nixerr = Errno::from_raw(-playback_state);
        error!(
            "PB: Alsa snd_pcm_state() of playback device returned an unexpected error: {nixerr}"
        );
        return Err(Box::new(nixerr));
    } else if playback_state == alsa_sys::SND_PCM_STATE_XRUN as i32 {
        warn!("PB: Prepare playback after buffer underrun");
        pcmdevice.prepare()?;
        buf_manager.sleep_for_target_delay(millis_per_frame);
    } else if playback_state == alsa_sys::SND_PCM_STATE_PREPARED as i32 {
        info!("PB: Starting playback from Prepared state");
        // This sleep applies for the first chunk and in combination with the threshold=1 (i.e. start at first write)
        // and the next chunk generates the initial target delay.
        buf_manager.sleep_for_target_delay(millis_per_frame);
    } else if playback_state == alsa_sys::SND_PCM_STATE_PAUSED as i32 {
        debug!("PB: Device is in paused state, unpausing.");
        if let Err(err) = pcmdevice.pause(false) {
            warn!("Error unpausing playback device {err:?}");
        }
    } else if playback_state != alsa_sys::SND_PCM_STATE_RUNNING as i32 {
        warn!(
            "PB: device is in an unexpected state: {}",
            state_desc(playback_state as u32)
        );
    }

    let frames_to_write = buffer.len() / bytes_per_frame;
    // Use a timeout based on the full device buffer size, not the current write size.
    // A write-size-based timeout is too tight when the buffer is nearly full
    // (e.g. after stall recovery), causing spurious timeouts and cascading stalls.
    // The buffer-based timeout gives the device enough time to drain at least
    // avail_min frames from a full buffer, which is the worst-case normal scenario.
    let mut timeout_millis = (2.0 * millis_per_frame * buf_manager.data.buffersize() as f32) as u32;
    if timeout_millis < 20 {
        timeout_millis = 20;
    }
    trace!("PB: pcmdevice.wait with timeout {timeout_millis} ms");
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
            if Errno::from_raw(err.errno()) == Errno::EPIPE {
                warn!("PB: wait underrun, trying to recover. Error: {err}");
                trace!("snd_pcm_prepare");
                // Would recover() be better than prepare()?
                pcmdevice.prepare()?;
            } else {
                warn!("PB: device failed while waiting for available buffer space, error: {err}");
                return Err(Box::new(err));
            }
        }
    }

    //trace!("Delay BEFORE writing {} is {:?} frames",  buffer.len() / bytes_per_frame, pcmdevice.status().ok().map(|status| status.get_delay()));
    match io.writei(buffer) {
        Ok(frames_written) => {
            //trace!("Delay AFTER writing {} is {:?} frames", frames_written, pcmdevice.status().ok().map(|status| status.get_delay()));
            trace!("PB: wrote {frames_written} frames out of requested {frames_to_write}");
            Ok(PlaybackResult::Normal(frames_written * bytes_per_frame))
        }
        Err(err) => match Errno::from_raw(err.errno()) {
            Errno::EAGAIN => {
                trace!("PB: encountered EAGAIN error on write, trying later");
                Ok(PlaybackResult::Normal(0))
            }
            Errno::EPIPE => {
                warn!("PB: write underrun, trying to recover. Error: {err}");
                trace!("snd_pcm_prepare");
                // Would recover() be better than prepare()?
                pcmdevice.prepare()?;
                buf_manager.sleep_for_target_delay(millis_per_frame);
                Ok(PlaybackResult::Normal(0))
            }
            _ => {
                warn!("PB: write failed, error: {err}");
                Err(Box::new(err))
            }
        },
    }
}

/// Capture a buffer.
#[allow(clippy::too_many_arguments)]
fn capture_buffer(
    buffer: &mut [u8],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<u8>,
    frames_to_read: usize,
    fds: &mut FileDescriptors,
    ctl: &Option<Ctl>,
    hctl: &Option<HCtl>,
    elems: &CaptureElements,
    status_channel: &crossbeam_channel::Sender<StatusMessage>,
    params: &mut CaptureParams,
    processing_params: &Arc<ProcessingParameters>,
) -> Res<(CaptureResult, usize)> {
    let capture_state = pcmdevice.state_raw();
    if capture_state == alsa_sys::SND_PCM_STATE_XRUN as i32 {
        warn!("Prepare capture device");
        pcmdevice.prepare()?;
    } else if capture_state < 0 {
        // This should never happen but sometimes does anyway,
        // for example if a USB device is unplugged.
        let nixerr = Errno::from_raw(-capture_state);
        error!(
            "Alsa snd_pcm_state() of capture device returned an unexpected error: {capture_state}"
        );
        return Err(Box::new(nixerr));
    } else if capture_state != alsa_sys::SND_PCM_STATE_RUNNING as i32 {
        debug!(
            "Starting capture from state: {}",
            state_desc(capture_state as u32)
        );
        pcmdevice.start()?;
    }
    let millis_per_chunk = 1000 * frames_to_read / params.samplerate;

    let mut timeout_millis = 8 * millis_per_chunk as u32;
    if timeout_millis < 20 {
        timeout_millis = 20;
    }
    let start = if log_enabled!(log::Level::Trace) {
        Some(Instant::now())
    } else {
        None
    };
    trace!("Capture pcmdevice.wait with timeout {timeout_millis} ms");
    loop {
        match fds.wait(timeout_millis as i32) {
            Ok(pollresult) => {
                if pollresult.poll_res == 0 {
                    debug!("Capture wait timed out after {timeout_millis} ms, device stalled");
                    return Ok((CaptureResult::Stalled, 0));
                }
                if pollresult.ctl {
                    trace!("Got a control event");
                    if let Some(c) = ctl {
                        let event_result =
                            process_events(c, elems, status_channel, params, processing_params);
                        match event_result {
                            CaptureResult::Done => return Ok((event_result, 0)),
                            CaptureResult::Stalled => debug!("Capture device is stalled"),
                            CaptureResult::Normal => {}
                        };
                    }
                    if let Some(h) = hctl {
                        let ev = h.handle_events().unwrap();
                        trace!("hctl handle events {ev}");
                    }
                }
                if pollresult.pcm {
                    trace!("Capture waited for {:?}", start.map(|s| s.elapsed()));
                    break;
                }
            }
            Err(err) => {
                if Errno::from_raw(err.errno()) == Errno::EPIPE {
                    warn!("Capture: wait overrun, trying to recover. Error: {err}");
                    trace!("snd_pcm_prepare");
                    // Would recover() be better than prepare()?
                    pcmdevice.prepare()?;
                    break;
                } else {
                    warn!(
                        "Capture: device failed while waiting for available frames, error: {err}"
                    );
                    return Err(Box::new(err));
                }
            }
        }
    }
    match io.readi(buffer) {
        Ok(frames_read) => {
            let bytes_read = frames_read * params.bytes_per_frame;
            if frames_read == 0 {
                debug!("Capture read returned 0 frames, device stalled");
                Ok((CaptureResult::Stalled, 0))
            } else {
                trace!("Capture read {frames_read} frames");
                Ok((CaptureResult::Normal, bytes_read))
            }
        }
        Err(err) => match Errno::from_raw(err.errno()) {
            Errno::EIO => {
                warn!("Capture: read failed with error: {err}");
                Err(Box::new(err))
            }
            Errno::EAGAIN => {
                trace!("Capture: encountered EAGAIN error on read, trying later");
                Ok((CaptureResult::Normal, 0))
            }
            Errno::EPIPE => {
                warn!("Capture: read overrun, trying to recover. Error: {err}");
                trace!("snd_pcm_prepare");
                // Would recover() be better than prepare()?
                pcmdevice.prepare()?;
                Ok((CaptureResult::Normal, 0))
            }
            _ => {
                warn!("Capture failed, error: {err}");
                Err(Box::new(err))
            }
        },
    }
}

/// Open an Alsa PCM device
fn open_pcm(
    devname: String,
    samplerate: u32,
    channels: u32,
    sample_format: &Option<AlsaSampleFormat>,
    buf_manager: &mut dyn DeviceBufferManager,
    capture: bool,
) -> Res<(alsa::PCM, AlsaSampleFormat)> {
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
    let chosen_format;
    {
        let hwp = HwParams::any(&pcmdev)?;

        // Set number of channels
        debug!("{}: {}", direction, list_channels_as_text(&hwp));
        debug!("{direction}: setting channels to {channels}");
        hwp.set_channels(channels)?;

        // Set samplerate
        debug!("{}: {}", direction, list_samplerates_as_text(&hwp));
        debug!("{direction}: setting rate to {samplerate}");
        hwp.set_rate(samplerate, ValueOr::Nearest)?;

        // Set sample format
        debug!("{}: {}", direction, list_formats_as_text(&hwp));
        chosen_format = match sample_format {
            Some(sfmt) => *sfmt,
            None => {
                let preferred = pick_preferred_format(&hwp)
                    .ok_or(DeviceError::new("Unable to find a supported sample format"))?;
                debug!("{direction}: Picked sample format {preferred:?}");
                preferred
            }
        };
        debug!("{direction}: setting format to {chosen_format:?}");
        match chosen_format {
            AlsaSampleFormat::S16_LE => hwp.set_format(Format::s16())?,
            AlsaSampleFormat::S24_4_LE => hwp.set_format(Format::s24())?,
            AlsaSampleFormat::S24_3_LE => hwp.set_format(Format::s24_3())?,
            AlsaSampleFormat::S32_LE => hwp.set_format(Format::s32())?,
            AlsaSampleFormat::F32_LE => hwp.set_format(Format::float())?,
            AlsaSampleFormat::F64_LE => hwp.set_format(Format::float64())?,
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
        debug!("Opening {direction} device \"{devname}\" with parameters: {hwp:?}, {swp:?}");
        pcmdev.sw_params(&swp)?;
        debug!("{direction} device \"{devname}\" successfully opened");
    }
    Ok((pcmdev, chosen_format))
}

fn send_capture_audio(
    channel: &crossbeam_channel::Sender<AudioMessage>,
    msg: AudioMessage,
) -> bool {
    match msg {
        AudioMessage::EndOfStream => channel.send(AudioMessage::EndOfStream).is_ok(),
        _ => match channel.try_send(msg) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                trace!("Capture: downstream queue full, dropping message");
                true
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        },
    }
}

fn nbr_capture_frames(resampler: &Option<ChunkResampler>, capture_frames: usize) -> usize {
    if let Some(resampl) = &resampler {
        resampl.resampler.input_frames_next()
    } else {
        capture_frames
    }
}

enum AlsaThreadState {
    Ready(BinarySampleFormat),
    Error(String),
}

enum PlaybackDeviceMessage {
    Data(usize),
    Pause,
    EndOfStream,
    SetPitch(f64),
}

enum CaptureDeviceMessage {
    Data { chunk_nbr: usize, nbr_bytes: usize },
    EndOfStream,
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
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
        let conf_sample_format = self.sample_format;

        let handle = thread::Builder::new()
            .name("AlsaPlayback".to_string())
            .spawn(move || {
                let channel_capacity = 8 * 1024 / chunksize + 3;
                debug!("Using a playback channel capacity of {channel_capacity} chunks.");
                let (tx_dev, rx_dev) = crossbeam_channel::bounded(channel_capacity);
                let (tx_state_dev, rx_state_dev) = crossbeam_channel::bounded(0);
                let (tx_start, rx_start) = crossbeam_channel::bounded(0);

                let ringbuffer = HeapRb::<u8>::new(channels * 4 * (2 * chunksize + 2048));
                let (mut device_producer, mut device_consumer) = ringbuffer.split();

                let status_channel_inner = status_channel.clone();
                let buffer_fill = Arc::new(Mutex::new(
                    countertimer::DeviceBufferEstimator::new(samplerate),
                ));
                let buffer_fill_inner = buffer_fill.clone();

                let innerhandle = thread::Builder::new()
                    .name("AlsaPlaybackInner".to_string())
                    .spawn(move || {
                        let mut buf_manager =
                            PlaybackBufferManager::new(chunksize as Frames, target_level as Frames);
                        match open_pcm(
                            devname,
                            samplerate as u32,
                            channels as u32,
                            &conf_sample_format,
                            &mut buf_manager,
                            false,
                        ) {
                            Ok((pcmdevice, sample_format)) => {
                                let binary_format = sample_format.to_binary_format();
                                tx_state_dev
                                    .send(AlsaThreadState::Ready(binary_format))
                                    .unwrap_or(());
                                if rx_start.recv().is_err() {
                                    return;
                                }

                                let io = pcmdevice.io_bytes();

                                let thread_handle = match promote_current_thread_to_real_time(
                                    chunksize as u32,
                                    samplerate as u32,
                                ) {
                                    Ok(h) => Some(h),
                                    Err(err) => {
                                        warn!(
                                            "Playback inner thread could not get real time priority, error: {err}"
                                        );
                                        None
                                    }
                                };
                                run_playback_inner_loop(
                                    &rx_dev,
                                    &mut device_consumer,
                                    &pcmdevice,
                                    &io,
                                    &buf_manager,
                                    &status_channel_inner,
                                    &buffer_fill_inner,
                                    samplerate,
                                    chunksize,
                                    channels,
                                    binary_format.bytes_per_sample(),
                                );

                                if let Some(h) = thread_handle {
                                    demote_current_thread_from_real_time(h).unwrap_or_default();
                                }
                            }
                            Err(err) => {
                                tx_state_dev
                                    .send(AlsaThreadState::Error(err.to_string()))
                                    .unwrap_or(());
                            }
                        }
                    })
                    .unwrap();

                match rx_state_dev.recv() {
                    Ok(AlsaThreadState::Ready(binary_format)) => {
                        status_channel
                            .send(StatusMessage::PlaybackReady)
                            .unwrap_or(());
                        barrier.wait();
                        tx_start.send(()).unwrap_or(());

                        let mut chunk_stats = ChunkStats {
                            rms: vec![0.0; channels],
                            peak: vec![0.0; channels],
                        };
                        let mut buf =
                            vec![0u8; channels * chunksize * binary_format.bytes_per_sample()];

                        // Buffer level tracking with time-based estimation (like WASAPI)
                        let adjust = adjust_period > 0.0 && adjust_enabled;
                        let mut buffer_avg = countertimer::Averager::new();
                        let mut timer = countertimer::Stopwatch::new();
                        let mut rate_controller = PIRateController::new_with_default_gains(
                            samplerate,
                            adjust_period as f64,
                            target_level,
                        );

                        loop {
                            match channel.recv() {
                                Ok(AudioMessage::Audio(chunk)) => {
                                    // Sample the estimator on each chunk (like WASAPI)
                                    let estimated_buffer_fill = buffer_fill
                                        .try_lock()
                                        .map(|b| b.estimate() as f64)
                                        .unwrap_or_default();
                                    buffer_avg.add_value(
                                        estimated_buffer_fill
                                            + (channel.len() * chunksize) as f64,
                                    );
                                    if adjust
                                        && timer.larger_than_millis(
                                            (1000.0 * adjust_period) as u64,
                                        )
                                    {
                                        if let Some(av_delay) = buffer_avg.average() {
                                            let speed = rate_controller.next(av_delay);
                                            timer.restart();
                                            buffer_avg.restart();
                                            debug!(
                                                "PB: buffer level {:.1}, set capture rate to {:.6}",
                                                av_delay, speed
                                            );
                                            status_channel
                                                .send(StatusMessage::SetSpeed(speed))
                                                .unwrap_or(());
                                            tx_dev
                                                .send(PlaybackDeviceMessage::SetPitch(speed))
                                                .unwrap_or(());
                                            if let Some(mut ps) = playback_status.try_write() {
                                                ps.buffer_level = av_delay as usize;
                                            }
                                        }
                                    }

                                    chunk.update_stats(&mut chunk_stats);
                                    let conversion_result =
                                        chunk_to_buffer_rawbytes(chunk, &mut buf, &binary_format);
                                    if let Some(mut playback_status) = playback_status.try_write() {
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

                                    let bytes_to_write = conversion_result.0;
                                    let sleep_duration = Duration::from_micros(
                                        (1_000_000 * chunksize / samplerate / 2) as u64,
                                    );
                                    let max_retries = 16;
                                    for _ in 0..max_retries {
                                        if device_producer.vacant_len() >= bytes_to_write {
                                            break;
                                        }
                                        std::thread::sleep(sleep_duration);
                                    }
                                    if device_producer.vacant_len() >= bytes_to_write {
                                        device_producer.push_slice(&buf[0..bytes_to_write]);
                                    } else {
                                        warn!(
                                            "Playback ring buffer is full, dropped chunk of {bytes_to_write} bytes"
                                        );
                                        continue;
                                    }
                                    if tx_dev
                                        .send(PlaybackDeviceMessage::Data(bytes_to_write))
                                        .is_err()
                                    {
                                        status_channel
                                            .send(StatusMessage::PlaybackError(
                                                "Playback inner queue closed".to_string(),
                                            ))
                                            .unwrap_or(());
                                        break;
                                    }
                                }
                                Ok(AudioMessage::Pause) => {
                                    tx_dev.send(PlaybackDeviceMessage::Pause).unwrap_or(());
                                }
                                Ok(AudioMessage::EndOfStream) => {
                                    tx_dev.send(PlaybackDeviceMessage::EndOfStream).unwrap_or(());
                                    break;
                                }
                                Err(err) => {
                                    status_channel
                                        .send(StatusMessage::PlaybackError(err.to_string()))
                                        .unwrap_or(());
                                    tx_dev.send(PlaybackDeviceMessage::EndOfStream).unwrap_or(());
                                    break;
                                }
                            }
                        }
                    }
                    Ok(AlsaThreadState::Error(err)) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err))
                            .unwrap_or(());
                        barrier.wait();
                    }
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(err.to_string()))
                            .unwrap_or(());
                        barrier.wait();
                    }
                }
                innerhandle.join().unwrap_or(());
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let silence_timeout = self.silence_timeout;
        let silence_threshold = self.silence_threshold;
        let conf_sample_format = self.sample_format;
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let stop_on_rate_change = self.stop_on_rate_change;
        let rate_measure_interval = self.rate_measure_interval;
        let stop_on_inactive = self.stop_on_inactive;
        let link_volume_control = self.link_volume_control.clone();
        let link_mute_control = self.link_mute_control.clone();

        let handle = thread::Builder::new()
            .name("AlsaCapture".to_string())
            .spawn(move || {
                let mut resampler = new_resampler(
                    &resampler_config,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                    processing_params.clone(),
                );
                let channel_capacity = if let Some(resamp) = &resampler {
                    let max_input_frames = resamp.resampler.input_frames_max();
                    32 * (chunksize + max_input_frames) / 1024 + 10
                } else {
                    32 * chunksize / 1024 + 10
                };

                let (tx_dev, rx_dev) = crossbeam_channel::bounded(channel_capacity);
                let (tx_inner_command, rx_inner_command) = crossbeam_channel::bounded(32);
                let (tx_state_dev, rx_state_dev) = crossbeam_channel::bounded(0);
                let (tx_start_inner, rx_start_inner) = crossbeam_channel::bounded(0);

                let ringbuffer = HeapRb::<u8>::new(channels * 4 * (2 * chunksize + 2048));
                let (mut device_producer, mut device_consumer) = ringbuffer.split();

                let status_channel_inner = status_channel.clone();
                let capture_status_inner = capture_status.clone();
                let processing_params_inner = processing_params.clone();

                let innerhandle = thread::Builder::new()
                    .name("AlsaCaptureInner".to_string())
                    .spawn(move || {
                        let mut buf_manager = CaptureBufferManager::new(
                            chunksize as Frames,
                            samplerate as f32 / capture_samplerate as f32,
                        );

                        match open_pcm(
                            devname,
                            capture_samplerate as u32,
                            channels as u32,
                            &conf_sample_format,
                            &mut buf_manager,
                            true,
                        ) {
                            Ok((pcmdevice, sample_format)) => {
                                let binary_format = sample_format.to_binary_format();
                                tx_state_dev
                                    .send(AlsaThreadState::Ready(binary_format))
                                    .unwrap_or(());
                                if rx_start_inner.recv().is_err() {
                                    return;
                                }

                                let io = pcmdevice.io_bytes();
                                let store_bytes_per_sample = binary_format.bytes_per_sample();
                                let bytes_per_frame = channels * store_bytes_per_sample;
                                let capture_bytes = chunksize * bytes_per_frame;
                                let capture_frames = chunksize as Frames;
                                let mut buffer = vec![0u8; capture_bytes];
                                let mut chunk_nbr = 0usize;

                                let pcminfo = pcmdevice.info().unwrap();
                                let card = pcminfo.get_card();
                                let device = pcminfo.get_device();
                                let subdevice = pcminfo.get_subdevice();

                                let fds = pcmdevice.get().unwrap();
                                let nbr_pcm_fds = fds.len();
                                let mut file_descriptors = FileDescriptors { fds, nbr_pcm_fds };

                                let mut element_loopback: Option<ElemData> = None;
                                let mut element_uac2_gadget: Option<ElemData> = None;
                                let mut capture_elements = CaptureElements::default();

                                let hctl =
                                    (card >= 0).then(|| HCtl::new(&format!("hw:{card}"), true).unwrap());
                                let ctl =
                                    (card >= 0).then(|| Ctl::new(&format!("hw:{card}"), true).unwrap());

                                if let Some(c) = &ctl {
                                    c.subscribe_events(true).unwrap();
                                }

                                let mut cap_params = CaptureParams {
                                    channels,
                                    sample_format: binary_format,
                                    silence_timeout,
                                    silence_threshold,
                                    chunksize,
                                    store_bytes_per_sample,
                                    bytes_per_frame,
                                    samplerate,
                                    capture_samplerate,
                                    async_src,
                                    capture_status: capture_status_inner,
                                    stop_on_rate_change,
                                    rate_measure_interval,
                                    stop_on_inactive,
                                    link_volume_control,
                                    link_mute_control,
                                    linked_mute_value: None,
                                    linked_volume_value: None,
                                };

                                if let Some(h) = &hctl {
                                    let ctl_fds = h.get().unwrap();
                                    file_descriptors.fds.extend(ctl_fds.iter());
                                    h.load().unwrap();
                                    element_loopback = find_elem(
                                        h,
                                        ElemIface::PCM,
                                        Some(device),
                                        Some(subdevice),
                                        "PCM Rate Shift 100000",
                                    );
                                    element_uac2_gadget = find_elem(
                                        h,
                                        ElemIface::PCM,
                                        Some(device),
                                        Some(subdevice),
                                        "Capture Pitch 1000000",
                                    );

                                    capture_elements.find_elements(
                                        h,
                                        device,
                                        subdevice,
                                        &cap_params.link_volume_control,
                                        &cap_params.link_mute_control,
                                    );
                                    if let Some(c) = &ctl {
                                        if let Some(ref vol_elem) = capture_elements.volume {
                                            let vol_db = vol_elem.read_volume_in_db(c);
                                            if let Some(vol) = vol_db {
                                                cap_params.linked_volume_value = Some(vol);
                                                status_channel_inner
                                                    .send(StatusMessage::SetVolume(vol))
                                                    .unwrap_or_default();
                                            }
                                        }
                                        if let Some(ref mute_elem) = capture_elements.mute {
                                            let active = mute_elem.read_as_bool();
                                            if let Some(active_val) = active {
                                                cap_params.linked_mute_value = Some(!active_val);
                                                status_channel_inner
                                                    .send(StatusMessage::SetMute(!active_val))
                                                    .unwrap_or_default();
                                            }
                                        }
                                    }
                                }

                                let thread_handle = match promote_current_thread_to_real_time(
                                    chunksize as u32,
                                    samplerate as u32,
                                ) {
                                    Ok(h) => Some(h),
                                    Err(err) => {
                                        warn!(
                                            "Capture inner thread could not get real time priority, error: {err}"
                                        );
                                        None
                                    }
                                };

                                loop {
                                    match rx_inner_command.try_recv() {
                                        Ok(CommandMessage::Exit) => {
                                            status_channel_inner
                                                .send(StatusMessage::CaptureDone)
                                                .unwrap_or(());
                                            tx_dev
                                                .send(CaptureDeviceMessage::EndOfStream)
                                                .unwrap_or(());
                                            break;
                                        }
                                        Ok(CommandMessage::SetSpeed { speed }) => {
                                            if let Some(elem_loopback) = &element_loopback {
                                                elem_loopback
                                                    .write_as_int((100_000.0 / speed) as i32);
                                            } else if let Some(elem_uac2_gadget) =
                                                &element_uac2_gadget
                                            {
                                                elem_uac2_gadget
                                                    .write_as_int((speed * 1_000_000.0) as i32);
                                            }
                                        }
                                        Err(crossbeam_channel::TryRecvError::Empty) => {}
                                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                            tx_dev
                                                .send(CaptureDeviceMessage::EndOfStream)
                                                .unwrap_or(());
                                            break;
                                        }
                                    }

                                    let capture_res = capture_buffer(
                                        &mut buffer,
                                        &pcmdevice,
                                        &io,
                                        capture_frames as usize,
                                        &mut file_descriptors,
                                        &ctl,
                                        &hctl,
                                        &capture_elements,
                                        &status_channel_inner,
                                        &mut cap_params,
                                        &processing_params_inner,
                                    );

                                    match capture_res {
                                        Ok((CaptureResult::Normal, bytes_read)) => {
                                            let pushed_bytes =
                                                device_producer.push_slice(&buffer[0..bytes_read]);
                                            if pushed_bytes < bytes_read {
                                                warn!(
                                                    "Capture ring buffer is full, dropped {} out of {} bytes",
                                                    bytes_read - pushed_bytes,
                                                    bytes_read
                                                );
                                            }
                                            tx_dev
                                                .try_send(CaptureDeviceMessage::Data {
                                                    chunk_nbr,
                                                    nbr_bytes: pushed_bytes,
                                                })
                                                .unwrap_or_default();
                                        }
                                        Ok((CaptureResult::Stalled, _)) => {
                                            debug!("Capture device stalled, no data received");
                                            tx_dev
                                                .try_send(CaptureDeviceMessage::Data {
                                                    chunk_nbr,
                                                    nbr_bytes: 0,
                                                })
                                                .unwrap_or_default();
                                        }
                                        Ok((CaptureResult::Done, _)) => {
                                            status_channel_inner
                                                .send(StatusMessage::CaptureDone)
                                                .unwrap_or(());
                                            tx_dev
                                                .send(CaptureDeviceMessage::EndOfStream)
                                                .unwrap_or(());
                                            break;
                                        }
                                        Err(msg) => {
                                            status_channel_inner
                                                .send(StatusMessage::CaptureError(
                                                    msg.to_string(),
                                                ))
                                                .unwrap_or(());
                                            tx_dev
                                                .send(CaptureDeviceMessage::EndOfStream)
                                                .unwrap_or(());
                                            break;
                                        }
                                    }

                                    sync_linked_controls(
                                        &processing_params_inner,
                                        &mut cap_params,
                                        &mut capture_elements,
                                        &ctl,
                                    );
                                    chunk_nbr += 1;
                                }

                                if let Some(h) = thread_handle {
                                    demote_current_thread_from_real_time(h).unwrap_or_default();
                                }
                                cap_params.capture_status.write().state = ProcessingState::Inactive;
                            }
                            Err(err) => {
                                tx_state_dev
                                    .send(AlsaThreadState::Error(err.to_string()))
                                    .unwrap_or(());
                            }
                        }
                    })
                    .unwrap();

                match rx_state_dev.recv() {
                    Ok(AlsaThreadState::Ready(binary_format)) => {
                        let bytes_per_sample = binary_format.bytes_per_sample();
                        let blockalign = bytes_per_sample * channels;

                        let mut capture_frames = chunksize;
                        let mut averager = countertimer::TimeAverage::new();
                        let mut watcher_averager = countertimer::TimeAverage::new();
                        let mut valuewatcher = countertimer::ValueWatcher::new(
                            capture_samplerate as f32,
                            RATE_CHANGE_THRESHOLD_VALUE,
                            RATE_CHANGE_THRESHOLD_COUNT,
                        );
                        let mut value_range = 0.0;
                        let mut chunk_stats = ChunkStats {
                            rms: vec![0.0; channels],
                            peak: vec![0.0; channels],
                        };
                        let mut rate_adjust = 0.0;
                        let mut silence_counter = countertimer::SilenceCounter::new(
                            silence_threshold,
                            silence_timeout,
                            capture_samplerate,
                            chunksize,
                        );
                        let mut state = ProcessingState::Running;
                        let mut saved_state = state;
                        let mut device_stalled = false;
                        let mut data_buffer = vec![0u8; 4 * blockalign * capture_frames];
                        let mut expected_chunk_nbr = 0usize;

                        status_channel.send(StatusMessage::CaptureReady).unwrap_or(());
                        barrier.wait();
                        tx_start_inner.send(()).unwrap_or(());

                        let thread_handle = match promote_current_thread_to_real_time(0, 1) {
                            Ok(h) => Some(h),
                            Err(err) => {
                                warn!(
                                    "Capture outer thread could not get real time priority, error: {err}"
                                );
                                None
                            }
                        };

                        'outer: loop {
                            match command_channel.try_recv() {
                                Ok(CommandMessage::Exit) => {
                                    tx_inner_command.send(CommandMessage::Exit).unwrap_or(());
                                }
                                Ok(CommandMessage::SetSpeed { speed }) => {
                                    rate_adjust = speed;
                                    if let Some(resampl) = &mut resampler {
                                        if async_src {
                                            if resampl
                                                .resampler
                                                .set_resample_ratio_relative(speed, true)
                                                .is_err()
                                            {
                                                debug!("Failed to set resampling speed to {speed}");
                                            }
                                        } else {
                                            warn!(
                                                "Requested rate adjust of synchronous resampler. Ignoring request."
                                            );
                                        }
                                    }
                                    tx_inner_command
                                        .try_send(CommandMessage::SetSpeed { speed })
                                        .unwrap_or_default();
                                }
                                Err(crossbeam_channel::TryRecvError::Empty) => {}
                                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                                    tx_inner_command.send(CommandMessage::Exit).unwrap_or(());
                                }
                            }

                            capture_frames = nbr_capture_frames(&resampler, capture_frames);
                            let capture_bytes = blockalign * capture_frames;
                            if data_buffer.len() < capture_bytes {
                                data_buffer.resize(capture_bytes, 0);
                            }

                            while device_consumer.occupied_len() < capture_bytes {
                                match rx_dev.recv() {
                                    Ok(CaptureDeviceMessage::Data {
                                        chunk_nbr,
                                        nbr_bytes,
                                    }) => {
                                        if chunk_nbr > expected_chunk_nbr {
                                            warn!(
                                                "Capture, samples were dropped, missing {} buffers.",
                                                chunk_nbr - expected_chunk_nbr
                                            );
                                        }
                                        expected_chunk_nbr = chunk_nbr + 1;
                                        if nbr_bytes == 0 {
                                            if !device_stalled {
                                                saved_state = state;
                                            }
                                            device_stalled = true;
                                            break;
                                        }
                                        if device_stalled {
                                            device_stalled = false;
                                            state = saved_state;
                                        }
                                    }
                                    Ok(CaptureDeviceMessage::EndOfStream) => {
                                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                        break 'outer;
                                    }
                                    Err(err) => {
                                        channel.send(AudioMessage::EndOfStream).unwrap_or(());
                                        status_channel
                                            .send(StatusMessage::CaptureError(err.to_string()))
                                            .unwrap_or(());
                                        break 'outer;
                                    }
                                }
                            }

                            // Drain any remaining messages in the rx_dev channel
                            while let Ok(CaptureDeviceMessage::Data {
                                chunk_nbr,
                                nbr_bytes: _,
                            }) = rx_dev.try_recv()
                            {
                                expected_chunk_nbr = chunk_nbr + 1;
                            }

                            if device_stalled {
                                state = ProcessingState::Stalled;
                                if !send_capture_audio(&channel, AudioMessage::Pause) {
                                    break;
                                }
                                continue;
                            }

                            device_consumer.pop_slice(&mut data_buffer[0..capture_bytes]);

                            averager.add_value(capture_bytes);
                            if let Some(capture_status_guard) = capture_status.try_upgradable_read() {
                                if averager.larger_than_millis(capture_status_guard.update_interval as u64)
                                {
                                    let bytes_per_sec = averager.average();
                                    averager.restart();
                                    let measured_rate_f =
                                        bytes_per_sec / (channels * bytes_per_sample) as f64;
                                    if let Ok(mut capture_status) =
                                        RwLockUpgradableReadGuard::try_upgrade(capture_status_guard)
                                    {
                                        capture_status.measured_samplerate = measured_rate_f as usize;
                                        capture_status.signal_range = value_range as f32;
                                        capture_status.rate_adjust = rate_adjust as f32;
                                        capture_status.state = state;
                                    }
                                }
                            }

                            watcher_averager.add_value(capture_bytes);
                            let rate_measure_interval_ms = (1000.0 * rate_measure_interval) as u64;
                            if watcher_averager.larger_than_millis(rate_measure_interval_ms) {
                                let bytes_per_sec = watcher_averager.average();
                                watcher_averager.restart();
                                let measured_rate_f =
                                    bytes_per_sec / (channels * bytes_per_sample) as f64;
                                let changed = valuewatcher.check_value(measured_rate_f as f32);
                                if changed && stop_on_rate_change {
                                    let _ = send_capture_audio(&channel, AudioMessage::EndOfStream);
                                    status_channel
                                        .send(StatusMessage::CaptureFormatChange(
                                            measured_rate_f as usize,
                                        ))
                                        .unwrap_or(());
                                    break;
                                }
                            }

                            let mut chunk = buffer_to_chunk_rawbytes(
                                &data_buffer[0..capture_bytes],
                                channels,
                                &binary_format,
                                capture_bytes,
                                &capture_status.read().used_channels,
                                false,
                            );

                            chunk.update_stats(&mut chunk_stats);
                            if let Some(mut capture_status_write) = capture_status.try_write() {
                                capture_status_write
                                    .signal_rms
                                    .add_record_squared(chunk_stats.rms_linear());
                                capture_status_write
                                    .signal_peak
                                    .add_record(chunk_stats.peak_linear());
                            }

                            value_range = chunk.maxval - chunk.minval;
                            state = silence_counter.update(value_range);

                            if state == ProcessingState::Running {
                                if let Some(resampl) = &mut resampler {
                                    resampl.resample_chunk(&mut chunk, chunksize, channels);
                                }
                                if !send_capture_audio(&channel, AudioMessage::Audio(chunk)) {
                                    break;
                                }
                            } else if !send_capture_audio(&channel, AudioMessage::Pause) {
                                break;
                            }
                        }

                        if let Some(h) = thread_handle {
                            demote_current_thread_from_real_time(h).unwrap_or_default();
                        }
                        capture_status.write().state = ProcessingState::Inactive;
                    }
                    Ok(AlsaThreadState::Error(err)) => {
                        status_channel
                            .send(StatusMessage::CaptureError(err))
                            .unwrap_or(());
                        barrier.wait();
                    }
                    Err(err) => {
                        status_channel
                            .send(StatusMessage::CaptureError(err.to_string()))
                            .unwrap_or(());
                        barrier.wait();
                    }
                }
                innerhandle.join().unwrap_or(());
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
