// Traits for audio devices
#[cfg(target_os = "linux")]
use crate::alsadevice;
use crate::config;
#[cfg(target_os = "macos")]
use crate::coreaudiodevice;
#[cfg(feature = "cpal-backend")]
use crate::cpaldevice;
use crate::filedevice;
#[cfg(feature = "pulse-backend")]
use crate::pulsedevice;
#[cfg(target_os = "windows")]
use crate::wasapidevice;
use num_integer as integer;
use rubato::{
    FftFixedOut, InterpolationParameters, InterpolationType, SincFixedOut, VecResampler,
    WindowFunction,
};
use std::error;
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Barrier, RwLock};
use std::thread;
use std::time::Instant;

use crate::CommandMessage;
use crate::PrcFmt;
use crate::Res;
use crate::StatusMessage;
use crate::{CaptureStatus, PlaybackStatus};

pub const RATE_CHANGE_THRESHOLD_COUNT: usize = 3;
pub const RATE_CHANGE_THRESHOLD_VALUE: f32 = 0.04;

#[derive(Debug)]
pub struct DeviceError {
    desc: String,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.desc)
    }
}

impl error::Error for DeviceError {
    fn description(&self) -> &str {
        &self.desc
    }
}

impl DeviceError {
    pub fn new(desc: &str) -> Self {
        DeviceError {
            desc: desc.to_owned(),
        }
    }
}

pub enum AudioMessage {
    //Quit,
    Audio(AudioChunk),
    Pause,
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

/// Container for RMS and peak values of a chunk
pub struct ChunkStats {
    pub rms: Vec<PrcFmt>,
    pub peak: Vec<PrcFmt>,
}

impl ChunkStats {
    pub fn rms_db(&self) -> Vec<f32> {
        self.rms
            .iter()
            .map(|val| {
                if *val == 0.0 {
                    -1000.0
                } else {
                    20.0 * val.log10() as f32
                }
            })
            .collect()
    }

    pub fn rms_linear(&self) -> Vec<f32> {
        self.rms.iter().map(|val| *val as f32).collect()
    }

    pub fn peak_db(&self) -> Vec<f32> {
        self.peak
            .iter()
            .map(|val| {
                if *val == 0.0 {
                    -1000.0
                } else {
                    20.0 * val.log10() as f32
                }
            })
            .collect()
    }

    pub fn peak_linear(&self) -> Vec<f32> {
        self.peak.iter().map(|val| *val as f32).collect()
    }
}

impl AudioChunk {
    pub fn new(
        waveforms: Vec<Vec<PrcFmt>>,
        maxval: PrcFmt,
        minval: PrcFmt,
        frames: usize,
        valid_frames: usize,
    ) -> Self {
        let timestamp = Instant::now();
        let channels = waveforms.len();
        //let frames = waveforms[0].len();
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

    pub fn get_stats(&self) -> ChunkStats {
        let rms_peak: Vec<(PrcFmt, PrcFmt)> =
            self.waveforms.iter().map(|wf| rms_and_peak(wf)).collect();
        let rms: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.0).collect();
        let peak: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.1).collect();
        ChunkStats { rms, peak }
    }
}

/// Get RMS and peak value of a vector
pub fn rms_and_peak(data: &[PrcFmt]) -> (PrcFmt, PrcFmt) {
    if !data.is_empty() {
        let (squaresum, peakval) = data.iter().fold((0.0, 0.0), |(sqsum, peak), value| {
            let newpeak = if peak > value.abs() {
                peak
            } else {
                value.abs()
            };
            (sqsum + *value * *value, newpeak)
        });
        ((squaresum / data.len() as PrcFmt).sqrt(), peakval)
    } else {
        (0.0, 0.0)
    }
}

/// A playback device
pub trait PlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
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
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>>;
}

