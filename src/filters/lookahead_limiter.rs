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
use crate::utils::time::time_to_samples;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct LookaheadLimiter {
    pub name: String,
    pub limit: PrcFmt,
    pub attack: usize,
    pub samplerate: usize,
    epsilon: PrcFmt,
    alpha: PrcFmt,
    lookahead_buffer: VecDeque<PrcFmt>,
    release_gain: PrcFmt,
    output_buffer: Vec<PrcFmt>,
}

impl LookaheadLimiter {
    pub fn from_config(
        name: &str,
        config: config::LookaheadLimiterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let (limit, attack, release, epsilon, alpha) =
            LookaheadLimiter::configure(&config, samplerate);

        debug!(
            "Creating lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release: {} samples, alpha: {}",
            name, config.limit, limit, attack, release, alpha
        );

        LookaheadLimiter {
            name: name.to_string(),
            limit,
            attack,
            samplerate,
            epsilon,
            alpha,
            lookahead_buffer: vec![0.0; samplerate].into(),
            release_gain: 1.0,
            output_buffer: vec![0.0 as PrcFmt; chunksize],
        }
    }

    fn configure(
        config: &config::LookaheadLimiterParameters,
        samplerate: usize,
    ) -> (f64, usize, usize, f64, f64) {
        let limit = db_to_linear(config.limit);

        let unit = config.unit();

        let attack_raw = time_to_samples(config.attack, unit, samplerate) as usize;
        let attack = if attack_raw <= samplerate {
            attack_raw
        } else {
            warn!(
                "Lookahead limiter attack time exceeds 1 second ({} samples > {} samplerate), limiting to 1 second.",
                attack_raw, samplerate
            );
            samplerate
        };

        let release = time_to_samples(config.release, unit, samplerate) as usize;
        // When release gain reduction is less than -80dB, just pass the signal through
        let epsilon = 10f64.powf(-80.0 / 20.0);
        // Release exponential factor
        let alpha = epsilon.powf(1.0 / time_to_samples(config.release, unit, samplerate));

        (limit, attack, release, epsilon, alpha)
    }

    fn apply_lookahead_limiter(&mut self, input: &mut [PrcFmt]) {
        let len = input.len();
        if len == 0 {
            return;
        }

        // Backward pass turning peaks into linear ramps.
        let mut peak = 1.0;
        let mut samples_since_peak = self.attack + 1;
        for i in (0..(self.attack + len)).rev() {
            // Get sample amplitude
            let amplitude = (if i < self.attack {
                self.lookahead_buffer[self.samplerate - self.attack + i]
            } else {
                input[i - self.attack]
            })
            .abs();

            // Compute reduction gain for current sample
            let mut gain = if amplitude > self.limit {
                self.limit / amplitude
            } else {
                1.0
            };

            // Compute ramp
            let mut ramp_gain = 1.0;
            if samples_since_peak <= self.attack {
                let ramp =
                    (self.attack - samples_since_peak) as PrcFmt / self.attack.max(1) as PrcFmt;
                ramp_gain = 1.0 - (ramp * (1.0 - peak));
                samples_since_peak += 1;
            }

            // Peak found, start new ramp
            if gain < ramp_gain {
                peak = gain;
                samples_since_peak = 1;
            } else {
                gain = ramp_gain;
            }

            // Save gain envelope
            if i < self.output_buffer.len() {
                self.output_buffer[i] = gain;
            }
        }

        // Forward pass turning peaks into exponential decay.
        for i in 0..len {
            self.release_gain = 1.0 - (1.0 - self.release_gain) * self.alpha;
            if self.release_gain > 1.0 - self.epsilon {
                self.release_gain = 1.0
            }
            if self.output_buffer[i] < self.release_gain {
                self.release_gain = self.output_buffer[i];
            } else {
                self.output_buffer[i] = self.release_gain;
            }
        }

        // Apply gain reduction to delayed samples
        for i in 0..len {
            self.output_buffer[i] *= if i < self.attack {
                self.lookahead_buffer[self.samplerate - self.attack + i]
            } else {
                input[i - self.attack]
            }
        }

        // Drop old samples from beginning of lookahead buffer and copy input samples to its end
        self.lookahead_buffer.drain(..len);
        self.lookahead_buffer.extend(input.iter().copied());

        // Ouput
        input[..len].copy_from_slice(&self.output_buffer[..len]);
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
            let release;
            (self.limit, self.attack, release, self.epsilon, self.alpha) =
                LookaheadLimiter::configure(&config, self.samplerate);

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
    fn test_lookahead_limiter_basic() {
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
            0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.875, 0.75, 0.625, 1.0, -1.0, 0.75, 0.625, 1.0, 0.95,
            0.995, 0.9995, 1.0, 1.0,
        ];
        limiter.apply_lookahead_limiter(&mut input);
        assert_close(&input, &expected, 1e-6);
    }

    /// Zero attack and release should behave like a peak limiter
    #[test]
    fn test_lookahead_limiter_peak() {
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
    fn test_lookahead_limiter_zero_release() {
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
    fn test_lookahead_limiter_state_persistence() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 5.0,
            release: 4.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut buf1 = vec![1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let expected1 = vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.9, 0.8, 0.7, 0.6, 1.0];
        limiter.apply_lookahead_limiter(&mut buf1);
        assert_close(&buf1, &expected1, 1e-6);

        let mut buf2 = vec![1.0, 1.0, 1.0, 1.0];
        let expected2 = vec![0.95, 0.995, 0.9995, 1.0];
        limiter.apply_lookahead_limiter(&mut buf2);
        assert_close(&buf2, &expected2, 1e-6);
    }

    #[test]
    fn test_lookahead_limiter_attack_clamped_to_one_second() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 48001.0,
            release: 4.0,
        };
        let limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        assert_eq!(limiter.attack, 48000);
    }
}
