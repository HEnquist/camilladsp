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

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use parking_lot::RwLock;

use crate::audiochunk::{AudioChunk, ChunkStats};
use crate::audiodevice::*;
use crate::config;
use crate::dummy_backend::control::{ControlListener, DummyControl};
use crate::dummy_backend::pacer::Pacer;
use crate::generatordevice::SignalSource;
use crate::utils::countertimer;
use crate::utils::rate_controller::PIRateController;
use crate::utils::stash::recycle_chunk;
use crate::{CamillaFloat, ToCamillaFloat};

use crate::CaptureStatus;
use crate::CommandMessage;
use crate::PlaybackStatus;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;

/// How far a dummy device is allowed to fall behind the clock before it gives up on
/// catching up, expressed in chunks. See `Pacer::resync_if_behind`.
const MAX_DEFICIT_CHUNKS: usize = 8;

/// The rate a device runs at once its clock is taken off nominal.
fn drifted_rate(samplerate: usize, drift_ppm: i32) -> f64 {
    samplerate as f64 * (1.0 + f64::from(drift_ppm) / 1.0e6)
}

/// Convert a measured value into the processing precision.
fn camilla_float(value: f32) -> CamillaFloat {
    f64::from(value).to_camilla_float()
}

pub struct DummyCaptureDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub signal: config::Signal,
    pub silence_threshold: f64,
    pub silence_timeout: f64,
    pub control_port: Option<u16>,
}

pub struct DummyPlaybackDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub target_level: usize,
    pub adjust_period: f32,
    pub enable_rate_adjust: bool,
    pub control_port: Option<u16>,
}

struct CaptureChannels {
    audio: crossbeam_channel::Sender<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
    command: crossbeam_channel::Receiver<CommandMessage>,
}

struct CaptureParams {
    channels: usize,
    chunksize: usize,
    samplerate: usize,
    signal: config::Signal,
    silence_threshold: f64,
    silence_timeout: f64,
    capture_status: Arc<RwLock<CaptureStatus>>,
    control: Arc<DummyControl>,
}

struct PlaybackParams {
    channels: usize,
    chunksize: usize,
    samplerate: usize,
    target_level: usize,
    adjust_period: f32,
    enable_rate_adjust: bool,
    playback_status: Arc<RwLock<PlaybackStatus>>,
    control: Arc<DummyControl>,
}

