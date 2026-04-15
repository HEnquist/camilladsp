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
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/en-US/MPL/2.0/>.

use crate::PrcFmt;
use crate::Res;
use crate::config;
use crate::filters::Filter;
use crate::utils::decibels::db_to_linear;
use crate::utils::time::{time_to_samples, time_to_seconds};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct LookaheadLimiter {
    pub name: String,
    pub limit: PrcFmt,
    pub attack: usize,
    pub samplerate: usize,
    alpha: PrcFmt,
    epsilon: PrcFmt,
    lookahead_buffer: VecDeque<PrcFmt>,
    release_gain: PrcFmt,
    gain_buffer: Vec<PrcFmt>,
    output_buffer: Vec<PrcFmt>,
}

impl LookaheadLimiter {
    pub fn from_config(
        name: &str,
        config: config::LookaheadLimiterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let limit = db_to_linear(config.limit);
        let unit = config.unit();
        let attack = time_to_samples(config.attack, unit, samplerate) as usize;
        // When release gain reduction is less than -80dB, just pass the signal through
        let epsilon = 10f64.powf(-80.0 / 20.0);
        let alpha = epsilon
            .powf(1.0 / (samplerate as PrcFmt * time_to_seconds(config.release, unit, samplerate)));

        if attack > samplerate {
            panic!(
                "Lookahead limiter attack time must not exceed 1 second ({} samples > {} samplerate)",
                attack, samplerate
            );
        }

        debug!(
            "Creating lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release: {} samples, alpha: {}",
            name,
            config.limit,
            limit,
            attack,
            time_to_samples(config.release, unit, samplerate),
            alpha
        );

        LookaheadLimiter {
            name: name.to_string(),
            limit,
            attack,
            samplerate,
            alpha,
            epsilon,
            lookahead_buffer: vec![0.0; samplerate].into(),
            release_gain: 1.0,
            gain_buffer: vec![1.0 as PrcFmt; attack + chunksize],
            output_buffer: vec![0.0 as PrcFmt; chunksize],
        }
    }

    fn apply_lookahead_limiter(&mut self, input: &mut [PrcFmt]) {
        let n = input.len();
        if n == 0 {
            return;
        }

        let get_sample = |i: usize, buf: &VecDeque<PrcFmt>, inp: &[PrcFmt]| -> PrcFmt {
            if i < self.attack {
                buf[self.samplerate - self.attack + i]
            } else {
                inp[i - self.attack]
            }
        };

        // Compute gain reduction curve like a simple peak limiter
        for i in 0..(self.attack + n) {
            let sample_abs = get_sample(i, &self.lookahead_buffer, input).abs();
            if sample_abs > self.limit {
                self.gain_buffer[i] = self.limit / sample_abs;
            } else {
                self.gain_buffer[i] = 1.0;
            }
        }

        // Backward pass turning peaks into linear ramps.
        let mut attack_peak = 1.0;
        let mut samples_since_attack_peak = self.attack;
        for i in (0..(self.attack + n)).rev() {
            let mut gain = 1.0;
            if samples_since_attack_peak <= self.attack {
                let ramp = (self.attack - samples_since_attack_peak) as PrcFmt
                    / (self.attack + 1) as PrcFmt;
                gain = 1.0 - (ramp * attack_peak);
                samples_since_attack_peak += 1;
            }
            if self.gain_buffer[i] < gain {
                gain = self.gain_buffer[i];
                attack_peak = self.gain_buffer[i];
                samples_since_attack_peak = 0;
            }
            self.gain_buffer[i] = gain;
        }

        // Forward pass turning peaks into exponential decay.
        for i in 0..n {
            self.release_gain = 1.0 - (1.0 - self.release_gain) * self.alpha;
            if self.release_gain > 1.0 - self.epsilon {
                self.release_gain = 1.0
            }
            if self.gain_buffer[i] < self.release_gain {
                self.release_gain = self.gain_buffer[i];
            } else {
                self.gain_buffer[i] = self.release_gain;
            }
        }

        let delayed_samples = &mut self.output_buffer[..n];
        for i in 0..n {
            delayed_samples[i] = get_sample(i, &self.lookahead_buffer, input);
        }
        self.lookahead_buffer.drain(0..n);
        for sample in &mut *input {
            self.lookahead_buffer.push_back(*sample);
        }
        for i in 0..n {
            input[i] = delayed_samples[i] * self.gain_buffer[i];
        }
    }
}

