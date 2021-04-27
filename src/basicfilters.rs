use std::sync::{Arc, RwLock};

use crate::filters::Filter;
use biquad::{Biquad, BiquadCoefficients};
use config;
use fifoqueue::FifoQueue;

use NewValue;
use PrcFmt;
use ProcessingStatus;
use Res;

#[derive(Clone, Debug)]
pub struct Gain {
    pub name: String,
    pub gain: PrcFmt,
}

pub struct Delay {
    pub name: String,
    samplerate: usize,
    pub queue: FifoQueue<PrcFmt>,
    biquad: Option<Biquad>,
}

pub struct Volume {
    pub name: String,
    ramptime_in_chunks: usize,
    current_volume: PrcFmt,
    target_volume: f32,
    target_linear_gain: PrcFmt,
    mute: bool,
    ramp_start: PrcFmt,
    ramp_step: usize,
    samplerate: usize,
    chunksize: usize,
    processing_status: Arc<RwLock<ProcessingStatus>>,
}

impl Volume {
    pub fn new(
        name: String,
        ramp_time_ms: f32,
        current_volume: f32,
        mute: bool,
        chunksize: usize,
        samplerate: usize,
        processing_status: Arc<RwLock<ProcessingStatus>>,
    ) -> Self {
        let ramptime_in_chunks =
            (ramp_time_ms / (1000.0 * chunksize as f32 / samplerate as f32)).round() as usize;
        let tempgain: PrcFmt = 10.0;
        let target_linear_gain = tempgain.powf(current_volume as PrcFmt / 20.0);
        Volume {
            name,
            ramptime_in_chunks,
            current_volume: current_volume as PrcFmt,
            ramp_start: current_volume as PrcFmt,
            target_volume: current_volume as f32,
            target_linear_gain,
            mute,
            ramp_step: 0,
            samplerate,
            chunksize,
            processing_status,
        }
    }

    pub fn from_config(
        name: String,
        conf: config::VolumeParameters,
        chunksize: usize,
        samplerate: usize,
        processing_status: Arc<RwLock<ProcessingStatus>>,
    ) -> Self {
        let current_volume = processing_status.read().unwrap().volume;
        let mute = processing_status.read().unwrap().mute;
        Volume::new(
            name,
            conf.ramp_time,
            current_volume,
            mute,
            chunksize,
            samplerate,
            processing_status,
        )
    }

    fn make_ramp(&self) -> Vec<PrcFmt> {
        let target_volume = if self.mute {
            -100.0
        } else {
            self.target_volume
        };

        let ramprange =
            (target_volume as PrcFmt - self.ramp_start) / self.ramptime_in_chunks as PrcFmt;
        let stepsize = ramprange / self.chunksize as PrcFmt;
        (0..self.chunksize)
            .map(|val| {
                (PrcFmt::new(10.0)).powf(
                    (self.ramp_start
                        + ramprange * (self.ramp_step as PrcFmt - 1.0)
                        + val as PrcFmt * stepsize)
                        / 20.0,
                )
            })
            .collect()
    }
}

impl Filter for Volume {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        let shared_vol = self.processing_status.read().unwrap().volume;
        let shared_mute = self.processing_status.read().unwrap().mute;

        // Volume setting changed
        if (shared_vol - self.target_volume).abs() > 0.01 || self.mute != shared_mute {
            if self.ramptime_in_chunks > 0 {
                trace!(
                    "starting ramp: {} -> {}, mute: {}",
                    self.current_volume,
                    shared_vol,
                    shared_mute
                );
                self.ramp_start = self.current_volume;
                self.ramp_step = 1;
            } else {
                trace!(
                    "switch volume without ramp: {} -> {}, mute: {}",
                    self.current_volume,
                    shared_vol,
                    shared_mute
                );
                self.current_volume = if shared_mute {
                    0.0
                } else {
                    shared_vol as PrcFmt
                };
                self.ramp_step = 0;
            }
            self.target_volume = shared_vol;
            self.target_linear_gain = if shared_mute {
                0.0
            } else {
                let tempgain: PrcFmt = 10.0;
                tempgain.powf(shared_vol as PrcFmt / 20.0)
            };
            self.mute = shared_mute;
        }

