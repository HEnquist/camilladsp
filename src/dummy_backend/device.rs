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

use parking_lot::RwLock;

use crate::audiochunk::{AudioChunk, ChunkStats};
use crate::audiodevice::*;
use crate::config;
use crate::dummy_backend::control::{ControlListener, DummyControl};
use crate::dummy_backend::pacer::Pacer;
use crate::generatordevice::SignalSource;
use crate::utils::countertimer;
use crate::utils::stash::recycle_chunk;

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

pub struct DummyCaptureDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub signal: config::Signal,
    pub control_port: Option<u16>,
}

pub struct DummyPlaybackDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub target_level: usize,
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
    capture_status: Arc<RwLock<CaptureStatus>>,
    control: Arc<DummyControl>,
}

struct PlaybackParams {
    channels: usize,
    chunksize: usize,
    samplerate: usize,
    target_level: usize,
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
    let max_deficit = (MAX_DEFICIT_CHUNKS * params.chunksize) as f64;

    crate::set_capture_state(&params.capture_status, ProcessingState::Running);
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
            Ok(CommandMessage::SetSpeed { .. }) => {
                // Round one has no resampler and no drift, so there is nothing to adjust.
                warn!("Dummy capture device does not support rate adjust. Ignoring request.");
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                error!("Command channel was closed");
                break;
            }
        };

        // A real device hands over a chunk once it has captured every frame in it,
        // so wait until the whole chunk is due before generating it.
        pacer.advance(params.chunksize);
        pacer.wait_for_backlog_below(0.0);
        if pacer.resync_if_behind(max_deficit) {
            params.control.count_resync();
            warn!("Dummy capture fell behind and dropped the backlog, as an overrun would");
        }

        let waveforms = generator.waveforms(params.channels, params.chunksize);
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
            let peak = chunk_stats.peak.iter().copied().fold(0.0f32, f32::max);
            capture_status.signal_range = 2.0 * peak;
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
) -> Option<String> {
    debug!("starting dummy playback loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut rms_values = Vec::new();
    let mut peak_values = Vec::new();
    let max_deficit = (MAX_DEFICIT_CHUNKS * params.chunksize) as f64;
    // A real device does not start draining until it is started, which happens once
    // `target_level` frames have been written to it. Until then the frames only pile up
    // in the buffer, so there is no pacer to run against.
    let mut pacer: Option<Pacer> = None;
    let mut prefilled = 0;

    loop {
        match channel.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                params.control.add_frames(chunk.frames);
                chunk.update_stats(&mut chunk_stats);
                crate::push_playback_audio_buffer(&params.playback_status, &chunk);
                crate::update_playback_signal_status(
                    &params.playback_status,
                    &chunk_stats,
                    &mut rms_values,
                    &mut peak_values,
                    0,
                );
                // The chunk goes into the virtual buffer, and its frames are gone
                // once it has drained.
                let buffer_level = match &mut pacer {
                    Some(pacer) => {
                        // Block until the buffer has drained back to the target level,
                        // which is what a real device does when its buffer is full.
                        pacer.advance(chunk.frames);
                        pacer.wait_for_backlog_below(params.target_level as f64);
                        if pacer.resync_if_behind(max_deficit) {
                            params.control.count_resync();
                            warn!(
                                "Dummy playback fell behind and dropped the backlog, as an underrun would"
                            );
                        }
                        pacer.backlog().max(0.0) as usize
                    }
                    None => {
                        prefilled += chunk.frames;
                        if prefilled >= params.target_level {
                            let mut started = Pacer::new(params.samplerate);
                            started.advance(prefilled);
                            pacer = Some(started);
                        }
                        prefilled
                    }
                };
                if let Some(mut playback_status) = params.playback_status.try_write() {
                    playback_status.buffer_level = buffer_level;
                } else {
                    xtrace!("playback status blocked, skip buffer level update");
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
                    playback_status,
                    control: listener.control(),
                };
                status_channel
                    .send(StatusMessage::PlaybackReady)
                    .unwrap_or(());
                barrier.wait();
                match playback_loop(params, channel) {
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
