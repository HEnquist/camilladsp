// Traits for audio devices
#[cfg(target_os = "linux")]
use crate::alsadevice;
use crate::config;
#[cfg(target_os = "macos")]
use crate::coreaudiodevice;
#[cfg(all(
    feature = "cpal-backend",
    feature = "jack-backend",
    any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd"
    )
))]
use crate::cpaldevice;
use crate::filedevice;
#[cfg(feature = "pulse-backend")]
use crate::pulsedevice;
#[cfg(target_os = "windows")]
use crate::wasapidevice;
use parking_lot::RwLock;
use rubato::{
    calculate_cutoff, FastFixedOut, FftFixedOut, PolynomialDegree, SincFixedOut,
    SincInterpolationParameters, SincInterpolationType, VecResampler, WindowFunction,
};
use std::error;
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
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

    pub fn stats(&self) -> ChunkStats {
        let rms_peak: Vec<(PrcFmt, PrcFmt)> =
            self.waveforms.iter().map(|wf| rms_and_peak(wf)).collect();
        let rms: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.0).collect();
        let peak: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.1).collect();
        ChunkStats { rms, peak }
    }

    pub fn update_stats(&self, stats: &mut ChunkStats) {
        stats.rms.resize(self.channels, 0.0);
        stats.peak.resize(self.channels, 0.0);
        for (wf, (peakval, rmsval)) in self
            .waveforms
            .iter()
            .zip(stats.peak.iter_mut().zip(stats.rms.iter_mut()))
        {
            let (rms, peak) = rms_and_peak(wf);
            *peakval = peak;
            *rmsval = rms;
        }
    }

    pub fn update_channel_mask(&self, mask: &mut [bool]) {
        mask.iter_mut()
            .zip(self.waveforms.iter())
            .for_each(|(m, w)| *m = !w.is_empty());
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
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        playback_status: Arc<RwLock<PlaybackStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>>;
}

/// A capture device
pub trait CaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: crossbeam_channel::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
        capture_status: Arc<RwLock<CaptureStatus>>,
    ) -> Res<Box<thread::JoinHandle<()>>>;
}

/// Create a playback device.
pub fn new_playback_device(conf: config::Devices) -> Box<dyn PlaybackDevice> {
    match conf.playback {
        #[cfg(target_os = "linux")]
        config::PlaybackDevice::Alsa {
            channels,
            ref device,
            format,
        } => Box::new(alsadevice::AlsaPlaybackDevice {
            devname: device.clone(),
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            target_level: conf.target_level(),
            adjust_period: conf.adjust_period(),
            enable_rate_adjust: conf.rate_adjust(),
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
        config::PlaybackDevice::CoreAudio(ref dev) => {
            Box::new(coreaudiodevice::CoreaudioPlaybackDevice {
                devname: dev.device.clone(),
                samplerate: conf.samplerate,
                chunksize: conf.chunksize,
                channels: dev.channels,
                sample_format: dev.format,
                target_level: conf.target_level(),
                adjust_period: conf.adjust_period(),
                enable_rate_adjust: conf.rate_adjust(),
                exclusive: dev.is_exclusive(),
            })
        }
        #[cfg(target_os = "windows")]
        config::PlaybackDevice::Wasapi(ref dev) => Box::new(wasapidevice::WasapiPlaybackDevice {
            devname: dev.device.clone(),
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            exclusive: dev.is_exclusive(),
            channels: dev.channels,
            sample_format: dev.format,
            target_level: conf.target_level(),
            adjust_period: conf.adjust_period(),
            enable_rate_adjust: conf.rate_adjust(),
        }),
        #[cfg(all(
            feature = "cpal-backend",
            feature = "jack-backend",
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd"
            )
        ))]
        config::PlaybackDevice::Jack {
            channels,
            ref device,
        } => Box::new(cpaldevice::CpalPlaybackDevice {
            devname: device.clone(),
            host: cpaldevice::CpalHost::Jack,
            samplerate: conf.samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: config::SampleFormat::FLOAT32LE,
            target_level: conf.target_level(),
            adjust_period: conf.adjust_period(),
            enable_rate_adjust: conf.rate_adjust(),
        }),
    }
}

pub fn resampler_is_async(conf: &Option<config::Resampler>) -> bool {
    matches!(
        &conf,
        Some(config::Resampler::AsyncSinc { .. }) | Some(config::Resampler::AsyncPoly { .. })
    )
}

