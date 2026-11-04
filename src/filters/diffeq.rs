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

use crate::config;
use crate::filters::Filter;

// Sample format
//type SmpFmt = i16;
use crate::CamillaFloat;
use crate::ToCamillaFloat;

/// Parameter validation lives in `camilladsp-schema`.
pub use camilladsp_schema::filters::diffeq::validate_config;

#[derive(Clone, Debug)]
pub struct DiffEq {
    /// Filter state, one value per filter order.
    s: Vec<CamillaFloat>,
    /// Feedback coefficients, normalized and zero padded, or empty when feedback can be skipped.
    a: Vec<CamillaFloat>,
    /// Feedforward coefficients, normalized and zero padded to one more than the filter order.
    b: Vec<CamillaFloat>,
    pub name: String,
}

impl DiffEq {
    pub fn new(name: &str, a_in: Vec<f64>, b_in: Vec<f64>) -> Self {
        let name = name.to_string();

        let mut a_scaled = if a_in.is_empty() { vec![1.0] } else { a_in };
        let mut b_scaled = if b_in.is_empty() { vec![1.0] } else { b_in };

        // The processing assumes that a0 is unity, so scale both sets of
        // coefficients to make that true. Coefficients that make this impossible
        // are left alone, validate_config rejects them before a filter is built.
        let a0 = a_scaled[0];
        if a0.is_finite() && a0 != 0.0 && a0 != 1.0 {
            a_scaled.iter_mut().for_each(|v| *v /= a0);
            b_scaled.iter_mut().for_each(|v| *v /= a0);
        }

        // Preserve the filter order even when all feedback coefficients are zero.
        let len = a_scaled.len().max(b_scaled.len());
        b_scaled.resize(len, 0.0);

        // Coefficients arrive from the config in f64 and cross into the
        // processing precision once, here.
        let a = if len > 9 || a_scaled[1..].iter().any(|&value| value != 0.0) {
            a_scaled.resize(len, 0.0);
            a_scaled.into_iter().map(|v| v.to_camilla_float()).collect()
        } else {
            Vec::new()
        };

        let b: Vec<CamillaFloat> = b_scaled.into_iter().map(|v| v.to_camilla_float()).collect();

        DiffEq {
            s: vec![0.0; len - 1],
            a,
            b,
            name,
        }
    }

    pub fn from_config(name: &str, conf: config::DiffEqParameters) -> Self {
        let a = conf.a();
        let b = conf.b();
        DiffEq::new(name, a, b)
    }

    /// Process a block of samples, with the filter order known at compile time.
    /// The state and the coefficients then stay in registers for the whole block.
    #[inline(always)]
    fn process_block<const ORDER: usize, const FEEDBACK: bool>(
        &mut self,
        waveform: &mut [CamillaFloat],
    ) {
        let mut s = [0.0; ORDER];
        s.copy_from_slice(&self.s);
        let mut a = [0.0; ORDER];
        if FEEDBACK {
            a.copy_from_slice(&self.a[1..=ORDER]);
        }
        let mut b = [0.0; ORDER];
        b.copy_from_slice(&self.b[1..=ORDER]);
        let b0 = self.b[0];
        for item in waveform.iter_mut() {
            let input = *item;
            let out = b0 * input + s[0];
            for n in 0..ORDER - 1 {
                let value = b[n] * input;
                let feedback = if FEEDBACK { a[n] * out } else { 0.0 };
                s[n] = value - feedback + s[n + 1];
            }
            let value = b[ORDER - 1] * input;
            let feedback = if FEEDBACK { a[ORDER - 1] * out } else { 0.0 };
            s[ORDER - 1] = value - feedback;
            *item = out;
        }
        self.s.copy_from_slice(&s);
    }

