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
use crate::utils::conversions::chunk_to_buffer_rawbytes;
use crate::utils::countertimer;
use crate::utils::rate_controller::PIRateController;
use crate::utils::resampling::{ChunkResampler, new_resampler, resampler_is_async};
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

/// The rate the capture device's own clock should run at.
///
/// A drift always moves the device clock. A rate adjust only does when there is no
/// resampler, since with one it is the resample ratio that changes and the device clock
/// stays where it is, which is the same split the real backends make.
fn capture_clock_rate(
    samplerate: usize,
    drift_ppm: i32,
    rate_adjust: f64,
    resampling: bool,
) -> f64 {
    let speed = if resampling || rate_adjust <= 0.0 {
        1.0
    } else {
        rate_adjust
    };
    speed * drifted_rate(samplerate, drift_ppm)
}

/// Convert a measured value into the processing precision.
fn camilla_float(value: f32) -> CamillaFloat {
    f64::from(value).to_camilla_float()
}

pub struct DummyCaptureDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub capture_samplerate: usize,
    pub resampler_config: Option<config::Resampler>,
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
    pub sample_format: Option<config::BinarySampleFormat>,
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
    capture_samplerate: usize,
    async_src: bool,
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
    sample_format: Option<config::BinarySampleFormat>,
    target_level: usize,
    adjust_period: f32,
    enable_rate_adjust: bool,
    playback_status: Arc<RwLock<PlaybackStatus>>,
    control: Arc<DummyControl>,
}

