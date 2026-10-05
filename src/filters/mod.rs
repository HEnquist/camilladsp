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

/// Basic gain, delay, and volume filters.
pub mod basicfilters;
/// Second-order IIR biquad filters.
pub mod biquad;
/// Multi-section biquad combinations (shelves, butterworth, etc.).
pub mod biquadcombo;
/// Hard/soft-clipping filter.
pub mod clipper;
/// Difference-equation (IIR) filter with arbitrary coefficients.
pub mod diffeq;
/// Dithering noise filters.
pub mod dither;
/// Convolution filter via FFT overlap-save.
pub mod fftconv;
/// Lookahead limiter.
pub mod lookahead_limiter;
/// Loudness compensation filter.
pub mod loudness;

use crate::config;

use crate::CamillaFloat;

/// Coefficient file reading and parameter validation live in `camilladsp-config`.
pub use camilladsp_config::filters::{read_coeff_file, read_wav, validate_filter};

/// Trait implemented by all single-channel audio filters.
pub trait Filter {
    /// Apply the filter to `waveform` in place.
    ///
    /// Infallible. A filter that was accepted at construction cannot start
    /// failing on a later chunk, and nothing could be done about it part way
    /// through a chunk in the processing thread if it did.
    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]);

    /// Hot-reload filter coefficients from a new configuration without rebuilding.
    fn update_parameters(&mut self, config: config::Filter);

    /// Hot-reload as [`Filter::update_parameters`], reusing anything `cache`
    /// already holds for this filter name.
    ///
    /// Only convolution filters have coefficients worth sharing between the
    /// channels of a step, so every other filter keeps the default and ignores
    /// the cache.
    fn update_parameters_cached(
        &mut self,
        config: config::Filter,
        _cache: &mut fftconv::ConvCoeffCache,
    ) {
        self.update_parameters(config);
    }

    /// Return the filter's name as given in the configuration.
    fn name(&self) -> &str;
}

/// Zero-pad `values` to at least `length` elements (never truncates).
pub fn pad_vector(values: &[CamillaFloat], length: usize) -> Vec<CamillaFloat> {
    let new_len = if values.len() > length {
        values.len()
    } else {
        length
    };
    let mut new_values: Vec<CamillaFloat> = vec![0.0; new_len];
    new_values[0..values.len()].copy_from_slice(values);
    new_values
}

#[cfg(test)]
mod tests {
    use crate::CamillaFloat;
    use crate::config::FileSampleFormat;
    use crate::filters::read_wav;
    use crate::filters::{pad_vector, read_coeff_file};

    fn is_close(left: CamillaFloat, right: CamillaFloat, maxdiff: CamillaFloat) -> bool {
        println!("{} - {} = {}", left, right, left - right);
        let res = (left - right).abs() < maxdiff;
        println!("Ok: {res}");
        res
    }

    fn compare_waveforms(
        left: &[CamillaFloat],
        right: &[CamillaFloat],
        maxdiff: CamillaFloat,
    ) -> bool {
        if left.len() != right.len() {
            println!("wrong length");
            return false;
        }
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn read_float32() {
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileSampleFormat::F32_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileSampleFormat::F32_LE, 12, 4).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_float64() {
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileSampleFormat::F64_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileSampleFormat::F64_LE, 24, 8).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int16() {
        let loaded =
            read_coeff_file("testdata/int16.raw", &FileSampleFormat::S16_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int16.raw", &FileSampleFormat::S16_LE, 6, 2).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int24() {
        let loaded =
            read_coeff_file("testdata/int24.raw", &FileSampleFormat::S24_4_RJ_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int24.raw", &FileSampleFormat::S24_4_RJ_LE, 12, 4).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int24_3() {
        let loaded =
            read_coeff_file("testdata/int243.raw", &FileSampleFormat::S24_3_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int243.raw", &FileSampleFormat::S24_3_LE, 9, 3).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int32() {
        let loaded =
            read_coeff_file("testdata/int32.raw", &FileSampleFormat::S32_LE, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int32.raw", &FileSampleFormat::S32_LE, 12, 4).unwrap();
        let expected: Vec<CamillaFloat> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_text() {
        let loaded = read_coeff_file("testdata/text.txt", &FileSampleFormat::TEXT, 0, 0).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/text_header.txt", &FileSampleFormat::TEXT, 4, 1).unwrap();
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn test_padding() {
        let values: Vec<CamillaFloat> = vec![1.0, 0.5];
        let values_padded: Vec<CamillaFloat> = vec![1.0, 0.5, 0.0, 0.0, 0.0];
        let values_0 = pad_vector(&values, 0);
        assert!(compare_waveforms(&values, &values_0, 1e-15));
        let values_5 = pad_vector(&values, 5);
        assert!(compare_waveforms(&values_padded, &values_5, 1e-15));
    }

    #[test]
    pub fn test_read_wav() {
        let values = read_wav("testdata/int32.wav", 0).unwrap();
        println!("{values:?}");
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&values, &expected, 1e-9));
        let bad = read_wav("testdata/int32.wav", 1);
        assert!(bad.is_err());
    }

    #[test]
    pub fn test_read_wav_rf64() {
        let values = read_wav("testdata/int32_rf64.wav", 0).unwrap();
        println!("{values:?}");
        let expected: Vec<CamillaFloat> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&values, &expected, 1e-9));
        let bad = read_wav("testdata/int32_rf64.wav", 1);
        assert!(bad.is_err());
    }
}