/// Create a playback device.
pub fn get_playback_device(conf: config::Devices) -> Box<dyn PlaybackDevice> {
    match conf.playback {
        #[cfg(target_os = "linux")]
        config::PlaybackDevice::Alsa {
            channels,
            device,
            format,
        } => Box::new(alsadevice::AlsaPlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            target_level: conf.target_level,
            adjust_period: conf.adjust_period,
            enable_rate_adjust: conf.enable_rate_adjust,
        }),
        #[cfg(feature = "pulse-backend")]
        config::PlaybackDevice::Pulse {
            channels,
            device,
            format,
        } => Box::new(pulsedevice::PulsePlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
        }),
        config::PlaybackDevice::File {
            channels,
            filename,
            format,
            ..
        } => Box::new(filedevice::FilePlaybackDevice {
            destination: filedevice::PlaybackDest::Filename(filename),
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
        }),
        config::PlaybackDevice::Stdout {
            channels, format, ..
        } => Box::new(filedevice::FilePlaybackDevice {
            destination: filedevice::PlaybackDest::Stdout,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
        }),
        #[cfg(target_os = "macos")]
        config::PlaybackDevice::CoreAudio {
            channels,
            device,
            format,
            change_format,
            exclusive,
        } => Box::new(coreaudiodevice::CoreaudioPlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            target_level: conf.target_level,
            adjust_period: conf.adjust_period,
            enable_rate_adjust: conf.enable_rate_adjust,
            change_format,
            exclusive,
        }),
        #[cfg(target_os = "windows")]
        config::PlaybackDevice::Wasapi {
            channels,
            device,
            format,
            exclusive,
        } => Box::new(wasapidevice::WasapiPlaybackDevice {
            devname: device,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            exclusive,
            channels,
            sample_format: format,
            target_level: conf.target_level,
            adjust_period: conf.adjust_period,
            enable_rate_adjust: conf.enable_rate_adjust,
        }),
        #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
        config::PlaybackDevice::Jack { channels, device } => {
            Box::new(cpaldevice::CpalPlaybackDevice {
                devname: device,
                host: cpaldevice::CpalHost::Jack,
                samplerate: conf.samplerate,
                chunksize: conf.chunksize,
                channels,
                sample_format: config::SampleFormat::FLOAT32LE,
                target_level: conf.target_level,
                adjust_period: conf.adjust_period,
                enable_rate_adjust: conf.enable_rate_adjust,
            })
        }
    }
}

pub fn resampler_is_async(conf: &config::Resampler) -> bool {
    matches!(
        &conf,
        config::Resampler::FastAsync
            | config::Resampler::BalancedAsync
            | config::Resampler::AccurateAsync
            | config::Resampler::FreeAsync { .. }
    )
}