pub fn new_async_sinc_parameters(
    resampler_conf: &config::AsyncSincParameters,
) -> SincInterpolationParameters {
    match &resampler_conf {
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::VeryFast,
        } => {
            let sinc_len = 64;
            let oversampling_factor = 1024;
            let interpolation = SincInterpolationType::Linear;
            let window = WindowFunction::Hann2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Fast,
        } => {
            let sinc_len = 128;
            let oversampling_factor = 1024;
            let interpolation = SincInterpolationType::Linear;
            let window = WindowFunction::Blackman2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Balanced,
        } => {
            let sinc_len = 192;
            let oversampling_factor = 512;
            let interpolation = SincInterpolationType::Quadratic;
            let window = WindowFunction::BlackmanHarris2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Accurate,
        } => {
            let sinc_len = 256;
            let oversampling_factor = 256;
            let interpolation = SincInterpolationType::Cubic;
            let window = WindowFunction::BlackmanHarris2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Free {
            sinc_len,
            window,
            f_cutoff,
            interpolation,
            oversampling_factor,
        } => {
            let interpolation = match interpolation {
                config::AsyncSincInterpolation::Nearest => SincInterpolationType::Nearest,
                config::AsyncSincInterpolation::Linear => SincInterpolationType::Linear,
                config::AsyncSincInterpolation::Quadratic => SincInterpolationType::Quadratic,
                config::AsyncSincInterpolation::Cubic => SincInterpolationType::Cubic,
            };

            let wind = match window {
                config::AsyncSincWindow::Hann => WindowFunction::Hann,
                config::AsyncSincWindow::Hann2 => WindowFunction::Hann2,
                config::AsyncSincWindow::Blackman => WindowFunction::Blackman,
                config::AsyncSincWindow::Blackman2 => WindowFunction::Blackman2,
                config::AsyncSincWindow::BlackmanHarris => WindowFunction::BlackmanHarris,
                config::AsyncSincWindow::BlackmanHarris2 => WindowFunction::BlackmanHarris2,
            };
            let cutoff = if let Some(co) = f_cutoff {
                *co
            } else {
                calculate_cutoff(*sinc_len, wind)
            };
            SincInterpolationParameters {
                sinc_len: *sinc_len,
                f_cutoff: cutoff,
                oversampling_factor: *oversampling_factor,
                interpolation,
                window: wind,
            }
        }
    }
}

pub fn new_resampler(
    resampler_conf: &Option<config::Resampler>,
    num_channels: usize,
    samplerate: usize,
    capture_samplerate: usize,
    chunksize: usize,
) -> Option<Box<dyn VecResampler<PrcFmt>>> {
    match &resampler_conf {
        Some(config::Resampler::AsyncSinc(parameters)) => {
            let sinc_params = new_async_sinc_parameters(parameters);
            debug!(
                "Creating asynchronous resampler with parameters: {:?}",
                sinc_params
            );
            Some(Box::new(
                SincFixedOut::<PrcFmt>::new(
                    samplerate as f64 / capture_samplerate as f64,
                    1.1,
                    sinc_params,
                    chunksize,
                    num_channels,
                )
                .unwrap(),
            ))
        }
        Some(config::Resampler::AsyncPoly { interpolation }) => {
            let degree = match interpolation {
                config::AsyncPolyInterpolation::Linear => PolynomialDegree::Linear,
                config::AsyncPolyInterpolation::Cubic => PolynomialDegree::Cubic,
                config::AsyncPolyInterpolation::Quintic => PolynomialDegree::Quintic,
                config::AsyncPolyInterpolation::Septic => PolynomialDegree::Septic,
            };
            Some(Box::new(
                FastFixedOut::<PrcFmt>::new(
                    samplerate as f64 / capture_samplerate as f64,
                    1.1,
                    degree,
                    chunksize,
                    num_channels,
                )
                .unwrap(),
            ))
        }
        Some(config::Resampler::Synchronous) => Some(Box::new(
            FftFixedOut::<PrcFmt>::new(capture_samplerate, samplerate, chunksize, 2, num_channels)
                .unwrap(),
        )),
        None => None,
    }
}