fn capture_loop(params: CaptureParams, msg_channels: CaptureChannels) {
    debug!("starting dummy capture loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut rms_values = Vec::new();
    let mut peak_values = Vec::new();
    let mut generator = SignalSource::new(&params.signal, params.samplerate);
    let mut pacer = Pacer::new(params.samplerate);
    let mut averager = countertimer::TimeAverage::new();
    let mut silence_counter = countertimer::SilenceCounter::new(
        params.silence_threshold,
        params.silence_timeout,
        params.samplerate,
        params.chunksize,
    );
    let max_deficit = (MAX_DEFICIT_CHUNKS * params.chunksize) as f64;
    let chunk_duration =
        Duration::from_secs_f64(params.chunksize as f64 / params.samplerate as f64);
    let mut state = ProcessingState::Running;
    let mut drift_ppm = 0;
    // Stays at zero until the first SetSpeed arrives, the same as the file backend, so a
    // run without rate adjust reports no adjustment rather than a nominal one.
    let mut rate_adjust = 0.0;

    crate::set_capture_state(&params.capture_status, state);
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
            Ok(CommandMessage::SetSpeed { speed: new_speed }) => {
                // There is no resampler here, so the device does what a clock-slave
                // device does and runs its own clock faster or slower. That is the same
                // shape as the ALSA UAC2 gadget path, `src/alsa_backend/device.rs:671`.
                trace!("Dummy capture setting speed to {new_speed}");
                rate_adjust = new_speed;
                pacer.set_rate(rate_adjust * drifted_rate(params.samplerate, drift_ppm));
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                error!("Command channel was closed");
                break;
            }
        };

        if params.control.stalled() {
            // A stalled device hands over nothing at all. Keep the pacer anchored to the
            // current time while that lasts, or the deficit built up during the stall
            // comes back as a burst of chunks at full speed once it clears.
            pacer.resync();
            if state != ProcessingState::Stalled {
                debug!("Dummy capture is stalled");
                state = ProcessingState::Stalled;
                crate::set_capture_state(&params.capture_status, state);
            }
            params.control.count_pause();
            if msg_channels.audio.send(AudioMessage::Pause).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
            thread::sleep(chunk_duration);
            continue;
        }

        let requested_drift = params.control.drift_ppm();
        if requested_drift != drift_ppm {
            drift_ppm = requested_drift;
            debug!("Dummy capture clock set to {drift_ppm} ppm off nominal");
            let speed = if rate_adjust > 0.0 { rate_adjust } else { 1.0 };
            pacer.set_rate(speed * drifted_rate(params.samplerate, drift_ppm));
        }

        // A real device hands over a chunk once it has captured every frame in it,
        // so wait until the whole chunk is due before generating it.
        pacer.advance(params.chunksize);
        pacer.wait_for_backlog_below(0.0);
        if pacer.resync_if_behind(max_deficit) {
            params.control.count_resync();
            warn!("Dummy capture fell behind and dropped the backlog, as an overrun would");
        }

        let mut waveforms = generator.waveforms(params.channels, params.chunksize);
        if params.control.silenced() {
            // Zero the samples rather than skipping generation, so the phase carries on
            // where it left off when the signal comes back.
            for waveform in waveforms.iter_mut() {
                waveform.fill(0.0);
            }
        }
        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, params.chunksize, params.chunksize);
        params.control.add_frames(params.chunksize);

        chunk.update_stats(&mut chunk_stats);
        crate::push_capture_audio_buffer(&params.capture_status, &chunk);
        crate::update_capture_signal_status(
            &params.capture_status,
            &chunk_stats,
            &mut rms_values,
            &mut peak_values,
        );
        // The chunk is generated with a nominal range of +/- 1.0, so the peak is what
        // says whether there is a signal in it.
        let peak = chunk_stats.peak.iter().copied().fold(0.0f32, f32::max);
        let value_range = 2.0 * peak;

        averager.add_value(params.chunksize);
        if let Some(mut capture_status) = params.capture_status.try_write()
            && averager.larger_than_millis(capture_status.update_interval as u64)
        {
            // The pacer keeps its frame position tied to the clock, so this is a real
            // measurement rather than the nominal rate read back.
            let measured_rate = averager.average();
            averager.restart();
            trace!("Measured sample rate is {measured_rate:.1} Hz");
            capture_status.measured_samplerate = measured_rate as usize;
            capture_status.signal_range = value_range;
            capture_status.rate_adjust = rate_adjust as f32;
        }

        let silence_state = silence_counter.update(camilla_float(value_range));
        if silence_state != state {
            state = silence_state;
            crate::set_capture_state(&params.capture_status, state);
        }
        if state != ProcessingState::Running {
            // Paused, so the chunk is not sent. Its buffers go back to the stash the way
            // the playback device returns them, instead of being dropped.
            recycle_chunk(chunk);
            params.control.count_pause();
            if msg_channels.audio.send(AudioMessage::Pause).is_err() {
                info!("Processing thread has already stopped.");
                break;
            }
            continue;
        }

        let msg = AudioMessage::Audio(chunk);
        if msg_channels.audio.send(msg).is_err() {
            info!("Processing thread has already stopped.");
            break;
        }
    }
    crate::set_capture_state(&params.capture_status, ProcessingState::Inactive);
}

