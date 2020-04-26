// Traits for audio devices
#[cfg(feature = "alsa-backend")]
use alsadevice;
use config;
use filedevice;
use num::integer;
#[cfg(feature = "pulse-backend")]
use pulsedevice;
use rubato::{SincFixedOut, Resampler, Interpolation};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

pub enum AudioMessage {
    //Quit,
    Audio(AudioChunk),
    EndOfStream,
}

/// Main container of audio data
pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub maxval: PrcFmt,
    pub minval: PrcFmt,
    pub timestamp: Instant,
    pub valid_frames: usize,
    pub waveforms: Vec<Vec<PrcFmt>>,
}

impl AudioChunk {
    pub fn new(
        waveforms: Vec<Vec<PrcFmt>>,
        maxval: PrcFmt,
        minval: PrcFmt,
        valid_frames: usize,
    ) -> Self {
        let timestamp = Instant::now();
        let channels = waveforms.len();
        let frames = waveforms[0].len();
        AudioChunk {
            frames,
            channels,
            maxval,
            minval,
            timestamp,
            valid_frames,
            waveforms,
        }
    }

    pub fn from(chunk: &AudioChunk, waveforms: Vec<Vec<PrcFmt>>) -> Self {
        let timestamp = chunk.timestamp;
        let maxval = chunk.maxval;
        let minval = chunk.minval;
        let frames = chunk.frames;
        let valid_frames = chunk.valid_frames;
        let channels = waveforms.len();
        AudioChunk {
            frames,
            channels,
            maxval,
            minval,
            timestamp,
            valid_frames,
            waveforms,
        }
    }
}

/// A playback device
pub trait PlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>>;
}

/// A capture device
pub trait CaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>>;
}

/// Create a playback device.
pub fn get_playback_device(conf: config::Devices) -> Box<dyn PlaybackDevice> {
    match conf.playback {
        #[cfg(feature = "alsa-backend")]
        config::PlaybackDevice::Alsa {
            channels,
            device,
            format,
        } => Box::new(alsadevice::AlsaPlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            format,
            target_level: conf.target_level,
            adjust_period: conf.adjust_period,
        }),
        #[cfg(feature = "pulse-backend")]
        config::PlaybackDevice::Pulse {
            channels,
            device,
            format,
        } => Box::new(pulsedevice::PulsePlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            bufferlength: conf.chunksize,
            channels,
            format,
        }),
        config::PlaybackDevice::File {
            channels,
            filename,
            format,
            ..
        } => Box::new(filedevice::FilePlaybackDevice {
            filename,
            samplerate: conf.samplerate,
            bufferlength: conf.chunksize,
            channels,
            format,
        }),
    }
}

pub fn get_resampler(conf: &config::Resampler, num_channels: usize, samplerate: usize, capture_samplerate: usize, chunksize: usize) -> Option<Box<dyn Resampler<PrcFmt>>> {   
    match &conf {
        config::Resampler::FastAsync => {
            let sinc_len = 64;
            let f_cutoff = 0.5f32.powf(16.0 / sinc_len as f32);
            let oversampling = 1024;
            let interp = Interpolation::Linear;
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                sinc_len,
                f_cutoff,
                oversampling,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
        config::Resampler::BalancedAsync => {
            let sinc_len = 128;
            let f_cutoff = 0.5f32.powf(16.0 / sinc_len as f32);
            let oversampling = 1024;
            let interp = Interpolation::Cubic;
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                sinc_len,
                f_cutoff,
                oversampling,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
        config::Resampler::AccurateAsync => {
            let sinc_len = 256;
            let f_cutoff = 0.5f32.powf(16.0 / sinc_len as f32);
            let oversampling = 256;
            let interp = Interpolation::Cubic;
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                sinc_len,
                f_cutoff,
                oversampling,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
        config::Resampler::FastSync => {
            let sinc_len = 128;
            let f_cutoff = 0.5f32.powf(16.0 / sinc_len as f32);
            let gcd = integer::gcd(samplerate, capture_samplerate);
            let oversampling = samplerate/gcd;
            let interp = Interpolation::Nearest;
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                sinc_len,
                f_cutoff,
                oversampling,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
        config::Resampler::AccurateSync => {
            let sinc_len = 256;
            let f_cutoff = 0.5f32.powf(16.0 / sinc_len as f32);
            let gcd = integer::gcd(samplerate, capture_samplerate);
            let oversampling = samplerate/gcd;
            let interp = Interpolation::Nearest;
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                sinc_len,
                f_cutoff,
                oversampling,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
        config::Resampler::Free { sinc_len, oversampling_ratio, interpolation } => {
            let f_cutoff = 0.5f32.powf(16.0 / *sinc_len as f32);
            let interp = match interpolation {
                config::InterpolationType::Cubic => Interpolation::Cubic,
                config::InterpolationType::Linear => Interpolation::Linear,
                config::InterpolationType::Nearest => Interpolation::Nearest,
            };
            let resampler = SincFixedOut::<PrcFmt>::new(
                samplerate as f32 / capture_samplerate as f32,
                *sinc_len,
                f_cutoff,
                *oversampling_ratio,
                interp,
                chunksize,
                num_channels
            );
            Some(Box::new(resampler))
        }
    }
}

/// Create a capture device. 
pub fn get_capture_device(conf: config::Devices) -> Box<dyn CaptureDevice> {
    //let resampler = get_resampler(&conf);
    match conf.capture {
        #[cfg(feature = "alsa-backend")]
        config::CaptureDevice::Alsa {
            channels,
            device,
            format,
        } => Box::new(alsadevice::AlsaCaptureDevice {
            devname: device,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate: conf.capture_samplerate,
            resampler_conf: conf.resampler_type,
            chunksize: conf.chunksize,
            channels,
            format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
        }),
        #[cfg(feature = "pulse-backend")]
        config::CaptureDevice::Pulse {
            channels,
            device,
            format,
        } => Box::new(pulsedevice::PulseCaptureDevice {
            devname: device,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate: conf.capture_samplerate,
            bufferlength: conf.chunksize,
            channels,
            format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
        }),
        config::CaptureDevice::File {
            channels,
            filename,
            format,
            extra_samples,
        } => Box::new(filedevice::FileCaptureDevice {
            filename,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate: conf.capture_samplerate,
            bufferlength: conf.chunksize,
            channels,
            format,
            extra_samples,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
        }),
    }
}