    /// Process a block of samples, for orders that have no compile time version.
    fn process_block_any_order(&mut self, waveform: &mut [CamillaFloat]) {
        let order = self.s.len();
        // Slicing once here keeps the loops below free of bounds checks.
        let s = &mut self.s[..order];
        let a = &self.a[1..=order];
        let b = &self.b[1..=order];
        let b0 = self.b[0];
        for item in waveform.iter_mut() {
            let input = *item;
            let out = b0 * input + s[0];
            for n in 0..order - 1 {
                s[n] = b[n] * input - a[n] * out + s[n + 1];
            }
            s[order - 1] = b[order - 1] * input - a[order - 1] * out;
            *item = out;
        }
    }

    // Keep FIR and IIR register allocation independent.
    #[inline(never)]
    fn process_block_dispatch<const FEEDBACK: bool>(&mut self, waveform: &mut [CamillaFloat]) {
        // The filter is a direct form 2 transposed structure, the same form the
        // Biquad filter uses, generalized to any order. The low orders get a
        // version with the order known at compile time, which is where nearly
        // all real filters land.
        match self.s.len() {
            1 => self.process_block::<1, FEEDBACK>(waveform),
            2 => self.process_block::<2, FEEDBACK>(waveform),
            3 => self.process_block::<3, FEEDBACK>(waveform),
            4 => self.process_block::<4, FEEDBACK>(waveform),
            5 => self.process_block::<5, FEEDBACK>(waveform),
            6 => self.process_block::<6, FEEDBACK>(waveform),
            7 => self.process_block::<7, FEEDBACK>(waveform),
            8 => self.process_block::<8, FEEDBACK>(waveform),
            _ => self.process_block_any_order(waveform),
        }
    }

    /// Flush stored subnormal numbers to zero.
    fn flush_subnormals(&mut self) {
        for (n, s) in self.s.iter_mut().enumerate() {
            if s.is_subnormal() {
                trace!(
                    "DiffEq filter '{}', flushing subnormal state at index {}",
                    self.name, n
                );
                *s = 0.0;
            }
        }
    }
}

impl Filter for DiffEq {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        if self.s.is_empty() {
            let b0 = self.b[0];
            waveform.iter_mut().for_each(|item| *item *= b0);
            return;
        }
        if self.a.is_empty() {
            self.process_block_dispatch::<false>(waveform);
        } else {
            self.process_block_dispatch::<true>(waveform);
        }
        self.flush_subnormals();
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::DiffEq {
            parameters: conf, ..
        } = conf
        {
            *self = DiffEq::from_config(&self.name, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            unreachable!("Invalid config change!");
        }
    }
}