fn playback_loop(
    params: PlaybackParams,
    channel: crossbeam_channel::Receiver<AudioMessage>,
    status_channel: &crossbeam_channel::Sender<StatusMessage>,
) -> Option<String> {
    debug!("starting dummy playback loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut rms_values = Vec::new();
    let mut peak_values = Vec::new();
    let max_deficit = (MAX_DEFICIT_CHUNKS * params.chunksize) as f64;
    // The target level is where the buffer should sit, not how big it is, so the device
    // has room above it. Without that headroom the write below blocks as soon as the
    // level reaches the target, the excess piles up in the queue until the capture
    // blocks too, and the rate control loop has nothing left to control: the buffer is
    // always exactly full whatever the capture clock does. Putting the target in the
    // middle of the buffer is what a real device is configured to do.
    let buffer_size = (2 * params.target_level) as f64;
    // A real device does not start draining until it is started, which happens once
    // `target_level` frames have been written to it. Until then the frames only pile up
    // in the buffer, so there is no pacer to run against.
    let mut pacer: Option<Pacer> = None;
    let mut prefilled = 0;
    let mut drift_ppm = 0;
    let mut rate_controller = PIRateController::new_with_default_gains(
        params.samplerate,
        f64::from(params.adjust_period),
        params.target_level,
    );
    let mut timer = countertimer::Stopwatch::new();
    let mut buffer_avg = countertimer::Averager::new();

    loop {
        match channel.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                params.control.add_frames(chunk.frames);
                if params.control.stalled() {
                    // A stalled device keeps taking chunks and throws them away, the way
                    // a real one does once it has been reset, so the queue does not back
                    // up behind it. Its buffer level and signal levels say nothing while
                    // that lasts, so they are left alone, as at
                    // `src/alsa_backend/device.rs:650`.
                    if let Some(pacer) = &mut pacer {
                        pacer.resync();
                    }
                    recycle_chunk(chunk);
                    continue;
                }
                chunk.update_stats(&mut chunk_stats);
                crate::push_playback_audio_buffer(&params.playback_status, &chunk);
                crate::update_playback_signal_status(
                    &params.playback_status,
                    &chunk_stats,
                    &mut rms_values,
                    &mut peak_values,
                    0,
                );
                // What is waiting behind this chunk is as much a part of the delay
                // as what is already in the buffer, so it counts towards the level, as
                // at `src/alsa_backend/device.rs:662`. Without it the level is blind to
                // a capture running fast, since the wait below pins the buffer itself at
                // the target and the excess piles up in the queue instead.
                let queued = (params.chunksize * channel.len()) as f64;
                // The chunk goes into the virtual buffer, and its frames are gone
                // once it has drained.
                let buffer_level = match &mut pacer {
                    Some(pacer) => {
                        // Measure as the chunk arrives, before it is written. Measured
                        // after the wait below, the reading would only ever be the level
                        // that wait stops at, whatever the device is really doing.
                        let level = pacer.backlog().max(0.0) + queued;
                        // Block until there is room in the buffer, which is what a
                        // real device does when its own is full.
                        pacer.advance(chunk.frames);
                        pacer.wait_for_backlog_below(buffer_size);
                        if pacer.resync_if_behind(max_deficit) {
                            params.control.count_resync();
                            warn!(
                                "Dummy playback fell behind and dropped the backlog, as an underrun would"
                            );
                        }
                        level
                    }
                    None => {
                        prefilled += chunk.frames;
                        if prefilled >= params.target_level {
                            let mut started = Pacer::new(params.samplerate);
                            started.advance(prefilled);
                            started.set_rate(drifted_rate(params.samplerate, drift_ppm));
                            pacer = Some(started);
                        }
                        prefilled as f64
                    }
                };
                // Published every chunk rather than once per adjust period, so a test
                // polling the getter does not have to wait one out. The controller below
                // uses the average over the period, the way the real backends do.
                if let Some(mut playback_status) = params.playback_status.try_write() {
                    playback_status.buffer_level = buffer_level as usize;
                } else {
                    xtrace!("playback status blocked, skip buffer level update");
                }
                buffer_avg.add_value(buffer_level);
                if timer.larger_than_millis((1000.0 * params.adjust_period) as u64)
                    && let Some(avg_level) = buffer_avg.average()
                {
                    timer.restart();
                    buffer_avg.restart();
                    if params.enable_rate_adjust {
                        let capture_speed = rate_controller.next(avg_level);
                        debug!("PB: buffer level {avg_level:.1}, SetSpeed {capture_speed}");
                        status_channel
                            .send(StatusMessage::SetSpeed(capture_speed))
                            .unwrap_or(());
                    }
                }
                let requested_drift = params.control.drift_ppm();
                if requested_drift != drift_ppm {
                    drift_ppm = requested_drift;
                    debug!("Dummy playback clock set to {drift_ppm} ppm off nominal");
                    if let Some(pacer) = &mut pacer {
                        pacer.set_rate(drifted_rate(params.samplerate, drift_ppm));
                    }
                }
                // The buffers themselves go back to the stash, the same way a real
                // playback device returns them after converting a chunk. Without this
                // the capture side allocates a new set for every chunk.
                recycle_chunk(chunk);
            }
            Ok(AudioMessage::Pause) => {
                params.control.count_pause();
                trace!("Pause message received");
            }
            Ok(AudioMessage::EndOfStream) => {
                break None;
            }
            Err(err) => {
                error!("Message channel error: {err}");
                break Some(err.to_string());
            }
        }
    }
}