/// Create a capture device.
pub fn new_capture_device(conf: config::Devices) -> Box<dyn CaptureDevice> {
    //let resampler = new_resampler(&conf);
    let capture_samplerate = if conf.capture_samplerate.is_some() && conf.resampler.is_some() {
        conf.capture_samplerate.unwrap()
    } else {
        conf.samplerate
    };
    let diff_rates = capture_samplerate != conf.samplerate;
    // Check for non-optimal resampling settings
    if !diff_rates && conf.resampler.is_some() && !conf.rate_adjust() {
        warn!(
            "Needless 1:1 sample rate conversion active. Not needed since enable_rate_adjust=False"
        );
    } else if diff_rates
        && conf.resampler.is_some()
        && !conf.rate_adjust()
        && matches!(&conf.resampler, Some(config::Resampler::AsyncSinc { .. }))
    {
        info!("Using AsyncSinc resampler for synchronous resampling. Consider switching to \"Synchronous\" to save CPU time.");
    } else if diff_rates
        && conf.resampler.is_some()
        && !conf.rate_adjust()
        && matches!(&conf.resampler, Some(config::Resampler::AsyncPoly { .. }))
    {
        info!("Using AsyncPoly resampler for synchronous resampling. Consider switching to \"Synchronous\" to increase resampling quality.");
    }
    match conf.capture {
        #[cfg(target_os = "linux")]
        config::CaptureDevice::Alsa {
            channels,
            ref device,
            format,
        } => Box::new(alsadevice::AlsaCaptureDevice {
            devname: device.clone(),
            samplerate: conf.samplerate,
            capture_samplerate,
            resampler_config: conf.resampler,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
        #[cfg(feature = "pulse-backend")]
        config::CaptureDevice::Pulse {
            channels,
            ref device,
            format,
        } => Box::new(pulsedevice::PulseCaptureDevice {
            devname: device.clone(),
            samplerate: conf.samplerate,
            resampler_config: conf.resampler,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: format,
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
        }),
        config::CaptureDevice::File(ref dev) => Box::new(filedevice::FileCaptureDevice {
            source: filedevice::CaptureSource::Filename(dev.filename.clone()),
            samplerate: conf.samplerate,
            capture_samplerate,
            resampler_config: conf.resampler,
            chunksize: conf.chunksize,
            channels: dev.channels,
            sample_format: dev.format,
            extra_samples: dev.extra_samples(),
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            skip_bytes: dev.skip_bytes(),
            read_bytes: dev.read_bytes(),
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
        config::CaptureDevice::Stdin(ref dev) => Box::new(filedevice::FileCaptureDevice {
            source: filedevice::CaptureSource::Stdin,
            samplerate: conf.samplerate,
            capture_samplerate,
            resampler_config: conf.resampler,
            chunksize: conf.chunksize,
            channels: dev.channels,
            sample_format: dev.format,
            extra_samples: dev.extra_samples(),
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            skip_bytes: dev.skip_bytes(),
            read_bytes: dev.read_bytes(),
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
        #[cfg(all(target_os = "linux", feature = "bluez-backend"))]
        config::CaptureDevice::Bluez(ref dev) => Box::new(filedevice::FileCaptureDevice {
            source: filedevice::CaptureSource::BluezDBus(dev.service(), dev.dbus_path.clone()),
            samplerate: conf.samplerate,
            capture_samplerate,
            resampler_config: conf.resampler,
            chunksize: conf.chunksize,
            channels: dev.channels,
            sample_format: dev.format,
            extra_samples: 0,
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            skip_bytes: 0,
            read_bytes: 0,
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
        #[cfg(target_os = "macos")]
        config::CaptureDevice::CoreAudio(ref dev) => {
            Box::new(coreaudiodevice::CoreaudioCaptureDevice {
                devname: dev.device.clone(),
                samplerate: conf.samplerate,
                resampler_config: conf.resampler,
                capture_samplerate,
                chunksize: conf.chunksize,
                channels: dev.channels,
                sample_format: dev.format,
                silence_threshold: conf.silence_threshold(),
                silence_timeout: conf.silence_timeout(),
                stop_on_rate_change: conf.stop_on_rate_change(),
                rate_measure_interval: conf.rate_measure_interval(),
            })
        }
        #[cfg(target_os = "windows")]
        config::CaptureDevice::Wasapi(ref dev) => Box::new(wasapidevice::WasapiCaptureDevice {
            devname: dev.device.clone(),
            samplerate: conf.samplerate,
            exclusive: dev.is_exclusive(),
            loopback: dev.is_loopback(),
            resampler_config: conf.resampler,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels: dev.channels,
            sample_format: dev.format,
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
        #[cfg(all(
            feature = "cpal-backend",
            feature = "jack-backend",
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd"
            )
        ))]
        config::CaptureDevice::Jack {
            channels,
            ref device,
        } => Box::new(cpaldevice::CpalCaptureDevice {
            devname: device.clone(),
            host: cpaldevice::CpalHost::Jack,
            samplerate: conf.samplerate,
            resampler_config: conf.resampler,
            capture_samplerate,
            chunksize: conf.chunksize,
            channels,
            sample_format: config::SampleFormat::FLOAT32LE,
            silence_threshold: conf.silence_threshold(),
            silence_timeout: conf.silence_timeout(),
            stop_on_rate_change: conf.stop_on_rate_change(),
            rate_measure_interval: conf.rate_measure_interval(),
        }),
    }
}

pub fn calculate_speed(avg_level: f64, target_level: usize, adjust_period: f32, srate: u32) -> f64 {
    let diff = avg_level as isize - target_level as isize;
    let rel_diff = (diff as f64) / (srate as f64);
    let speed = 1.0 - 0.5 * rel_diff / adjust_period as f64;
    debug!(
        "Avg. buffer level: {:.1}, target level: {:.1}, corrected capture rate: {:.4}%, ({:+.1}Hz at {}Hz)",
        avg_level,
        target_level,
        100.0 * speed,
        srate as f64 * (speed-1.0),
        srate
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
        let stats = chunk.stats();
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