pub fn get_async_parameters(
    conf: &config::Resampler,
    samplerate: usize,
    capture_samplerate: usize,
) -> InterpolationParameters {
    match &conf {
        config::Resampler::FastAsync => {
            let sinc_len = 64;
            let f_cutoff = 0.915_602_15;
            let oversampling_factor = 1024;
            let interpolation = InterpolationType::Linear;
            let window = WindowFunction::Hann2;
            InterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::Resampler::BalancedAsync => {
            let sinc_len = 128;
            let f_cutoff = 0.925_914_65;
            let oversampling_factor = 1024;
            let interpolation = InterpolationType::Linear;
            let window = WindowFunction::Blackman2;
            InterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::Resampler::AccurateAsync => {
            let sinc_len = 256;
            let f_cutoff = 0.947_337_15;
            let oversampling_factor = 256;
            let interpolation = InterpolationType::Cubic;
            let window = WindowFunction::BlackmanHarris2;
            InterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::Resampler::Synchronous => {
            let sinc_len = 64;
            let f_cutoff = 0.915_602_15;
            let gcd = integer::gcd(samplerate, capture_samplerate);
            let oversampling_factor = samplerate / gcd;
            let interpolation = InterpolationType::Nearest;
            let window = WindowFunction::Hann2;
            InterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::Resampler::FreeAsync {
            sinc_len,
            oversampling_ratio,
            interpolation,
            window,
            f_cutoff,
        } => {
            let interp = match interpolation {
                config::InterpolationType::Cubic => InterpolationType::Cubic,
                config::InterpolationType::Linear => InterpolationType::Linear,
                config::InterpolationType::Nearest => InterpolationType::Nearest,
            };
            let wind = match window {
                config::WindowFunction::Hann => WindowFunction::Hann,
                config::WindowFunction::Hann2 => WindowFunction::Hann2,
                config::WindowFunction::Blackman => WindowFunction::Blackman,
                config::WindowFunction::Blackman2 => WindowFunction::Blackman2,
                config::WindowFunction::BlackmanHarris => WindowFunction::BlackmanHarris,
                config::WindowFunction::BlackmanHarris2 => WindowFunction::BlackmanHarris2,
            };
            InterpolationParameters {
                sinc_len: *sinc_len,
                f_cutoff: *f_cutoff,
                oversampling_factor: *oversampling_ratio,
                interpolation: interp,
                window: wind,
            }
        }
    }
}

pub fn get_resampler(
    conf: &config::Resampler,
    num_channels: usize,
    samplerate: usize,
    capture_samplerate: usize,
    chunksize: usize,
) -> Option<Box<dyn VecResampler<PrcFmt>>> {
    if resampler_is_async(conf) {
        let parameters = get_async_parameters(conf, samplerate, capture_samplerate);
        debug!(
            "Creating asynchronous resampler with parameters: {:?}",
            parameters
        );
        Some(Box::new(
            SincFixedOut::<PrcFmt>::new(
                samplerate as f64 / capture_samplerate as f64,
                1.1,
                parameters,
                chunksize,
                num_channels,
            )
            .unwrap(),
        ))
    } else {
        Some(Box::new(
            FftFixedOut::<PrcFmt>::new(capture_samplerate, samplerate, chunksize, 2, num_channels)
                .unwrap(),
        ))
    }
}

/// Create a capture device.
pub fn get_capture_device(conf: config::Devices) -> Box<dyn CaptureDevice> {
    //let resampler = get_resampler(&conf);
    let capture_samplerate = if conf.capture_samplerate > 0 && conf.enable_resampling {
        conf.capture_samplerate
    } else {
        conf.samplerate
    };
    let diff_rates = capture_samplerate != conf.samplerate;
    // Check for non-optimal resampling settings
    if !diff_rates && conf.enable_resampling && !conf.enable_rate_adjust {
        warn!(
            "Needless 1:1 sample rate conversion active. Not needed since enable_rate_adjust=False"
        );
    } else if diff_rates
        && conf.enable_resampling
        && !conf.enable_rate_adjust
        && resampler_is_async(&conf.resampler_type)
    {
        info!("Using Async resampler for synchronous resampling. Consider switching to \"Synchronous\" to save CPU time.");
    }
    match conf.capture {
        #[cfg(target_os = "linux")]
        config::CaptureDevice::Alsa {
            channels,
            device,
            format,
        } => Box::new(alsadevice::AlsaCaptureDevice {
            devname: device,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate,
            resampler_conf: conf.resampler_type,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
            stop_on_rate_change: conf.stop_on_rate_change,
            rate_measure_interval: conf.rate_measure_interval,
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
            resampler_conf: conf.resampler_type,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
        }),
        config::CaptureDevice::File {
            channels,
            filename,
            format,
            extra_samples,
            skip_bytes,
            read_bytes,
        } => Box::new(filedevice::FileCaptureDevice {
            source: filedevice::CaptureSource::Filename(filename),
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate,
            resampler_conf: conf.resampler_type,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            extra_samples,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
            skip_bytes,
            read_bytes,
            stop_on_rate_change: conf.stop_on_rate_change,
            rate_measure_interval: conf.rate_measure_interval,
        }),
        config::CaptureDevice::Stdin {
            channels,
            format,
            extra_samples,
            skip_bytes,
            read_bytes,
        } => Box::new(filedevice::FileCaptureDevice {
            source: filedevice::CaptureSource::Stdin,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            capture_samplerate,
            resampler_conf: conf.resampler_type,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            extra_samples,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
            skip_bytes,
            read_bytes,
            stop_on_rate_change: conf.stop_on_rate_change,
            rate_measure_interval: conf.rate_measure_interval,
        }),
        #[cfg(target_os = "macos")]
        config::CaptureDevice::CoreAudio {
            channels,
            device,
            format,
            change_format,
        } => Box::new(coreaudiodevice::CoreaudioCaptureDevice {
            devname: device,
            samplerate: conf.samplerate,
            enable_resampling: conf.enable_resampling,
            resampler_conf: conf.resampler_type,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            change_format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
            stop_on_rate_change: conf.stop_on_rate_change,
            rate_measure_interval: conf.rate_measure_interval,
        }),
        #[cfg(target_os = "windows")]
        config::CaptureDevice::Wasapi {
            channels,
            device,
            format,
            exclusive,
            loopback,
        } => Box::new(wasapidevice::WasapiCaptureDevice {
            devname: device,
            samplerate: conf.samplerate,
            exclusive,
            loopback,
            enable_resampling: conf.enable_resampling,
            resampler_conf: conf.resampler_type,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            silence_threshold: conf.silence_threshold,
            silence_timeout: conf.silence_timeout,
            stop_on_rate_change: conf.stop_on_rate_change,
            rate_measure_interval: conf.rate_measure_interval,
        }),
        #[cfg(all(feature = "cpal-backend", feature = "jack-backend"))]
        config::CaptureDevice::Jack { channels, device } => {
            Box::new(cpaldevice::CpalCaptureDevice {
                devname: device,
                host: cpaldevice::CpalHost::Jack,
                samplerate: conf.samplerate,
                enable_resampling: conf.enable_resampling,
                resampler_conf: conf.resampler_type,
                capture_samplerate,
                chunksize: conf.chunksize,
                channels,
                sample_format: config::SampleFormat::FLOAT32LE,
                silence_threshold: conf.silence_threshold,
                silence_timeout: conf.silence_timeout,
                stop_on_rate_change: conf.stop_on_rate_change,
                rate_measure_interval: conf.rate_measure_interval,
            })
        }
    }
}

pub fn calculate_speed(avg_level: f64, target_level: usize, adjust_period: f32, srate: u32) -> f64 {
    let diff = avg_level as isize - target_level as isize;
    let rel_diff = (diff as f64) / (srate as f64);
    let speed = 1.0 - 0.5 * rel_diff / adjust_period as f64;
    debug!(
        "Avg. buffer level: {:.1}, target level: {:.1}, corrected capture rate: {:.4}%",
        avg_level,
        target_level,
        100.0 * speed
    );
    speed
}

#[cfg(test)]
mod tests {
    use crate::audiodevice::{rms_and_peak, AudioChunk, ChunkStats};

    #[test]
    fn vec_rms_and_peak() {
        let data = vec![1.0, 1.0, -1.0, -1.0];
        assert_eq!((1.0, 1.0), rms_and_peak(&data));
        let data = vec![0.0, -4.0, 0.0, 0.0];
        assert_eq!((2.0, 4.0), rms_and_peak(&data));
    }

    #[test]
    fn chunk_rms_and_peak() {
        let data1 = vec![1.0, 1.0, -1.0, -1.0];
        let data2 = vec![0.0, -4.0, 0.0, 0.0];
        let waveforms = vec![data1, data2];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 1, 1);
        let stats = chunk.get_stats();
        assert_eq!(stats.rms[0], 1.0);
        assert_eq!(stats.rms[1], 2.0);
        assert_eq!(stats.peak[0], 1.0);
        assert_eq!(stats.peak[1], 4.0);
    }

    #[test]
    fn rms_and_peak_to_db() {
        let stats = ChunkStats {
            rms: vec![0.0, 0.5],
            peak: vec![1.0],
        };
        assert_eq!(-1000.0, stats.rms_db()[0]);
        assert_eq!(0.0, stats.peak_db()[0]);
        assert!(stats.rms_db()[1] > -6.1 && stats.rms_db()[1] < -5.9);
    }
}
