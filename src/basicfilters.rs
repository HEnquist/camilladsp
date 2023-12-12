use std::sync::Arc;

use circular_queue::CircularQueue;

use crate::audiodevice::AudioChunk;
use crate::biquad::{Biquad, BiquadCoefficients};
use crate::config;
use crate::filters::Filter;

use crate::NewValue;
use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::Res;

#[derive(Clone, Debug)]
pub struct Gain {
    pub name: String,
    pub gain: PrcFmt,
}

pub struct Delay {
    pub name: String,
    samplerate: usize,
    queue: CircularQueue<PrcFmt>,
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
    processing_params: Arc<ProcessingParameters>,
    fader: usize,
}

impl Volume {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        ramp_time_ms: f32,
        current_volume: f32,
        mute: bool,
        chunksize: usize,
        samplerate: usize,
        processing_params: Arc<ProcessingParameters>,
        fader: usize,
    ) -> Self {
        let name = name.to_string();
        let ramptime_in_chunks =
            (ramp_time_ms / (1000.0 * chunksize as f32 / samplerate as f32)).round() as usize;
        let current_volume_with_mute = if mute { -100.0 } else { current_volume };
        let target_linear_gain = if mute {
            0.0
        } else {
            let tempgain: PrcFmt = 10.0;
            tempgain.powf(current_volume as PrcFmt / 20.0)
        };
        Self {
            name,
            ramptime_in_chunks,
            current_volume: current_volume_with_mute as PrcFmt,
            ramp_start: current_volume as PrcFmt,
            target_volume: current_volume,
            target_linear_gain,
            mute,
            ramp_step: 0,
            samplerate,
            chunksize,
            processing_params,
            fader,
        }
    }

    pub fn from_config(
        name: &str,
        conf: config::VolumeParameters,
        chunksize: usize,
        samplerate: usize,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        let fader = conf.fader as usize;
        let current_volume = processing_params.current_volume(fader);
        let mute = processing_params.is_mute(fader);
        Self::new(
            name,
            conf.ramp_time(),
            current_volume,
            mute,
            chunksize,
            samplerate,
            processing_params,
            fader,
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
                (PrcFmt::coerce(10.0)).powf(
                    (self.ramp_start
                        + ramprange * (self.ramp_step as PrcFmt - 1.0)
                        + val as PrcFmt * stepsize)
                        / 20.0,
                )
            })
            .collect()
    }

    fn prepare_processing(&mut self) {
        let shared_vol = self.processing_params.target_volume(self.fader);
        let shared_mute = self.processing_params.is_mute(self.fader);

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
    }

    pub fn process_chunk(&mut self, chunk: &mut AudioChunk) {
        self.prepare_processing();

        // Not in a ramp
        if self.ramp_step == 0 {
            for waveform in chunk.waveforms.iter_mut() {
                for item in waveform.iter_mut() {
                    *item *= self.target_linear_gain;
                }
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
            for waveform in chunk.waveforms.iter_mut() {
                for (item, stepgain) in waveform.iter_mut().zip(ramp.iter()) {
                    *item *= *stepgain;
                }
            }
            self.current_volume = 20.0 * ramp.last().unwrap().log10();
        }

        // Update shared current volume
        self.processing_params
            .set_current_volume(self.fader, self.current_volume as f32);
    }
}

impl Filter for Volume {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        self.prepare_processing();

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

