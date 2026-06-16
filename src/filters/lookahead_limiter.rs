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
use ringbuf::LocalRb;
use ringbuf::storage::Heap;
use ringbuf::traits::*;

pub struct LookaheadLimiter {
    pub name: String,
    pub limit: PrcFmt,
    pub attack_samples: usize,
    pub release_coeff: PrcFmt,
    pub samplerate: usize,
    lookahead_buffer: LocalRb<Heap<PrcFmt>>,
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
        let (limit, attack_samples, release_coeff) =
            LookaheadLimiter::configure(&config, samplerate);

        debug!(
            "Creating lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release coefficient: {}",
            name, config.limit, limit, attack_samples, release_coeff
        );

        let lookahead_buffer_len = samplerate.max(chunksize);
        let lookahead_buffer = LocalRb::from(vec![0.0 as PrcFmt; lookahead_buffer_len]);

        LookaheadLimiter {
            name: name.to_string(),
            limit,
            attack_samples,
            release_coeff,
            samplerate,
            lookahead_buffer,
            release_gain: 1.0,
            output_buffer: vec![0.0 as PrcFmt; chunksize],
        }
    }

    fn configure(
        config: &config::LookaheadLimiterParameters,
        samplerate: usize,
    ) -> (PrcFmt, usize, PrcFmt) {
        let limit = db_to_linear(config.limit);

        let unit = config.unit();

        let attack_samples = time_to_samples(config.attack, unit, samplerate).round() as usize;

        let release_samples = time_to_samples(config.release, unit, samplerate);
        let release_coeff = (-1.0 / release_samples).exp();

        (limit, attack_samples, release_coeff)
    }

    fn apply_lookahead_limiter(&mut self, input: &mut [PrcFmt]) {
        let len = input.len();
        if len == 0 {
            return;
        }

        let lookahead_start = self.lookahead_buffer.occupied_len() - self.attack_samples;
        let (first, second) = self.lookahead_buffer.as_slices();
        let get_input_sample = |i: usize| -> PrcFmt {
            if i < self.attack_samples {
                let idx = lookahead_start + i;
                if idx < first.len() {
                    first[idx]
                } else {
                    second[idx - first.len()]
                }
            } else {
                input[i - self.attack_samples]
            }
        };

        // Backward pass turning peaks into linear ramps.
        let mut peak = 1.0;
        let mut samples_since_peak = self.attack_samples + 1;

        for i in (0..(self.attack_samples + len)).rev() {
            // Get sample amplitude
            let amplitude = get_input_sample(i).abs();

            // Compute reduction gain for current sample
            let mut gain = if amplitude > self.limit {
                self.limit / amplitude
            } else {
                1.0
            };

            // Compute ramp
            let mut ramp_gain = 1.0;
            if samples_since_peak <= self.attack_samples {
                let ramp = (self.attack_samples - samples_since_peak) as PrcFmt
                    / self.attack_samples.max(1) as PrcFmt;
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
        // Release uses the same 1/e time constant coefficient as Compressor.
        // They are not exactly equal because Compressor works in the dB domain and has 1e-6 bias.
        for i in 0..len {
            self.release_gain = self.release_gain.powf(self.release_coeff);
            if self.output_buffer[i] < self.release_gain {
                self.release_gain = self.output_buffer[i];
            } else {
                self.output_buffer[i] = self.release_gain;
            }
        }

        // Apply gain reduction to delayed samples
        for i in 0..len {
            self.output_buffer[i] *= get_input_sample(i);
        }

        // Drop old samples from beginning of lookahead buffer and copy input samples to its end
        self.lookahead_buffer.push_slice_overwrite(input);

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
            (self.limit, self.attack_samples, self.release_coeff) =
                LookaheadLimiter::configure(&config, self.samplerate);

            debug!(
                "Updated lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release coefficient: {}",
                self.name, config.limit, self.limit, self.attack_samples, self.release_coeff
            );
        } else {
            panic!("Invalid config change!");
        }
    }
}