impl CaptureDevice for DummyCaptureDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Sender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: crossbeam_channel::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        _processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let signal = self.signal;
        let silence_threshold = self.silence_threshold;
        let silence_timeout = self.silence_timeout;
        let control_port = self.control_port;

        let handle = thread::Builder::new()
            .name("DummyCapture".to_string())
            .spawn(move || {
                // The listener lives for as long as the device does, and releases the
                // port on every way out of the loop below.
                let listener = ControlListener::start(control_port, "capture");
                let params = CaptureParams {
                    channels,
                    chunksize,
                    samplerate,
                    signal,
                    silence_threshold,
                    silence_timeout,
                    capture_status,
                    control: listener.control(),
                };
                status_channel
                    .send(StatusMessage::CaptureReady)
                    .unwrap_or(());
                barrier.wait();
                let msg_channels = CaptureChannels {
                    audio: channel,
                    status: status_channel,
                    command: command_channel,
                };
                capture_loop(params, msg_channels);
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}

impl PlaybackDevice for DummyPlaybackDevice {
    fn start(
        &mut self,
        channel: crossbeam_channel::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let samplerate = self.samplerate;
        let chunksize = self.chunksize;
        let channels = self.channels;
        let target_level = self.target_level;
        let adjust_period = self.adjust_period;
        let enable_rate_adjust = self.enable_rate_adjust;
        let control_port = self.control_port;

        let handle = thread::Builder::new()
            .name("DummyPlayback".to_string())
            .spawn(move || {
                // The listener lives for as long as the device does, and releases the
                // port on every way out of the loop below.
                let listener = ControlListener::start(control_port, "playback");
                let params = PlaybackParams {
                    channels,
                    chunksize,
                    samplerate,
                    target_level,
                    adjust_period,
                    enable_rate_adjust,
                    playback_status,
                    control: listener.control(),
                };
                status_channel
                    .send(StatusMessage::PlaybackReady)
                    .unwrap_or(());
                barrier.wait();
                match playback_loop(params, channel, &status_channel) {
                    Some(msg) => {
                        status_channel
                            .send(StatusMessage::PlaybackError(msg))
                            .unwrap_or(());
                    }
                    None => {
                        status_channel
                            .send(StatusMessage::PlaybackDone)
                            .unwrap_or(());
                    }
                }
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