        // Update shared current volume
        self.processing_params
            .set_current_volume(self.fader, self.current_volume as f32);
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Volume {
            parameters: conf, ..
        } = conf
        {
            self.ramptime_in_chunks = (conf.ramp_time()
                / (1000.0 * self.chunksize as f32 / self.samplerate as f32))
                .round() as usize;
            self.fader = conf.fader as usize;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

fn calculate_gain(gain_value: PrcFmt, inverted: bool, mute: bool, linear: bool) -> PrcFmt {
    let mut gain = if linear {
        gain_value
    } else {
        (10.0 as PrcFmt).powf(gain_value / 20.0)
    };
    if inverted {
        gain = -gain;
    }
    if mute {
        gain = 0.0;
    }
    gain
}

impl Gain {
    /// A simple filter providing gain in dB, and can also invert the signal.
    pub fn new(name: &str, gain_value: PrcFmt, inverted: bool, mute: bool, linear: bool) -> Self {
        let name = name.to_string();
        let gain = calculate_gain(gain_value, inverted, mute, linear);
        Gain { name, gain }
    }

    pub fn from_config(name: &str, conf: config::GainParameters) -> Self {
        let gain = conf.gain;
        let inverted = conf.is_inverted();
        let mute = conf.is_mute();
        let linear = conf.scale() == config::GainScale::Linear;
        Gain::new(name, gain, inverted, mute, linear)
    }
}

impl Filter for Gain {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        for item in waveform.iter_mut() {
            *item *= self.gain;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Gain {
            parameters: conf, ..
        } = conf
        {
            let gain_value = conf.gain;
            let inverted = conf.is_inverted();
            let mute = conf.is_mute();
            let linear = conf.scale() == config::GainScale::Linear;
            let gain = calculate_gain(gain_value, inverted, mute, linear);
            self.gain = gain;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl Delay {
    /// Creates a delay filter with delay in samples
    pub fn new(name: &str, samplerate: usize, delay: PrcFmt, subsample: bool) -> Self {
        let name = name.to_string();

        let (integerdelay, biquad) = if subsample {
            let samples = delay.floor();
            let fraction = delay - samples;
            let bqcoeffs = BiquadCoefficients::new(1.0 - fraction, 0.0, 1.0 - fraction, 1.0, 0.0);
            let bq = Biquad::new("subsample", samplerate, bqcoeffs);
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

        // for super-small delays, store at least a single sample
        let integerdelay = integerdelay.max(1);
        let mut queue = CircularQueue::with_capacity(integerdelay);
        for _ in 0..integerdelay {
            queue.push(0.0);
        }

        Self {
            name,
            samplerate,
            queue,
            biquad,
        }
    }

    pub fn from_config(name: &str, samplerate: usize, conf: config::DelayParameters) -> Self {
        let delay_samples = match conf.unit() {
            config::TimeUnit::Milliseconds => conf.delay / 1000.0 * (samplerate as PrcFmt),
            config::TimeUnit::Millimetres => conf.delay / 1000.0 * (samplerate as PrcFmt) / 343.0,
            config::TimeUnit::Samples => conf.delay,
        };

        Self::new(name, samplerate, delay_samples, conf.subsample())
    }
}

impl Filter for Delay {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        for item in waveform.iter_mut() {
            // this returns the item that was popped while pushing
            *item = self.queue.push(*item).unwrap();
        }
        if let Some(bq) = &mut self.biquad {
            bq.process_waveform(waveform)?;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Delay { parameters, .. } = conf {
            *self = Self::from_config(&self.name, self.samplerate, parameters);
        } else {
            // This should never happen unless there is a bug somewhere else
            unreachable!("Invalid config change!");
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
    if conf.ramp_time() < 0.0 {
        return Err(config::ConfigError::new("Ramp time cannot be negative").into());
    }
    Ok(())
}

/// Validate a Gain config.
pub fn validate_gain_config(conf: &config::GainParameters) -> Res<()> {
    if conf.scale() == config::GainScale::Decibel {
        if conf.gain < -150.0 {
            return Err(config::ConfigError::new("Gain must be larger than -150 dB").into());
        } else if conf.gain > 150.0 {
            return Err(config::ConfigError::new("Gain must be less than +150 dB").into());
        }
    } else if conf.gain < -10.0 {
        return Err(config::ConfigError::new("Linear gain must be larger than -10.0").into());
    } else if conf.gain > 10.0 {
        return Err(config::ConfigError::new("Linear gain must be less than +10.0").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::basicfilters::{Delay, Gain};
    use crate::filters::Filter;
    use crate::PrcFmt;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
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
        let mut gain = Gain::new("test", 0.0, true, false, false);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_inv);
    }

    #[test]
    fn gain_ampl() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_ampl = vec![-5.0, 0.0, 5.0];
        let mut gain = Gain::new("test", 20.0, false, false, false);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_ampl);
    }

    #[test]
    fn delay_small() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![0.0, 0.0, 0.0, 0.0, -0.5, 1.0, 0.0, 0.0];
        let mut delay = Delay::new("test", 44100, 3.0, false);
        delay.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_supersmall() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut delay = Delay::new("test", 44100, 0.1, false);
        delay.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_large() {
        let mut waveform1 = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut waveform2 = vec![0.0; 8];
        let waveform_delayed = vec![0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut delay = Delay::new("test", 44100, 9.0, false);
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
        let mut delay = Delay::new("test", 44100, 1.7, true);
        delay.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(waveform, waveform_delayed, 1.0e-6));
    }
}