#[cfg(test)]
#[cfg_attr(camillafloat_f32, allow(clippy::excessive_precision))]
mod tests {
    use crate::CamillaFloat;
    use crate::config;
    use crate::config::finite;
    use crate::filters::Filter;
    use crate::filters::diffeq::{DiffEq, validate_config};

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
    fn check_result() {
        let mut filter = DiffEq::new(
            "test",
            vec![1.0, -0.1462978543780541, 0.005350765548905586],
            vec![0.21476322779271284, 0.4295264555854257, 0.21476322779271284],
        );
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![0.215, 0.461, 0.281, 0.039, 0.004, 0.0, 0.0, 0.0];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-3));
    }

    #[test]
    fn check_result_unscaled() {
        // Same filter as in check_result, with all coefficients multiplied by three.
        // Scaling both a and b leaves the transfer function unchanged.
        let mut filter = DiffEq::new(
            "test",
            vec![3.0, -0.4388935631341623, 0.016052296646716757],
            vec![0.6442896833781385, 1.288579366756277, 0.6442896833781385],
        );
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![0.215, 0.461, 0.281, 0.039, 0.004, 0.0, 0.0, 0.0];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-3));
    }

    fn order_twelve() -> (Vec<f64>, Vec<f64>) {
        let a = vec![
            1.0,
            0.05,
            0.0333333333333333,
            0.025,
            0.02,
            0.0166666666666667,
            0.0142857142857143,
            0.0125,
            0.0111111111111111,
            0.01,
            0.0090909090909091,
            0.0083333333333333,
            0.0076923076923077,
        ];
        let b = vec![
            0.3,
            0.15,
            0.1,
            0.075,
            0.06,
            0.05,
            0.0428571428571429,
            0.0375,
            0.0333333333333333,
            0.03,
            0.0272727272727273,
            0.025,
            0.05,
        ];
        (a, b)
    }

    #[test]
    fn check_result_high_order() {
        // Twelfth order, above the highest compile time order.
        // Reference values from the difference equation, evaluated in direct form 1.
        let (a, b) = order_twelve();
        let mut filter = DiffEq::new("test", a, b);
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![
            0.3,
            0.135,
            0.08325,
            0.0588375,
            0.044908125,
            0.03601209375,
            0.029887948884,
            0.025439674877,
            0.022075875735,
            0.019451146989,
            0.017351068867,
            0.015635919346,
        ];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-6));
    }

    #[test]
    fn check_state_between_chunks() {
        // Splitting the waveform must give the same result as processing it in one go.
        let (a, b) = order_twelve();
        let mut filter = DiffEq::new("test", a.clone(), b.clone());
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        filter.process_waveform(&mut wave);

        let mut split_filter = DiffEq::new("test", a, b);
        let mut first = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut second = vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        split_filter.process_waveform(&mut first);
        split_filter.process_waveform(&mut second);
        first.append(&mut second);
        assert!(compare_waveforms(wave, first, 1e-9));
    }

    #[test]
    fn check_result_uneven_lengths() {
        // More b than a coefficients.
        let mut filter = DiffEq::new("test", vec![1.0, -0.5], vec![0.2, 0.1, 0.05, 0.01]);
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![0.2, 0.2, 0.15, 0.085, 0.0425, 0.02125, 0.010625, 0.0053125];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-6));

        // More a than b coefficients.
        let mut filter = DiffEq::new("test", vec![1.0, -0.5, 0.2, -0.05], vec![0.5]);
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![
            0.5,
            0.25,
            0.025,
            -0.0125,
            0.00125,
            0.004375,
            0.0013125,
            -0.00015625,
        ];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-6));
    }

    #[test]
    fn check_result_no_state() {
        // A single pair of coefficients is a plain gain.
        let mut filter = DiffEq::new("test", vec![2.0], vec![1.0]);
        let mut wave = vec![1.0, -1.0, 0.5];
        let expected = vec![0.5, -0.5, 0.25];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-9));
    }

    fn parameters(a: Vec<f64>, b: Vec<f64>) -> config::DiffEqParameters {
        config::DiffEqParameters {
            a: Some(a.into_iter().map(|value| finite!(value)).collect()),
            b: Some(b.into_iter().map(|value| finite!(value)).collect()),
        }
    }

    #[test]
    fn fir_matches_convolution_across_chunks_and_updates() {
        use crate::ToCamillaFloat;

        for taps in (1..=10).chain([16, 33]) {
            let coefficients: Vec<_> = (0..taps)
                .map(|i| (i as f64 * 0.23).cos() / taps as f64)
                .collect();
            for scale in [1.0, 2.0] {
                let mut filter = DiffEq::from_config("test", parameters(vec![1.0], vec![1.0]));
                let a = if scale == 1.0 {
                    Vec::new()
                } else {
                    let mut a = vec![0.0; taps + 3];
                    a[0] = scale;
                    a
                };
                let fir = config::Filter::DiffEq {
                    description: None,
                    parameters: parameters(a, coefficients.iter().map(|v| v * scale).collect()),
                };
                for _ in 0..2 {
                    filter.update_parameters(fir.clone());
                    let mut input = Vec::new();
                    for (block, frames) in [1, 7, 64, 257, 7, 64].into_iter().enumerate() {
                        let start = input.len();
                        let mut wave: Vec<_> = (start..start + frames)
                            .map(|i| {
                                if block < 3 {
                                    (0.7 * (i as f64 * 0.13 + 0.31).sin()).to_camilla_float()
                                } else {
                                    0.0
                                }
                            })
                            .collect();
                        input.extend_from_slice(&wave);
                        let expected: Vec<_> = (start..start + frames)
                            .map(|n| {
                                (0..taps.min(n + 1)).rev().fold(0.0, |sum, k| {
                                    coefficients[k].to_camilla_float() * input[n - k] + sum
                                })
                            })
                            .collect();
                        filter.process_waveform(&mut wave);
                        assert_eq!(wave, expected, "taps {taps}, scale {scale}, block {block}");
                    }
                    filter.update_parameters(config::Filter::DiffEq {
                        description: None,
                        parameters: parameters(vec![1.0, -0.5], vec![0.5]),
                    });
                    let mut impulse = [1.0, 0.0, 0.0, 0.0];
                    filter.process_waveform(&mut impulse);
                    assert_eq!(impulse, [0.5, 0.25, 0.125, 0.0625]);
                }
            }
        }
    }

    #[test]
    fn validate_stable() {
        // Biquad lowpass, poles at 0.073 and 0.073.
        assert!(
            validate_config(&parameters(
                vec![1.0, -0.1462978543780541, 0.005350765548905586],
                vec![0.21476322779271284, 0.4295264555854257, 0.21476322779271284],
            ))
            .is_ok()
        );
        // Single pole at 0.5.
        assert!(validate_config(&parameters(vec![1.0, -0.5], vec![0.5])).is_ok());
        // Complex pole pair at 0.707, note that a1 is larger than unity.
        assert!(validate_config(&parameters(vec![1.0, -1.2, 0.5], vec![1.0])).is_ok());
        // Fourth order, poles at 0.9, -0.9, 0.5 and -0.5.
        assert!(
            validate_config(&parameters(vec![1.0, 0.0, -1.06, 0.0, 0.2025], vec![1.0])).is_ok()
        );
        // Pole at zero, from the trailing zero coefficient.
        assert!(validate_config(&parameters(vec![1.0, 0.5, 0.0], vec![1.0])).is_ok());
        // Unscaled coefficients, poles at 0.073 and 0.073.
        assert!(validate_config(&parameters(vec![4.0, -0.585, 0.0214], vec![1.0])).is_ok());
        // No feedback, a plain FIR filter.
        assert!(validate_config(&parameters(vec![], vec![0.5, 0.5])).is_ok());
        assert!(validate_config(&parameters(vec![1.0], vec![0.5, 0.5])).is_ok());
    }

    #[test]
    fn validate_unstable() {
        // Single pole at 1.1.
        assert!(validate_config(&parameters(vec![1.0, -1.1], vec![1.0])).is_err());
        // Single pole exactly on the unit circle.
        assert!(validate_config(&parameters(vec![1.0, -1.0], vec![1.0])).is_err());
        // Poles at 1.1 and -1.1.
        assert!(validate_config(&parameters(vec![1.0, 0.0, -1.21], vec![1.0])).is_err());
        // Poles at 1.5 and -0.6, all coefficients are smaller than unity.
        assert!(validate_config(&parameters(vec![1.0, -0.9, -0.9], vec![1.0])).is_err());
        // Unscaled coefficients, pole at 1.1.
        assert!(validate_config(&parameters(vec![2.0, -2.2], vec![1.0])).is_err());
    }

    #[test]
    fn validate_invalid_coefficients() {
        assert!(validate_config(&parameters(vec![0.0, 0.5], vec![1.0])).is_err());
        // A non-finite coefficient cannot be built any more, `FiniteF64` cannot hold one, so
        // there is nothing left to test here. Rejecting `.nan` and `.inf` in a config file is
        // covered by `config::utils::tests::non_finite_rejected_while_parsing`.
    }
}