pub fn validate_config(config: &config::LookaheadLimiterParameters, samplerate: usize) -> Res<()> {
    if config.attack < 0.0 {
        let msg = "Attack time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    let attack_samples = time_to_samples(config.attack, config.unit(), samplerate).round() as usize;
    if attack_samples > samplerate {
        let msg = "Lookahead limiter attack time must be less than or equal to 1 second.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.release < 0.0 {
        let msg = "Release time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiochunk::AudioChunk;
    use crate::config::TimeUnit;
    use crate::processors::Processor;

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
            release: 1.0 / std::f64::consts::LN_2,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, 2.0, -2.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            1.0, 1.0,
        ];
        let expected = vec![
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            0.875,
            0.75,
            0.625,
            1.0,
            -1.0,
            0.5_f64.powf(1.0 / 2.0),
            0.625,
            1.0,
            0.5_f64.powf(1.0 / 2.0),
            0.5_f64.powf(1.0 / 4.0),
            0.5_f64.powf(1.0 / 8.0),
            0.5_f64.powf(1.0 / 16.0),
            0.5_f64.powf(1.0 / 32.0),
        ];
        limiter.apply_lookahead_limiter(&mut input);
        assert_close(&input, &expected, 1e-6);
    }

    /// Zero attack and release should behave like a peak limiter
    #[test]
    fn test_lookahead_limiter_same_as_limiter() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 0.0,
            release: 0.0,
        };
        let mut lookahead_limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let limiter = crate::filters::limiter::Limiter::from_config(
            "test",
            config::LimiterParameters {
                soft_clip: None,
                clip_limit: 0.0,
            },
        );

        let mut lookahead_input = vec![0.5, 1.0, 2.0, -2.0, -1.0, -0.5, 1.5, -1.5, 0.0];
        let mut limiter_input = lookahead_input.clone();

        lookahead_limiter.apply_lookahead_limiter(&mut lookahead_input);
        limiter.apply_clip(&mut limiter_input);

        assert_eq!(lookahead_input, limiter_input);
    }

    #[test]
    fn test_lookahead_limiter_zero_attack_matches_compressor() {
        let release_samples: PrcFmt = 4.0;
        let samplerate = 48000;
        let mut limiter_input = vec![2.0, 1.0, 1.0, 1.0, 1.0];
        let chunksize = limiter_input.len();
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 0.0,
            release: release_samples,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, samplerate, chunksize);
        let mut compressor = crate::processors::compressor::Compressor::from_config(
            "test",
            config::CompressorParameters {
                channels: 1,
                monitor_channels: None,
                process_channels: None,
                attack: 0.0,
                release: release_samples / samplerate as PrcFmt,
                threshold: 0.0,
                factor: 1.0e20,
                makeup_gain: None,
                soft_clip: None,
                clip_limit: None,
            },
            samplerate,
            chunksize,
        );

        let mut compressor_chunk = AudioChunk::new(
            vec![limiter_input.clone()],
            1.0,
            -1.0,
            limiter_input.len(),
            limiter_input.len(),
        );

        limiter.apply_lookahead_limiter(&mut limiter_input);
        compressor.process_chunk(&mut compressor_chunk).unwrap();

        // The values are not exactly equal because compressor works in the dB domain and has 1e-6 bias.
        assert_close(&limiter_input, &compressor_chunk.waveforms[0], 1e-6);
    }

    #[test]
    fn test_lookahead_limiter_zero_release() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 2.0,
            release: 0.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0];
        limiter.apply_lookahead_limiter(&mut input);
        for &val in &input {
            assert!(val.abs() <= 1.0);
        }
    }

    #[test]
    fn test_lookahead_limiter_state_persistence() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 5.0,
            release: 1.0 / std::f64::consts::LN_2,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut buf1 = vec![1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let expected1 = vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.9, 0.8, 0.7, 0.6, 1.0];
        limiter.apply_lookahead_limiter(&mut buf1);
        assert_close(&buf1, &expected1, 1e-6);

        let mut buf2 = vec![1.0, 1.0, 1.0, 1.0];
        let expected2 = vec![
            0.5_f64.powf(1.0 / 2.0),
            0.5_f64.powf(1.0 / 4.0),
            0.5_f64.powf(1.0 / 8.0),
            0.5_f64.powf(1.0 / 16.0),
        ];
        limiter.apply_lookahead_limiter(&mut buf2);
        assert_close(&buf2, &expected2, 1e-6);
    }

    #[test]
    fn test_lookahead_limiter_attack_over_one_second_rejected() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 48001.0,
            release: 4.0,
        };
        assert!(validate_config(&config, 48000).is_err());
    }

    #[test]
    fn test_lookahead_limiter_chunksize_larger_than_samplerate() {
        let samplerate = 4;
        let chunksize = 8;
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            unit: TimeUnit::Samples,
            attack: 4.0,
            release: 1.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, samplerate, chunksize);
        let mut input = vec![1.0, 1.0, 2.0, 1.0, 1.0, -2.0, 1.0, 1.0];

        limiter.apply_lookahead_limiter(&mut input);

        assert_eq!(input.len(), chunksize);
    }
}
