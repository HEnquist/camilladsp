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

use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;
use crate::config;
use crate::config::TimeUnit;
use crate::filters::Filter;
use crate::utils::decibels::db_to_linear;
use crate::utils::time::time_to_samples;
use ringbuf::LocalRb;
use ringbuf::storage::Heap;
use ringbuf::traits::*;

/// Convert the configured values to the internal limiter parameters:
/// linear limit, attack/lookahead in samples, and release coefficient.
pub fn limiter_parameters(
    limit: f64,
    attack: f64,
    attack_unit: TimeUnit,
    release: f64,
    release_unit: TimeUnit,
    samplerate: usize,
) -> (CamillaFloat, usize, CamillaFloat) {
    let limit = db_to_linear(limit).to_camilla_float();
    let attack_samples = time_to_samples(attack, attack_unit, samplerate).round() as usize;
    let release_samples = time_to_samples(release, release_unit, samplerate);
    let release_coeff = (-1.0 / release_samples).exp().to_camilla_float();
    (limit, attack_samples, release_coeff)
}

/// Validate the attack and release times of a lookahead limiter.
pub fn validate_times(
    attack: f64,
    attack_unit: TimeUnit,
    release: f64,
    samplerate: usize,
) -> Res<()> {
    if attack < 0.0 {
        let msg = "Attack time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    let attack_samples = time_to_samples(attack, attack_unit, samplerate).round() as usize;
    if attack_samples > samplerate {
        let msg = "Lookahead limiter attack time must be less than or equal to 1 second.";
        return Err(config::ConfigError::new(msg).into());
    }
    if release < 0.0 {
        let msg = "Release time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    Ok(())
}

/// A view of the lookahead window, the `attack_samples` newest samples of the
/// history buffer followed by the samples of the current chunk.
/// Index 0 is the sample that is due to leave the limiter first.
struct LookaheadWindow<'a> {
    first: &'a [CamillaFloat],
    second: &'a [CamillaFloat],
    start: usize,
    attack_samples: usize,
    current: &'a [CamillaFloat],
}

impl<'a> LookaheadWindow<'a> {
    fn new(
        history: &'a LocalRb<Heap<CamillaFloat>>,
        attack_samples: usize,
        current: &'a [CamillaFloat],
    ) -> Self {
        let start = history.occupied_len() - attack_samples;
        let (first, second) = history.as_slices();
        LookaheadWindow {
            first,
            second,
            start,
            attack_samples,
            current,
        }
    }

    #[inline]
    fn get(&self, index: usize) -> CamillaFloat {
        if index < self.attack_samples {
            let idx = self.start + index;
            if idx < self.first.len() {
                self.first[idx]
            } else {
                self.second[idx - self.first.len()]
            }
        } else {
            self.current[index - self.attack_samples]
        }
    }
}

/// The lookahead limiter algorithm, shared by the `LookaheadLimiter` filter
/// and the `LookaheadLimiter` processor.
///
/// It holds the lookahead history of the detection signal together with the release
/// envelope state, and turns the detection signal into a gain envelope that applies
/// to the signal delayed by `attack_samples`.
pub struct LookaheadGain {
    pub limit: CamillaFloat,
    pub attack_samples: usize,
    pub release_coeff: CamillaFloat,
    /// Lookahead history of the detection signal, always kept full.
    history: LocalRb<Heap<CamillaFloat>>,
    release_gain: CamillaFloat,
    /// Gain envelope of the chunk being processed.
    /// Doubles as output buffer in [LookaheadGain::process_waveform].
    gain: Vec<CamillaFloat>,
    /// Number of valid values in `gain`.
    gain_len: usize,
}

impl LookaheadGain {
    pub fn new(
        limit: CamillaFloat,
        attack_samples: usize,
        release_coeff: CamillaFloat,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let history_len = samplerate.max(chunksize);
        LookaheadGain {
            limit,
            attack_samples,
            release_coeff,
            history: LocalRb::from(vec![0.0 as CamillaFloat; history_len]),
            release_gain: 1.0,
            gain: vec![0.0 as CamillaFloat; chunksize],
            gain_len: 0,
        }
    }

    /// Update the parameters. The lookahead history is padded with silence,
    /// so that an increased attack time does not pull in stale samples.
    pub fn set_parameters(
        &mut self,
        limit: CamillaFloat,
        attack_samples: usize,
        release_coeff: CamillaFloat,
    ) {
        self.limit = limit;
        self.attack_samples = attack_samples;
        self.release_coeff = release_coeff;
        for _ in 0..self.attack_samples {
            self.history.push_overwrite(0.0);
        }
    }

    /// The gain envelope calculated for the most recent chunk.
    pub fn envelope(&self) -> &[CamillaFloat] {
        &self.gain[..self.gain_len]
    }

    /// Calculate the gain envelope for the coming chunk of the detection signal,
    /// leaving the result in `self.gain`. The history buffer is left untouched.
    fn calculate_envelope(&mut self, detection: &[CamillaFloat]) {
        let len = detection.len();
        let attack_samples = self.attack_samples;
        let limit = self.limit;

        // Backward pass turning peaks into linear ramps.
        {
            let window = LookaheadWindow::new(&self.history, attack_samples, detection);
            let mut peak = 1.0;
            let mut samples_since_peak = attack_samples + 1;

            for i in (0..(attack_samples + len)).rev() {
                // Get sample amplitude
                let amplitude = window.get(i).abs();

                // Compute reduction gain for current sample
                let mut gain = if amplitude > limit {
                    limit / amplitude
                } else {
                    1.0
                };

                // Compute ramp
                let mut ramp_gain = 1.0;
                if samples_since_peak <= attack_samples {
                    let ramp = (attack_samples - samples_since_peak) as CamillaFloat
                        / attack_samples.max(1) as CamillaFloat;
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
                if i < self.gain.len() {
                    self.gain[i] = gain;
                }
            }
        }

        // Forward pass turning peaks into exponential decay.
        // Release uses the same 1/e time constant coefficient as Compressor.
        // They are not exactly equal because Compressor works in the dB domain and has 1e-6 bias.
        for i in 0..len {
            self.release_gain = self.release_gain.powf(self.release_coeff);
            if self.gain[i] < self.release_gain {
                self.release_gain = self.gain[i];
            } else {
                self.gain[i] = self.release_gain;
            }
        }
        self.gain_len = len;
    }

    /// Calculate the gain envelope for `detection`, and store `detection` in the
    /// lookahead history. The envelope is fetched with [LookaheadGain::envelope],
    /// and applies to a signal that is delayed by `attack_samples` elsewhere.
    pub fn process_detection(&mut self, detection: &[CamillaFloat]) {
        if detection.is_empty() {
            self.gain_len = 0;
            return;
        }
        self.calculate_envelope(detection);
        self.history.push_slice_overwrite(detection);
    }

    /// Limit `waveform` in place, using the waveform itself as detection signal.
    /// The output is delayed by `attack_samples`.
    pub fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        let len = waveform.len();
        if len == 0 {
            self.gain_len = 0;
            return;
        }
        self.calculate_envelope(waveform);

        // Apply gain reduction to delayed samples
        {
            let attack_samples = self.attack_samples;
            let window = LookaheadWindow::new(&self.history, attack_samples, waveform);
            for (i, gain) in self.gain[..len].iter_mut().enumerate() {
                *gain *= window.get(i);
            }
        }

        // Drop old samples from beginning of lookahead buffer and copy input samples to its end
        self.history.push_slice_overwrite(waveform);

        // Output
        waveform.copy_from_slice(&self.gain[..len]);
    }
}

pub struct LookaheadLimiter {
    pub name: String,
    pub samplerate: usize,
    gain: LookaheadGain,
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

        LookaheadLimiter {
            name: name.to_string(),
            samplerate,
            gain: LookaheadGain::new(limit, attack_samples, release_coeff, samplerate, chunksize),
        }
    }

    fn configure(
        config: &config::LookaheadLimiterParameters,
        samplerate: usize,
    ) -> (CamillaFloat, usize, CamillaFloat) {
        limiter_parameters(
            config.limit,
            config.attack,
            config.attack_unit(),
            config.release,
            config.release_unit(),
            samplerate,
        )
    }
}