impl Filter for LookaheadLimiter {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        self.apply_lookahead_limiter(waveform);
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Filter) {
        if let config::Filter::LookaheadLimiter {
            parameters: config, ..
        } = config
        {
            let new_attack =
                time_to_samples(config.attack, config.unit(), self.samplerate) as usize;
            if new_attack > self.samplerate {
                panic!(
                    "Lookahead limiter attack time exceeds 1 second ({} samples > {} samplerate)",
                    new_attack, self.samplerate
                );
            }
            self.limit = db_to_linear(config.limit);
            self.attack = new_attack;
            let release = time_to_samples(config.release, config.unit(), self.samplerate) as usize;
            let epsilon = 10f64.powf(-80.0 / 20.0);
            self.alpha = epsilon.powf(1.0 / release as PrcFmt);

            debug!(
                "Updated lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release: {} samples, alpha: {}",
                self.name, config.limit, self.limit, self.attack, release, self.alpha
            );
        } else {
            panic!("Invalid config change!");
        }
    }
}

pub fn validate_config(config: &config::LookaheadLimiterParameters) -> Res<()> {
    if config.attack <= 0.0 {
        let msg = "Attack time must be positive.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.release < 0.0 {
        let msg = "Release time must be non-negative.";
        return Err(config::ConfigError::new(msg).into());
    }
    let unit = config.unit();
    match unit {
        config::TimeUnit::Microseconds => {
            if config.attack > 1_000_000.0 {
                let msg = "Attack time must not exceed 1 second.";
                return Err(config::ConfigError::new(msg).into());
            }
        }
        config::TimeUnit::Milliseconds => {
            if config.attack > 1000.0 {
                let msg = "Attack time must not exceed 1 second.";
                return Err(config::ConfigError::new(msg).into());
            }
        }
        config::TimeUnit::Millimetres => {
            let seconds = config.attack / 1000.0 / 343.0;
            if seconds > 1.0 {
                let msg = "Attack time must not exceed 1 second.";
                return Err(config::ConfigError::new(msg).into());
            }
        }
        config::TimeUnit::Samples => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TimeUnit;

    fn assert_close(left: &[PrcFmt], right: &[PrcFmt], epsilon: PrcFmt) {
        assert_eq!(left.len(), right.len());
        for (i, (&l, &r)) in left.iter().zip(right.iter()).enumerate() {
            if (l - r).abs() > epsilon {
                panic!(
                    "Mismatch at index {i}: left={l}, right={r}, diff={}\nleft:   {left:?}\nright: {right:?}",
                    l - r
                );
            }
        }
    }

    #[test]
    fn test_no_limiting_below_threshold() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 4.0,
            release: 4.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![0.5, 0.5, 0.5];
        let expected = vec![0.5, 0.5, 0.5];
        limiter.apply_lookahead_limiter(&mut input);
        assert_close(&input, &expected, 1e-9);
    }

    #[test]
    fn test_limiter_basic() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 4.0,
            release: 4.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, 2.0, -2.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            1.0, 1.0,
        ];
        let expected = vec![
            0.0, 0.0, 0.0, 0.0, 1.0, 0.9, 0.8, 0.7, 0.6, 1.0, -1.0, 0.7, 0.6, 1.0, 0.95, 0.995,
            0.9995, 1.0, 1.0,
        ];
        limiter.apply_lookahead_limiter(&mut input);
        assert_close(&input, &expected, 1e-6);
    }

    /// Zero attack and release should behave like a peak limiter
    #[test]
    fn test_limiter_peak() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 0.0,
            release: 0.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![
            1.0, 1.0, 1.0, 1.0, 2.0, -2.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        let expected = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        limiter.apply_lookahead_limiter(&mut input);
        assert_close(&input, &expected, 1e-6);
    }

    #[test]
    fn test_limiter_zero_release() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 4.0,
            release: 0.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![2.0, 2.0, 2.0, 2.0, 2.0];
        limiter.apply_lookahead_limiter(&mut input);
        for &val in &input {
            assert!(val.abs() <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn test_limiter_state_persistence() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 4.0,
            release: 4.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut buf1 = vec![1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0];
        let expected1 = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.9, 0.8, 0.7, 0.6, 1.0];
        limiter.apply_lookahead_limiter(&mut buf1);
        assert_close(&buf1, &expected1, 1e-6);

        let mut buf2 = vec![1.0, 1.0, 1.0, 1.0];
        let expected2 = vec![0.95, 0.995, 0.9995, 1.0];
        limiter.apply_lookahead_limiter(&mut buf2);
        assert_close(&buf2, &expected2, 1e-6);
    }
}
