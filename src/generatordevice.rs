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

use crate::audiochunk::{AudioChunk, ChunkStats};
use crate::audiodevice::*;
use crate::config;

use std::f64::consts::PI;
use std::sync::{Arc, Barrier};
use std::thread;

use parking_lot::RwLock;

use rand::{SeedableRng, rngs::SmallRng};
use rand_distr::{Distribution, Uniform};

use crate::CamillaFloat;
use crate::CaptureStatus;
use crate::CommandMessage;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;
use crate::ToCamillaFloat;
use crate::utils::decibels::db_to_linear;
use crate::utils::stash::{container_from_stash, vec_from_stash};

pub(crate) struct SineGenerator {
    time: f64,
    freq: f64,
    delta_t: f64,
    amplitude: CamillaFloat,
}

impl SineGenerator {
    pub(crate) fn new(freq: f64, fs: usize, amplitude: CamillaFloat) -> Self {
        SineGenerator {
            time: 0.0,
            freq,
            delta_t: 1.0 / fs as f64,
            amplitude,
        }
    }
}

impl Iterator for SineGenerator {
    type Item = CamillaFloat;
    fn next(&mut self) -> Option<CamillaFloat> {
        let output = (self.freq * self.time * PI * 2.).sin() as CamillaFloat * self.amplitude;
        self.time += self.delta_t;
        Some(output)
    }
}

pub(crate) struct SquareGenerator {
    time: f64,
    freq: f64,
    delta_t: f64,
    amplitude: CamillaFloat,
}

impl SquareGenerator {
    pub(crate) fn new(freq: f64, fs: usize, amplitude: CamillaFloat) -> Self {
        SquareGenerator {
            time: 0.0,
            freq,
            delta_t: 1.0 / fs as f64,
            amplitude,
        }
    }
}

impl Iterator for SquareGenerator {
    type Item = CamillaFloat;
    fn next(&mut self) -> Option<CamillaFloat> {
        let output =
            (self.freq * self.time * PI * 2.).sin().signum() as CamillaFloat * self.amplitude;
        self.time += self.delta_t;
        Some(output)
    }
}

pub(crate) struct NoiseGenerator {
    rng: SmallRng,
    distribution: Uniform<CamillaFloat>,
}

impl NoiseGenerator {
    pub(crate) fn new(amplitude: CamillaFloat) -> Self {
        let rng = SmallRng::from_os_rng();
        let distribution = Uniform::new_inclusive(-amplitude, amplitude).unwrap();
        NoiseGenerator { rng, distribution }
    }
}

impl Iterator for NoiseGenerator {
    type Item = CamillaFloat;
    fn next(&mut self) -> Option<CamillaFloat> {
        Some(self.distribution.sample(&mut self.rng))
    }
}

/// The signal generator selected by a `Signal` config block, as one type.
///
/// Both the signal generator capture device and the dummy capture device build
/// chunks from this, so the rule that keeps the channels correct lives in one place.
pub(crate) enum SignalSource {
    Sine(SineGenerator),
    Square(SquareGenerator),
    Noise(NoiseGenerator),
}

impl SignalSource {
    pub(crate) fn new(signal: &config::Signal, samplerate: usize) -> Self {
        match signal {
            config::Signal::Sine { freq, level } => SignalSource::Sine(SineGenerator::new(
                freq.get(),
                samplerate,
                db_to_linear(level.get()).to_camilla_float(),
            )),
            config::Signal::Square { freq, level } => SignalSource::Square(SquareGenerator::new(
                freq.get(),
                samplerate,
                db_to_linear(level.get()).to_camilla_float(),
            )),
            config::Signal::WhiteNoise { level } => SignalSource::Noise(NoiseGenerator::new(
                db_to_linear(level.get()).to_camilla_float(),
            )),
        }
    }

    /// Build one chunk worth of waveforms.
    ///
    /// White noise must be generated independently per channel so the channels are
    /// uncorrelated. Periodic signals (sine, square) are generated once and copied to
    /// keep all channels in phase.
    pub(crate) fn waveforms(
        &mut self,
        channels: usize,
        chunksize: usize,
    ) -> Vec<Vec<CamillaFloat>> {
        let independent = matches!(self, SignalSource::Noise(_));
        let mut first = vec_from_stash(chunksize);
        for (sample, value) in first.iter_mut().zip(&mut *self) {
            *sample = value;
        }
        let mut waveforms = container_from_stash(channels);
        waveforms.push(first);
        for _ in 1..channels {
            let mut waveform = vec_from_stash(chunksize);
            if independent {
                for (sample, value) in waveform.iter_mut().zip(&mut *self) {
                    *sample = value;
                }
            } else {
                waveform.copy_from_slice(&waveforms[0]);
            }
            waveforms.push(waveform);
        }
        waveforms
    }
}

impl Iterator for SignalSource {
    type Item = CamillaFloat;
    fn next(&mut self) -> Option<CamillaFloat> {
        match self {
            SignalSource::Sine(g) => g.next(),
            SignalSource::Square(g) => g.next(),
            SignalSource::Noise(g) => g.next(),
        }
    }
}

pub struct GeneratorDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub signal: config::Signal,
}

struct CaptureChannels {
    audio: crossbeam_channel::Sender<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
    command: crossbeam_channel::Receiver<CommandMessage>,
}

struct GeneratorParams {
    channels: usize,
    chunksize: usize,
    capture_status: Arc<RwLock<CaptureStatus>>,
    signal: config::Signal,
    samplerate: usize,
}

fn capture_loop(params: GeneratorParams, msg_channels: CaptureChannels) {
    debug!("starting generator loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut rms_values = Vec::new();
    let mut peak_values = Vec::new();
    let mut generator = SignalSource::new(&params.signal, params.samplerate);

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
                warn!("Signal generator does not support rate adjust. Ignoring request.");
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                error!("Command channel was closed");
                break;
            }
        };
        let waveforms = generator.waveforms(params.channels, params.chunksize);

        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, params.chunksize, params.chunksize);

        chunk.update_stats(&mut chunk_stats);
        crate::push_capture_audio_buffer(&params.capture_status, &chunk);
        crate::update_capture_signal_status(
            &params.capture_status,
            &chunk_stats,
            &mut rms_values,
            &mut peak_values,
        );
        let msg = AudioMessage::Audio(chunk);
        if msg_channels.audio.send(msg).is_err() {
            info!("Processing thread has already stopped.");
            break;
        }
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for GeneratorDevice {
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

        let handle = thread::Builder::new()
            .name("SignalGenerator".to_string())
            .spawn(move || {
                let params = GeneratorParams {
                    signal,
                    samplerate,
                    channels,
                    chunksize,
                    capture_status,
                };
                match status_channel.send(StatusMessage::CaptureReady) {
                    Ok(()) => {}
                    Err(_err) => {}
                }
                barrier.wait();
                let msg_channels = CaptureChannels {
                    audio: channel,
                    status: status_channel,
                    command: command_channel,
                };
                debug!("starting captureloop");
                capture_loop(params, msg_channels);
            })
            .unwrap();
        Ok(Box::new(handle))
    }
}
