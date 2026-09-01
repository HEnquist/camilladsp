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

use std::sync::Arc;

use ringbuf::LocalRb;
use ringbuf::storage::Heap;
use ringbuf::traits::*;

use crate::audiochunk::AudioChunk;
use crate::config;
use crate::filters::Filter;
use crate::filters::biquad::{Biquad, BiquadCoefficients};

use crate::CamillaFloat;
use crate::ProcessingParameters;
use crate::Res;
use crate::ToCamillaFloat;
use crate::ToF32;
use crate::utils::decibels::{db_to_linear, gain_from_value};
use crate::utils::time::delay_to_samples;

#[derive(Clone, Debug)]
pub struct Gain {
    pub name: String,
    pub gain: CamillaFloat,
}

pub struct Delay {
    pub name: String,
    samplerate: usize,
    queue: Option<LocalRb<Heap<CamillaFloat>>>,
    biquad: Option<Biquad>,
}

pub struct Volume {
    pub name: String,
    ramptime_in_chunks: usize,
    /// Level in dB. Control value, not audio, and f32 at both ends of the API.
    current_volume: f32,
    target_volume: f32,
    target_linear_gain: CamillaFloat,
    mute: bool,
    /// Level in dB at the start of the current ramp.
    ramp_start: f32,
    ramp_step: usize,
    samplerate: usize,
    chunksize: usize,
    processing_params: Arc<ProcessingParameters>,
    fader: usize,
    volume_limit: f32,
    /// Value of the shared pause counter when this filter last ran, used to detect
    /// whether audio flow was interrupted since the previous chunk.
    last_pause_count: u64,
}

impl Volume {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        ramp_time_ms: f32,
        limit: f32,
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
        // Start in sync with the shared counter, so the first chunk is not mistaken
        // for a resume after a pause.
        let last_pause_count = processing_params.pause_count();
        let current_volume_with_mute = if mute { -100.0 } else { current_volume };
        let target_linear_gain = if mute {
            0.0
        } else {
            db_to_linear(current_volume as f64).to_camilla_float()
        };
        Self {
            name,
            ramptime_in_chunks,
            current_volume: current_volume_with_mute,
            ramp_start: current_volume,
            target_volume: current_volume,
            target_linear_gain,
            mute,
            ramp_step: 0,
            samplerate,
            chunksize,
            processing_params,
            fader,
            volume_limit: limit,
            last_pause_count,
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
        let current_volume = processing_params.target_volume(fader);
        let mute = processing_params.is_mute(fader);
        Self::new(
            name,
            conf.ramp_time_ms(),
            conf.limit(),
            current_volume,
            mute,
            chunksize,
            samplerate,
            processing_params,
            fader,
        )
    }

    fn make_ramp(&self) -> Vec<CamillaFloat> {
        let target_volume = if self.mute {
            -100.0
        } else {
            self.target_volume
        };

        // The ramp is laid out in dB, at the f32 precision the levels are kept
        // in, and only the resulting gain factors cross into the processing
        // precision, since those get multiplied into the samples.
        let ramprange = (target_volume - self.ramp_start) / self.ramptime_in_chunks as f32;
        let stepsize = ramprange / self.chunksize as f32;
        (0..self.chunksize)
            .map(|val| {
                let level_db = self.ramp_start
                    + ramprange * (self.ramp_step as f32 - 1.0)
                    + val as f32 * stepsize;
                db_to_linear(level_db as f64).to_camilla_float()
            })
            .collect()
    }

    fn prepare_processing(&mut self) {
        let shared_vol = self.processing_params.target_volume(self.fader);
        let shared_mute = self.processing_params.is_mute(self.fader);

        // are we above the set limit?
        let target_volume = shared_vol.min(self.volume_limit);

        // Did audio flow stop between the previous chunk and this one? If so, any volume
        // change we are seeing now was made while paused, and ramping it would fade in
        // from a level that is no longer relevant. Track this on every chunk, so that a
        // pause is only ever attributed to the chunk that directly follows it.
        let pause_count = self.processing_params.pause_count();
        let resumed_after_pause = pause_count != self.last_pause_count;
        self.last_pause_count = pause_count;

        // Volume setting changed
        if (target_volume - self.target_volume).abs() > 0.01 || self.mute != shared_mute {
            if self.ramptime_in_chunks > 0 && !resumed_after_pause {
                trace!(
                    "starting ramp: {} -> {}, mute: {}",
                    self.current_volume, target_volume, shared_mute
                );
                self.ramp_start = self.current_volume;
                self.ramp_step = 1;
            } else {
                trace!(
                    "switch volume without ramp: {} -> {}, mute: {}",
                    self.current_volume, target_volume, shared_mute
                );
                self.current_volume = if shared_mute { 0.0 } else { target_volume };
                self.ramp_step = 0;
            }
            self.target_volume = target_volume;
            self.target_linear_gain = if shared_mute {
                0.0
            } else {
                db_to_linear(target_volume as f64).to_camilla_float()
            };
            self.mute = shared_mute;
        }
    }