fn capture_loop(
    params: CaptureParams,
    msg_channels: CaptureChannels,
    mut resampler: Option<ChunkResampler>,
) {
    debug!("starting dummy capture loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut rms_values = Vec::new();
    let mut peak_values = Vec::new();
    // Everything on the device side of the resampler runs at the capture rate: the signal
    // is generated at it, so the tone keeps its frequency in Hz, and the pacer counts the
    // frames the device really produces rather than the ones it hands on.
    let mut generator = SignalSource::new(&params.signal, params.capture_samplerate);
    let mut pacer = Pacer::new(params.capture_samplerate);
    let mut averager = countertimer::TimeAverage::new();
    let mut silence_counter = countertimer::SilenceCounter::new(
        params.silence_threshold,
        params.silence_timeout,
        params.capture_samplerate,
        params.chunksize,
    );
    // One chunk of frames measured on the capture side of the resampler, which is what the
    // pacer counts, so the deficit limit means the same eight chunks of time either way.
    let capture_chunk_frames =
        params.chunksize as f64 * params.capture_samplerate as f64 / params.samplerate as f64;
    let max_deficit = MAX_DEFICIT_CHUNKS as f64 * capture_chunk_frames;
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
                trace!("Dummy capture setting speed to {new_speed}");
                rate_adjust = new_speed;
                match &mut resampler {
                    Some(resampl) => {
                        if params.async_src {
                            // The ratio is what changes, exactly as in the file backend
                            // at `src/file_backend/device.rs:441`.
                            if resampl
                                .set_resample_ratio_relative(new_speed, true)
                                .is_err()
                            {
                                debug!("Failed to set resampling speed to {new_speed}");
                            }
                        } else {
                            warn!(
                                "Requested rate adjust of synchronous resampler. Ignoring request."
                            );
                        }
                    }
                    // With no resampler the device does what a clock-slave device does and
                    // runs its own clock faster or slower. That is the same shape as the
                    // ALSA UAC2 gadget path, `src/alsa_backend/device.rs:671`.
                    None => pacer.set_rate(capture_clock_rate(
                        params.capture_samplerate,
                        drift_ppm,
                        rate_adjust,
                        false,
                    )),
                }
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
            pacer.set_rate(capture_clock_rate(
                params.capture_samplerate,
                drift_ppm,
                rate_adjust,
                resampler.is_some(),
            ));
        }

        // A resampler asks for however many frames it needs to fill one output chunk,
        // and that count moves with the ratio, so it is read per iteration the way
        // `nbr_capture_bytes` does in the file backend.
        let capture_frames = match &resampler {
            Some(resampl) => resampl.resampler.input_frames_next(),
            None => params.chunksize,
        };
        // A real device hands over a chunk once it has captured every frame in it,
        // so wait until the whole chunk is due before generating it.
        pacer.advance(capture_frames);
        pacer.wait_for_backlog_below(0.0);
        if pacer.resync_if_behind(max_deficit) {
            params.control.count_resync();
            warn!("Dummy capture fell behind and dropped the backlog, as an overrun would");
        }

        let mut waveforms = generator.waveforms(params.channels, capture_frames);
        if params.control.silenced() {
            // Zero the samples rather than skipping generation, so the phase carries on
            // where it left off when the signal comes back.
            for waveform in waveforms.iter_mut() {
                waveform.fill(0.0);
            }
        }
        let mut chunk = AudioChunk::new(waveforms, 1.0, -1.0, capture_frames, capture_frames);
        params.control.add_frames(capture_frames);

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

        // Counted on the capture side too, so the measured rate is the rate the device
        // is really running at and not the rate it feeds the pipeline at.
        averager.add_value(capture_frames);
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

        if let Some(resampl) = &mut resampler {
            resampl.resample_chunk(&mut chunk, params.chunksize, params.channels);
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
    // A real device converts every chunk to the format the hardware wants on its way out,
    // which is where clipping happens. Without a format configured the audio is dropped as
    // it arrives, which is what the rest of the suite wants and is one copy cheaper.
    let mut convert_buffer = params
        .sample_format
        .map(|format| vec![0u8; params.chunksize * params.channels * format.bytes_per_sample()]);

    loop {
        match channel.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                let frames = chunk.frames;
                params.control.add_frames(frames);
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
                // The conversion consumes the chunk and returns its buffers to the stash,
                // the same way a real playback device does. Without a format there is
                // nothing to convert, so the chunk is handed back here instead: either way
                // the capture side reuses these buffers rather than allocating a fresh set
                // for every chunk.
                let nbr_clipped = match (&mut convert_buffer, &params.sample_format) {
                    (Some(buffer), Some(format)) => {
                        let (_bytes, clipped) = chunk_to_buffer_rawbytes(chunk, buffer, format);
                        clipped
                    }
                    _ => {
                        recycle_chunk(chunk);
                        0
                    }
                };
                crate::update_playback_signal_status(
                    &params.playback_status,
                    &chunk_stats,
                    &mut rms_values,
                    &mut peak_values,
                    nbr_clipped,
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
                        pacer.advance(frames);
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
                        prefilled += frames;
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
        processing_params: Arc<ProcessingParameters>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let samplerate = self.samplerate;
        let capture_samplerate = self.capture_samplerate;
        let resampler_config = self.resampler_config;
        let async_src = resampler_is_async(&resampler_config);
        let chunksize = self.chunksize;
        let channels = self.channels;
        let signal = self.signal;
        let silence_threshold = self.silence_threshold;
        let silence_timeout = self.silence_timeout;
        let control_port = self.control_port;

        let handle = thread::Builder::new()
            .name("DummyCapture".to_string())
            .spawn(move || {
                // Built here rather than in `start`, so the resampler lives on the thread
                // that uses it, as in the other backends.
                let resampler = new_resampler(
                    &resampler_config,
                    channels,
                    samplerate,
                    capture_samplerate,
                    chunksize,
                    processing_params,
                );
                // The listener lives for as long as the device does, and releases the
                // port on every way out of the loop below.
                let listener = ControlListener::start(control_port, "capture");
                let params = CaptureParams {
                    channels,
                    chunksize,
                    samplerate,
                    capture_samplerate,
                    async_src,
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
                capture_loop(params, msg_channels, resampler);
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
        let sample_format = self.sample_format;
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
                    sample_format,
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

#[cfg(test)]
mod tests {
    use super::capture_clock_rate;

    fn assert_rate(rate: f64, expected: f64) {
        assert!(
            (rate - expected).abs() < 1e-6,
            "rate was {rate}, expected {expected}"
        );
    }

    #[test]
    fn a_drift_moves_the_device_clock() {
        assert_rate(capture_clock_rate(48000, 0, 0.0, false), 48000.0);
        assert_rate(capture_clock_rate(48000, 1000, 0.0, false), 48048.0);
        // Also with a resampler, since the drift is the device's own clock running off
        // nominal and not a request made of it.
        assert_rate(capture_clock_rate(48000, 1000, 1.001, true), 48048.0);
    }

    #[test]
    fn a_rate_adjust_moves_the_clock_only_without_a_resampler() {
        // No resampler, so the device is its own clock slave and follows the request.
        assert_rate(capture_clock_rate(48000, 0, 1.001, false), 48048.0);
        // With one, the resample ratio absorbs the request and the clock stays put.
        assert_rate(capture_clock_rate(48000, 0, 1.001, true), 48000.0);
    }

    #[test]
    fn no_rate_adjust_yet_is_not_a_stopped_clock() {
        // The adjust reads as exactly zero until the first SetSpeed arrives, which must
        // not be taken for a request to stop the device.
        assert_rate(capture_clock_rate(48000, 0, 0.0, false), 48000.0);
    }
}
