use crate::audiodevice::*;
use crate::config;

use std::f64::consts::PI;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;

use parking_lot::RwLock;

use rand::{rngs::SmallRng, SeedableRng};
use rand_distr::{Distribution, Uniform};

use crate::CaptureStatus;
use crate::CommandMessage;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::ProcessingState;
use crate::Res;
use crate::StatusMessage;

struct SineGenerator {
    time: f64,
    freq: f64,
    delta_t: f64,
    amplitude: PrcFmt,
}

impl SineGenerator {
    fn new(freq: f64, fs: usize, amplitude: PrcFmt) -> Self {
        SineGenerator {
            time: 0.0,
            freq,
            delta_t: 1.0 / fs as f64,
            amplitude,
        }
    }
}

impl Iterator for SineGenerator {
    type Item = PrcFmt;
    fn next(&mut self) -> Option<PrcFmt> {
        let output = (self.freq * self.time * PI * 2.).sin() as PrcFmt * self.amplitude;
        self.time += self.delta_t;
        Some(output)
    }
}

struct SquareGenerator {
    time: f64,
    freq: f64,
    delta_t: f64,
    amplitude: PrcFmt,
}

impl SquareGenerator {
    fn new(freq: f64, fs: usize, amplitude: PrcFmt) -> Self {
        SquareGenerator {
            time: 0.0,
            freq,
            delta_t: 1.0 / fs as f64,
            amplitude,
        }
    }
}

impl Iterator for SquareGenerator {
    type Item = PrcFmt;
    fn next(&mut self) -> Option<PrcFmt> {
        let output = (self.freq * self.time * PI * 2.).sin().signum() as PrcFmt * self.amplitude;
        self.time += self.delta_t;
        Some(output)
    }
}

struct NoiseGenerator {
    rng: SmallRng,
    distribution: Uniform<PrcFmt>,
}

impl NoiseGenerator {
    fn new(amplitude: PrcFmt) -> Self {
        let rng = SmallRng::from_entropy();
        let distribution = Uniform::new_inclusive(-amplitude, amplitude);
        NoiseGenerator { rng, distribution }
    }
}

impl Iterator for NoiseGenerator {
    type Item = PrcFmt;
    fn next(&mut self) -> Option<PrcFmt> {
        Some(self.distribution.sample(&mut self.rng))
    }
}

pub struct GeneratorDevice {
    pub chunksize: usize,
    pub samplerate: usize,
    pub channels: usize,
    pub signal: config::Signal,
}

struct CaptureChannels {
    audio: mpsc::SyncSender<AudioMessage>,
    status: crossbeam_channel::Sender<StatusMessage>,
    command: mpsc::Receiver<CommandMessage>,
}

struct GeneratorParams {
    channels: usize,
    chunksize: usize,
    capture_status: Arc<RwLock<CaptureStatus>>,
    signal: config::Signal,
    samplerate: usize,
}

fn decibel_to_amplitude(level: PrcFmt) -> PrcFmt {
    (10.0 as PrcFmt).powf(level / 20.0)
}

fn capture_loop(params: GeneratorParams, msg_channels: CaptureChannels) {
    debug!("starting generator loop");
    let mut chunk_stats = ChunkStats {
        rms: vec![0.0; params.channels],
        peak: vec![0.0; params.channels],
    };
    let mut sine_gen;
    let mut square_gen;
    let mut noise_gen;

    let mut generator: &mut dyn Iterator<Item = PrcFmt> = match params.signal {
        config::Signal::Sine { freq, level } => {
            sine_gen = SineGenerator::new(freq, params.samplerate, decibel_to_amplitude(level));
            &mut sine_gen as &mut dyn Iterator<Item = PrcFmt>
        }
        config::Signal::Square { freq, level } => {
            square_gen = SquareGenerator::new(freq, params.samplerate, decibel_to_amplitude(level));
            &mut square_gen as &mut dyn Iterator<Item = PrcFmt>
        }
        config::Signal::WhiteNoise { level } => {
            noise_gen = NoiseGenerator::new(decibel_to_amplitude(level));
            &mut noise_gen as &mut dyn Iterator<Item = PrcFmt>
        }
    };

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
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                error!("Command channel was closed");
                break;
            }
        };
        let mut waveform = vec![0.0; params.chunksize];
        for (sample, value) in waveform.iter_mut().zip(&mut generator) {
            *sample = value;
        }
        let mut waveforms = Vec::with_capacity(params.channels);
        waveforms.push(waveform);
        for _ in 1..params.channels {
            waveforms.push(waveforms[0].clone());
        }

        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, params.chunksize, params.chunksize);

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
        let msg = AudioMessage::Audio(chunk);
        if msg_channels.audio.send(msg).is_err() {
            info!("Processing thread has already stopped.");
            break;
        }
    }
    params.capture_status.write().state = ProcessingState::Inactive;
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for GeneratorDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
        _processing_status: Arc<ProcessingParameters>,
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