    pub fn process_chunk(&mut self, chunk: &mut AudioChunk) {
        self.prepare_processing();

        // Not in a ramp
        if self.ramp_step == 0 {
            xtrace!("Vol: applying linear gain {}", self.target_linear_gain);
            for waveform in chunk.waveforms.iter_mut() {
                for item in waveform.iter_mut() {
                    *item *= self.target_linear_gain;
                }
            }
        }
        // Ramping
        else if self.ramp_step <= self.ramptime_in_chunks {
            trace!("Vol: ramp step {}", self.ramp_step);
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
            self.current_volume = 20.0 * ramp.last().unwrap().to_f32().log10();
        }

        // Update shared current volume. `Loudness` reads this to size its
        // compensation, which makes the two filters order-sensitive across
        // channels. See `parallelize_filters` in pipeline.rs, which changes
        // that order and says why the one chunk of lag is accepted. Anything
        // else that comes to read or write shared state here inherits the
        // same question.
        self.processing_params
            .set_current_volume(self.fader, self.current_volume.to_f32());
    }
}

impl Filter for Volume {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
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
            self.current_volume = 20.0 * ramp.last().unwrap().to_f32().log10();
        }

        // Update shared current volume. `Loudness` reads this to size its
        // compensation, which makes the two filters order-sensitive across
        // channels. See `parallelize_filters` in pipeline.rs, which changes
        // that order and says why the one chunk of lag is accepted. Anything
        // else that comes to read or write shared state here inherits the
        // same question.
        self.processing_params
            .set_current_volume(self.fader, self.current_volume.to_f32());
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Volume {
            parameters: conf, ..
        } = conf
        {
            self.ramptime_in_chunks = (conf.ramp_time_ms()
                / (1000.0 * self.chunksize as f32 / self.samplerate as f32))
                .round() as usize;
            self.fader = conf.fader as usize;
            self.volume_limit = conf.limit();
            if self.volume_limit < self.current_volume {
                self.current_volume = self.volume_limit;
            }
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl Gain {
    /// A simple filter providing gain in dB, and can also invert the signal.
    pub fn new(name: &str, gain_value: f64, inverted: bool, mute: bool, linear: bool) -> Self {
        let name = name.to_string();
        let gain = gain_from_value(gain_value, linear, inverted, mute).to_camilla_float();
        Gain { name, gain }
    }

    pub fn from_config(name: &str, conf: config::GainParameters) -> Self {
        let gain = conf.gain;
        let inverted = conf.is_inverted();
        let mute = conf.is_mute();
        let linear = conf.scale() == config::GainScale::Linear;
        Gain::new(name, gain, inverted, mute, linear)
    }

    pub fn process_single(&self, value: CamillaFloat) -> CamillaFloat {
        value * self.gain
    }
}

impl Filter for Gain {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        for item in waveform.iter_mut() {
            *item *= self.gain;
        }
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
            let gain = gain_from_value(gain_value, linear, inverted, mute).to_camilla_float();
            self.gain = gain;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

fn build_subsample_biquad(delay: f64, samplerate: usize) -> (usize, Option<Biquad>) {
    // delay is less than 0.1 samples, ignore
    if delay < 0.1 {
        debug!("Delay too small, ignoring");
        return (0, None);
    }
    // delay is less than 1.1 samples, use first order allpass
    if delay < 1.1 {
        let coeff = (1.0 - delay) / (1.0 + delay);
        debug!("Using first order allpass for delay of {delay:.2} samples");
        // 1st order Thiran allpass
        let bqcoeffs = BiquadCoefficients::new(coeff, 0.0, coeff, 1.0, 0.0);
        trace!("Coefficients: {bqcoeffs:?}");
        return (0, Some(Biquad::new("subsample", samplerate, bqcoeffs)));
    }

    // delay is large enough to use a second order allpass
    let mut samples = delay.floor();
    let mut fraction = delay - samples;
    // adjust fraction and samples to keep fraction between 1.1 and 2.1
    samples -= 1.0;
    fraction += 1.0;
    if fraction < 1.1 {
        samples -= 1.0;
        fraction += 1.0;
    }
    // 2nd order Thiran allpass
    debug!("Using second order allpass for delay of {samples} + {fraction:.2} samples");
    let coeff1 = 2.0 * (2.0 - fraction) / (1.0 + fraction);
    let coeff2 = (2.0 - fraction) / (2.0 + fraction) * (1.0 - fraction) / (1.0 + fraction);
    let bqcoeffs = BiquadCoefficients::new(coeff1, coeff2, coeff2, coeff1, 1.0);
    trace!("Coefficients: {bqcoeffs:?}");
    (
        samples as usize,
        Some(Biquad::new("subsample", samplerate, bqcoeffs)),
    )
}

impl Delay {
    /// Creates a delay filter with delay in samples
    pub fn new(name: &str, samplerate: usize, delay: f64, subsample: bool) -> Self {
        let name = name.to_string();

        let (integerdelay, biquad) = if subsample {
            let (samples, bq) = build_subsample_biquad(delay, samplerate);
            debug!(
                "Building delay filter '{}' with delay {} + {:.2} samples",
                name,
                samples,
                delay - samples as f64
            );
            (samples, bq)
        } else {
            let samples = delay.round() as usize;
            debug!("Building delay filter '{name}' with delay {samples} samples");
            (samples, None)
        };

        let queue = if integerdelay > 0 {
            let mut q = LocalRb::new(integerdelay);
            for _ in 0..integerdelay {
                let _ = q.try_push(0.0);
            }
            Some(q)
        } else {
            None
        };

        Self {
            name,
            samplerate,
            queue,
            biquad,
        }
    }

    pub fn from_config(name: &str, samplerate: usize, conf: config::DelayParameters) -> Self {
        let delay_samples = delay_to_samples(conf.delay, conf.delay_unit(), samplerate);

        Self::new(name, samplerate, delay_samples, conf.subsample())
    }

    pub fn process_single(&mut self, input: CamillaFloat) -> CamillaFloat {
        let mut value = if let Some(q) = &mut self.queue {
            q.push_overwrite(input).unwrap()
        } else {
            input
        };
        if let Some(bq) = &mut self.biquad {
            value = bq.process_single(value);
        }
        value
    }
}

impl Filter for Delay {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        if let Some(q) = &mut self.queue {
            for item in waveform.iter_mut() {
                // this returns the item that was popped while pushing
                *item = q.push_overwrite(*item).unwrap();
            }
        }
        if let Some(bq) = &mut self.biquad {
            bq.process_waveform(waveform);
        }
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
    if conf.ramp_time_ms() < 0.0 {
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
    use crate::CamillaFloat;
    use crate::ProcessingParameters;
    use crate::filters::Filter;
    use crate::filters::basicfilters::{Delay, Gain, Volume};
    use std::sync::Arc;

    fn is_close(left: CamillaFloat, right: CamillaFloat, maxdiff: CamillaFloat) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(
        left: Vec<CamillaFloat>,
        right: Vec<CamillaFloat>,
        maxdiff: CamillaFloat,
    ) -> bool {
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
        gain.process_waveform(&mut waveform);
        assert_eq!(waveform, waveform_inv);
    }

    #[test]
    fn gain_ampl() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_ampl = vec![-5.0, 0.0, 5.0];
        let mut gain = Gain::new("test", 20.0, false, false, false);
        gain.process_waveform(&mut waveform);
        assert_eq!(waveform, waveform_ampl);
    }

    #[test]
    fn delay_small() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![0.0, 0.0, 0.0, 0.0, -0.5, 1.0, 0.0, 0.0];
        let mut delay = Delay::new("test", 44100, 3.0, false);
        delay.process_waveform(&mut waveform);
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_supersmall() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = waveform.clone();
        let mut delay = Delay::new("test", 44100, 0.1, false);
        delay.process_waveform(&mut waveform);
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_large() {
        let mut waveform1 = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut waveform2 = vec![0.0; 8];
        let waveform_delayed = vec![0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut delay = Delay::new("test", 44100, 9.0, false);
        delay.process_waveform(&mut waveform1);
        delay.process_waveform(&mut waveform2);
        assert_eq!(waveform1, vec![0.0; 8]);
        assert_eq!(waveform2, waveform_delayed);
    }

    #[test]
    fn delay_fraction() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected_waveform = vec![
            0.0,
            0.01051051051051051,
            -0.13446780113446782,
            -0.2476751025299573,
            1.0522122611990257,
            -0.23903133046978262,
            0.07523664949897024,
            -0.021743938066703532,
            0.006413537427714274,
            -0.001882310318672015,
        ];
        let mut delay = Delay::new("test", 44100, 1.7, true);
        delay.process_waveform(&mut waveform);
        assert!(compare_waveforms(waveform, expected_waveform, 1.0e-6));
    }

    const VOL_CHUNKSIZE: usize = 1024;
    const VOL_SAMPLERATE: usize = 44100;

    /// Build a volume filter with a ramp time of `ramp_chunks` chunks, at 0 dB.
    fn make_volume(params: &Arc<ProcessingParameters>, ramp_chunks: f32) -> Volume {
        make_volume_full(params, ramp_chunks, 50.0, 0.0, false, VOL_CHUNKSIZE)
    }

    /// Build a volume filter, with the ramp time expressed as a number of chunks.
    fn make_volume_full(
        params: &Arc<ProcessingParameters>,
        ramp_chunks: f32,
        limit: f32,
        current_volume: f32,
        mute: bool,
        chunksize: usize,
    ) -> Volume {
        let ramp_time_ms = 1000.0 * (chunksize as f32) / (VOL_SAMPLERATE as f32) * ramp_chunks;
        Volume::new(
            "volume",
            ramp_time_ms,
            limit,
            current_volume,
            mute,
            chunksize,
            VOL_SAMPLERATE,
            params.clone(),
            0,
        )
    }

    fn gain_at(db: CamillaFloat) -> CamillaFloat {
        (10.0 as CamillaFloat).powf(db / 20.0)
    }

    /// A volume change made while audio is flowing is ramped, and the ramp
    /// decreases monotonically until it settles at the target.
    #[test]
    fn volume_ramps_while_running() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume(&params, 2.0);

        // No change yet, unity gain.
        let mut chunk = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk);
        assert!(chunk.iter().all(|s| (s - 1.0).abs() < 1e-10));

        params.set_target_volume(0, -20.0);

        // First ramp chunk: stays inside the range and falls across the chunk.
        let mut chunk1 = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk1);
        assert!(chunk1.iter().all(|s| *s <= gain_at(0.0) + 1e-6));
        assert!(chunk1.iter().all(|s| *s >= gain_at(-20.0) - 1e-6));
        assert!(chunk1[0] > chunk1[VOL_CHUNKSIZE - 1]);

        // Second ramp chunk continues downwards.
        let mut chunk2 = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk2);
        assert!(chunk2[VOL_CHUNKSIZE - 1] < chunk1[VOL_CHUNKSIZE - 1]);
        assert!(chunk2[VOL_CHUNKSIZE - 1] >= gain_at(-20.0) - 1e-6);

        // Ramp is done, the target is applied flat.
        let mut chunk3 = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk3);
        assert!(chunk3.iter().all(|s| (s - gain_at(-20.0)).abs() < 1e-6));
    }

    /// A volume change made while paused is applied directly on resume, with no ramp.
    #[test]
    fn volume_change_during_pause_is_not_ramped() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume(&params, 2.0);

        let mut chunk = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk);

        // Audio stops, volume is changed while paused, then playback resumes.
        params.bump_pause_count();
        params.bump_pause_count();
        params.set_target_volume(0, -20.0);

        let mut resumed = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut resumed);
        assert!(resumed.iter().all(|s| (s - gain_at(-20.0)).abs() < 1e-6));
    }

    /// A pause is only attributed to the chunk right after it, so a change made
    /// later, while running again, is still ramped.
    #[test]
    fn volume_ramps_again_after_resuming() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume(&params, 2.0);

        let mut chunk = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk);

        // A pause happens, but no volume change is made during it.
        params.bump_pause_count();
        let mut resumed = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut resumed);

        // Now change the volume while running, this must ramp.
        params.set_target_volume(0, -20.0);
        let mut chunk1 = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk1);
        assert!(chunk1[0] > chunk1[VOL_CHUNKSIZE - 1]);
        assert!(chunk1[VOL_CHUNKSIZE - 1] > gain_at(-20.0) + 1e-6);
    }

    /// With no ramp time configured, changes are always applied directly.
    #[test]
    fn volume_without_ramptime_switches_directly() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume(&params, 0.0);

        let mut chunk = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk);

        params.set_target_volume(0, -20.0);
        let mut chunk1 = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk1);
        assert!(chunk1.iter().all(|s| (s - gain_at(-20.0)).abs() < 1e-6));
    }

    /// A mute toggled while paused is applied directly on resume, like a volume change.
    #[test]
    fn mute_during_pause_is_not_ramped() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume(&params, 2.0);

        let mut chunk = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut chunk);

        params.bump_pause_count();
        params.set_mute(0, true);

        let mut resumed = vec![1.0; VOL_CHUNKSIZE];
        filter.process_waveform(&mut resumed);
        assert!(resumed.iter().all(|s| s.abs() < 1e-10));
    }

    /// A filter built at a fixed level applies that gain with no ramping.
    #[test]
    fn volume_applies_initial_level() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, -20.0);
        let mut filter = make_volume_full(&params, 0.0, 50.0, -20.0, false, 4);

        let mut waveform: Vec<CamillaFloat> = vec![1.0, -1.0, 0.5, -0.5];
        filter.process_waveform(&mut waveform);

        let gain = gain_at(-20.0);
        let expected: Vec<CamillaFloat> = vec![gain, -gain, 0.5 * gain, -0.5 * gain];
        assert!(compare_waveforms(waveform, expected, 1e-10));
    }

    /// A filter built muted outputs silence.
    #[test]
    fn volume_muted_outputs_silence() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        params.set_mute(0, true);
        let mut filter = make_volume_full(&params, 0.0, 50.0, 0.0, true, 4);

        let mut waveform: Vec<CamillaFloat> = vec![1.0, 0.5, -0.5, -1.0];
        filter.process_waveform(&mut waveform);
        assert!(waveform.iter().all(|s| s.abs() < 1e-10));
    }

    /// Changes smaller than the 0.01 dB detection threshold are ignored.
    #[test]
    fn volume_ignores_changes_below_threshold() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume_full(&params, 0.0, 50.0, 0.0, false, 4);

        let mut wave1: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut wave1);

        // Below the threshold, so the gain stays at unity.
        params.set_target_volume(0, 0.005);
        let mut wave2: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut wave2);
        assert!(wave2.iter().all(|s| (s - 1.0).abs() < 1e-10));

        // Above the threshold, so it is applied.
        params.set_target_volume(0, 0.02);
        let mut wave3: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut wave3);
        assert!(wave3.iter().all(|s| (s - gain_at(0.02)).abs() < 1e-6));
    }

    /// A target above the configured limit is clamped to the limit.
    #[test]
    fn volume_limit_clamps_target() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume_full(&params, 0.0, 10.0, 0.0, false, 4);

        params.set_target_volume(0, 20.0);
        let mut waveform: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut waveform);
        assert!(waveform.iter().all(|s| (s - gain_at(10.0)).abs() < 1e-6));
    }

    /// Ramping must not depend on chunk size or wall clock timing. With a tiny chunk
    /// size the old timestamp based staleness check had a threshold of a few hundred
    /// microseconds, which made this ramp get skipped whenever the test thread was
    /// descheduled between chunks.
    #[test]
    fn volume_ramps_with_tiny_chunksize() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        let mut filter = make_volume_full(&params, 2.0, 50.0, 0.0, false, 4);

        let mut chunk0: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut chunk0);
        assert!(chunk0.iter().all(|s| (s - 1.0).abs() < 1e-10));

        params.set_target_volume(0, -20.0);

        let mut chunk1: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut chunk1);
        assert!(chunk1[0] > chunk1[3]);
        assert!(chunk1.iter().all(|s| *s <= gain_at(0.0) + 1e-6));
        assert!(chunk1.iter().all(|s| *s >= gain_at(-20.0) - 1e-6));

        let mut chunk2: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut chunk2);
        assert!(chunk2[3] < chunk1[3]);

        let mut chunk3: Vec<CamillaFloat> = vec![1.0; 4];
        filter.process_waveform(&mut chunk3);
        assert!(chunk3.iter().all(|s| (s - gain_at(-20.0)).abs() < 1e-6));
    }
}