impl Filter for LookaheadLimiter {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        self.gain.process_waveform(waveform);
    }

    fn update_parameters(&mut self, config: config::Filter) {
        if let config::Filter::LookaheadLimiter {
            parameters: config, ..
        } = config
        {
            let (limit, attack_samples, release_coeff) =
                LookaheadLimiter::configure(&config, self.samplerate);
            self.gain
                .set_parameters(limit, attack_samples, release_coeff);

            debug!(
                "Updated lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} samples, release coefficient: {}",
                self.name, config.limit, limit, attack_samples, release_coeff
            );
        } else {
            panic!("Invalid config change!");
        }
    }
}

pub fn validate_config(config: &config::LookaheadLimiterParameters, samplerate: usize) -> Res<()> {
    validate_times(
        config.attack,
        config.attack_unit(),
        config.release,
        samplerate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiochunk::AudioChunk;
    use crate::config::TimeUnit;
    use crate::processors::Processor;

    fn assert_close(left: &[CamillaFloat], right: &[CamillaFloat], epsilon: CamillaFloat) {
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
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
            attack: 4.0,
            release: 1.0 / std::f64::consts::LN_2,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, 2.0, -2.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            1.0, 1.0,
        ];
        let expected: Vec<CamillaFloat> = vec![
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
            0.5_f64.powf(1.0 / 2.0) as CamillaFloat,
            0.625,
            1.0,
            0.5_f64.powf(1.0 / 2.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 4.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 8.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 16.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 32.0) as CamillaFloat,
        ];
        limiter.process_waveform(&mut input);
        assert_close(&input, &expected, 1e-6);
    }

    /// Zero attack and release should behave like a peak limiter
    #[test]
    fn test_lookahead_limiter_same_as_limiter() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
            attack: 0.0,
            release: 0.0,
        };
        let mut lookahead_limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let clipper = crate::filters::clipper::Clipper::from_config(
            "test",
            config::ClipperParameters {
                soft_clip: None,
                clip_limit: 0.0,
            },
        );

        let mut lookahead_input = vec![0.5, 1.0, 2.0, -2.0, -1.0, -0.5, 1.5, -1.5, 0.0];
        let mut clipper_input = lookahead_input.clone();

        lookahead_limiter.process_waveform(&mut lookahead_input);
        clipper.apply_clip(&mut clipper_input);

        assert_eq!(lookahead_input, clipper_input);
    }

    #[test]
    fn test_lookahead_limiter_zero_attack_matches_compressor() {
        let release_samples: f64 = 4.0;
        let samplerate = 48000;
        let mut limiter_input = vec![2.0, 1.0, 1.0, 1.0, 1.0];
        let chunksize = limiter_input.len();
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
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
                attack_unit: TimeUnit::Seconds,
                release: release_samples / samplerate as f64,
                release_unit: TimeUnit::Seconds,
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

        limiter.process_waveform(&mut limiter_input);
        compressor.process_chunk(&mut compressor_chunk);

        // The values are not exactly equal because compressor works in the dB domain and has 1e-6 bias.
        assert_close(&limiter_input, &compressor_chunk.waveforms[0], 1e-6);
    }

    #[test]
    fn test_lookahead_limiter_zero_release() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
            attack: 2.0,
            release: 0.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut input = vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0];
        limiter.process_waveform(&mut input);
        for &val in &input {
            assert!(val.abs() <= 1.0);
        }
    }

    #[test]
    fn test_lookahead_limiter_state_persistence() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
            attack: 5.0,
            release: 1.0 / std::f64::consts::LN_2,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 1024);
        let mut buf1 = vec![1.0, 1.0, 1.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let expected1: Vec<CamillaFloat> =
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.9, 0.8, 0.7, 0.6, 1.0];
        limiter.process_waveform(&mut buf1);
        assert_close(&buf1, &expected1, 1e-6);

        let mut buf2 = vec![1.0, 1.0, 1.0, 1.0];
        let expected2: Vec<CamillaFloat> = vec![
            0.5_f64.powf(1.0 / 2.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 4.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 8.0) as CamillaFloat,
            0.5_f64.powf(1.0 / 16.0) as CamillaFloat,
        ];
        limiter.process_waveform(&mut buf2);
        assert_close(&buf2, &expected2, 1e-6);
    }

    #[test]
    fn test_lookahead_limiter_attack_over_one_second_rejected() {
        let config = config::LookaheadLimiterParameters {
            limit: 0.0,
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
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
            attack_unit: TimeUnit::Samples,
            release_unit: TimeUnit::Samples,
            attack: 4.0,
            release: 1.0,
        };
        let mut limiter = LookaheadLimiter::from_config("test", config, samplerate, chunksize);
        let mut input = vec![1.0, 1.0, 2.0, 1.0, 1.0, -2.0, 1.0, 1.0];

        limiter.process_waveform(&mut input);

        assert_eq!(input.len(), chunksize);
    }
}