        // Not in a ramp
        if self.ramp_step == 0 {
            for item in waveform.iter_mut() {
                *item *= self.target_linear_gain;
            }
        }
        // Ramping
        else if self.ramp_step <= self.ramptime_in_chunks {
            trace!("ramp step {}", self.ramp_step);
            let ramp = self.make_ramp();
            self.ramp_step += 1;
            if self.ramp_step > self.ramptime_in_chunks {
                // Last step of ramp
                self.ramp_step = 0;
            }
            for (item, stepgain) in waveform.iter_mut().zip(ramp.iter()) {
                *item *= *stepgain;
            }
            self.current_volume = 20.0 * ramp.last().unwrap().log10();
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Volume { parameters: conf } = conf {
            self.ramptime_in_chunks = (conf.ramp_time
                / (1000.0 * self.chunksize as f32 / self.samplerate as f32))
                .round() as usize;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl Gain {
    /// A simple filter providing gain in dB, and can also invert the signal.
    pub fn new(name: String, gain_db: PrcFmt, inverted: bool, mute: bool) -> Self {
        let mut gain: PrcFmt = 10.0;
        gain = gain.powf(gain_db / 20.0);
        if inverted {
            gain = -gain;
        }
        if mute {
            gain = 0.0;
        }
        Gain { name, gain }
    }

    pub fn from_config(name: String, conf: config::GainParameters) -> Self {
        let gain = conf.gain;
        let inverted = conf.inverted;
        let mute = conf.mute;
        Gain::new(name, gain, inverted, mute)
    }
}

impl Filter for Gain {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for item in waveform.iter_mut() {
            *item *= self.gain;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Gain { parameters: conf } = conf {
            let gain_db = conf.gain;
            let inverted = conf.inverted;
            let mut gain: PrcFmt = 10.0;
            gain = gain.powf(gain_db / 20.0);
            if inverted {
                gain = -gain;
            }
            if conf.mute {
                gain = 0.0;
            }
            self.gain = gain;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl Delay {
    /// Creates a delay filter with delay in samples
    /// Will be improved as it gets slow for long delays
    pub fn new(name: String, samplerate: usize, delay: PrcFmt, subsample: bool) -> Self {
        let (integerdelay, biquad) = if subsample {
            let samples = delay.floor();
            let fraction = delay - samples;
            let bqcoeffs = BiquadCoefficients::new(1.0 - fraction, 0.0, 1.0 - fraction, 1.0, 0.0);
            let bq = Biquad::new("subsample".to_string(), 12345, bqcoeffs);
            debug!(
                "Building delay filter '{}' with delay {} + {} samples",
                name, samples, fraction
            );
            (samples as usize, Some(bq))
        } else {
            let samples = delay.round() as usize;
            debug!(
                "Building delay filter '{}' with delay {} samples",
                name, samples
            );
            (samples, None)
        };
        let mut queue = FifoQueue::filled_with(integerdelay + 1, 0.0);
        let _elem = queue.pop();
        Delay {
            name,
            samplerate,
            queue,
            biquad,
        }
    }

    pub fn from_config(name: String, samplerate: usize, conf: config::DelayParameters) -> Self {
        let delay_samples = match conf.unit {
            config::TimeUnit::Milliseconds => conf.delay / 1000.0 * (samplerate as PrcFmt),
            config::TimeUnit::Samples => conf.delay,
        };
        Delay::new(name, samplerate, delay_samples, conf.subsample)
    }
}

impl Filter for Delay {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for item in waveform.iter_mut() {
            self.queue.push(*item)?;
            *item = self.queue.pop().unwrap();
        }
        if let Some(bq) = &mut self.biquad {
            bq.process_waveform(waveform)?;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Delay { parameters: conf } = conf {
            let delay_samples = match conf.unit {
                config::TimeUnit::Milliseconds => conf.delay / 1000.0 * (self.samplerate as PrcFmt),
                config::TimeUnit::Samples => conf.delay,
            };
            let (integerdelay, biquad) = if conf.subsample {
                let full_samples = delay_samples.floor();
                let fraction = delay_samples - full_samples;
                let bqcoeffs =
                    BiquadCoefficients::new(1.0 - fraction, 0.0, 1.0 - fraction, 1.0, 0.0);
                let bq = Biquad::new("subsample".to_string(), 12345, bqcoeffs);
                debug!(
                    "Updating delay filter '{}' with delay {} + {} samples",
                    self.name, full_samples, fraction
                );
                (full_samples as usize, Some(bq))
            } else {
                let full_samples = delay_samples.round() as usize;
                debug!(
                    "Updating delay filter '{}' with delay {} samples",
                    self.name, full_samples
                );
                (full_samples, None)
            };
            let mut queue = FifoQueue::filled_with(integerdelay + 1, 0.0);
            let _elem = queue.pop();
            self.queue = queue;
            self.biquad = biquad;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a Loudness config.
pub fn validate_delay_config(conf: &config::DelayParameters) -> Res<()> {
    if conf.delay < 0.0 {
        return Err(config::ConfigError::new("Delay cannot be negative").into());
    }
    Ok(())
}

/// Validate a Volume config.
pub fn validate_volume_config(conf: &config::VolumeParameters) -> Res<()> {
    if conf.ramp_time < 0.0 {
        return Err(config::ConfigError::new("Ramp time cannot be negative").into());
    }
    Ok(())
}

/// Validate a Gain config.
pub fn validate_gain_config(conf: &config::GainParameters) -> Res<()> {
    if conf.gain < -150.0 {
        return Err(config::ConfigError::new("Gain must be larger than -150 dB").into());
    } else if conf.gain > 150.0 {
        return Err(config::ConfigError::new("Gain must be less than +150 dB").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use basicfilters::{Delay, Gain};
    use filters::Filter;

    fn is_close(left: f64, right: f64, maxdiff: f64) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<f64>, right: Vec<f64>, maxdiff: f64) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn gain_invert() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_inv = vec![0.5, 0.0, -0.5];
        let mut gain = Gain::new("test".to_string(), 0.0, true, false);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_inv);
    }

    #[test]
    fn gain_ampl() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_ampl = vec![-5.0, 0.0, 5.0];
        let mut gain = Gain::new("test".to_string(), 20.0, false, false);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_ampl);
    }

    #[test]
    fn delay_small() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![0.0, 0.0, 0.0, 0.0, -0.5, 1.0, 0.0, 0.0];
        let mut delay = Delay::new("test".to_string(), 44100, 3.0, false);
        delay.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_large() {
        let mut waveform1 = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut waveform2 = vec![0.0; 8];
        let waveform_delayed = vec![0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut delay = Delay::new("test".to_string(), 44100, 9.0, false);
        delay.process_waveform(&mut waveform1).unwrap();
        delay.process_waveform(&mut waveform2).unwrap();
        assert_eq!(waveform1, vec![0.0; 8]);
        assert_eq!(waveform2, waveform_delayed);
    }

    #[test]
    fn delay_fraction() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![
            0.0,
            0.0,
            -0.15,
            -0.15500000000000003,
            1.0465,
            -0.31395,
            0.094185,
            -0.0282555,
            0.008476649999999999,
            -0.0025429949999999997,
        ];
        let mut delay = Delay::new("test".to_string(), 44100, 1.7, true);
        delay.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform, waveform_delayed, 1.0e-6));
    }
}
