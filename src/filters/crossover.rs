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

//! Linear-phase FIR crossover filters.
//!
//! The lowpass magnitude is 1/(1+r(x)) and the highpass is r(x)/(1+r(x)), with x the
//! prewarped normalized frequency and r(x) ~ x^n, where n = slope/6.
//! This gives an asymptotic slope of 6*n dB/octave, -6 dB at the crossover frequency,
//! and lowpass + highpass sums to exactly a pure delay.
//! The FIR length is chosen automatically so that the truncation error stays below
//! TRUNCATION_TOLERANCE. The latency is (length-1)/2 samples.
//!
//! NOTE: this algorithm is duplicated in pycamilladsp-plot, camilladsp_plot/crossover.py.
//! The GUI uses that copy to show the latency, so any change to the magnitude formula,
//! TRUNCATION_TOLERANCE, ODD_ORDER_SMOOTHING or the length search must be made in BOTH
//! places, and the expected latencies in the tests of both repos must be updated.

use crate::PrcFmt;
use crate::Res;
use crate::config;
use crate::filters::Filter;
use crate::filters::fftconv::FftConv;
use realfft::RealFftPlanner;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Max allowed truncation error of the magnitude response (-100 dB).
const TRUNCATION_TOLERANCE: f64 = 1.0e-5;
/// Smoothing of |x|^n for odd n, avoids an extremely long impulse response.
const ODD_ORDER_SMOOTHING: f64 = 0.01;
const MIN_DESIGN_FFT: usize = 1 << 15;
const MAX_DESIGN_FFT: usize = 1 << 23;
const MAX_CACHED_DESIGNS: usize = 64;

pub const MIN_SLOPE: usize = 12;
pub const MAX_SLOPE: usize = 96;

/// Zero-phase lowpass prototype, one side of the symmetric impulse response.
/// Element 0 is the center tap, the latency in samples equals len()-1.
#[derive(Debug)]
struct Prototype {
    half: Vec<f64>,
}

type CacheKey = (usize, u64, usize);

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<Prototype>>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<Prototype>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lowpass_magnitude(samplerate: usize, freq: f64, order: usize, nfft: usize) -> Vec<f64> {
    let fs = samplerate as f64;
    let wc = (std::f64::consts::PI * freq / fs).tan();
    let odd = !order.is_multiple_of(2);
    let eps_n = ODD_ORDER_SMOOTHING.powi(order as i32);
    (0..=nfft / 2)
        .map(|k| {
            if k == nfft / 2 {
                return 0.0;
            }
            let f = k as f64 * fs / nfft as f64;
            let x = (std::f64::consts::PI * f / fs).tan() / wc;
            let r = if odd {
                (x * x + ODD_ORDER_SMOOTHING * ODD_ORDER_SMOOTHING).powf(order as f64 / 2.0) - eps_n
            } else {
                x.powi(order as i32)
            };
            1.0 / (1.0 + r)
        })
        .collect()
}

fn design_prototype(samplerate: usize, freq: f64, slope: usize) -> Prototype {
    let order = slope / 6;
    let mut planner = RealFftPlanner::<f64>::new();
    let mut nfft = MIN_DESIGN_FFT;
    loop {
        let mut spectrum: Vec<num_complex::Complex<f64>> =
            lowpass_magnitude(samplerate, freq, order, nfft)
                .into_iter()
                .map(|m| num_complex::Complex::new(m, 0.0))
                .collect();
        let ifft = planner.plan_fft_inverse(nfft);
        let mut impulse = ifft.make_output_vec();
        ifft.process(&mut spectrum, &mut impulse).unwrap();
        let half: Vec<f64> = impulse[..=nfft / 2]
            .iter()
            .map(|v| v / nfft as f64)
            .collect();

        // Error bound from truncating at index m: 2 * sum of |h[k]| for k > m.
        let mut tail = 0.0;
        let mut latency = half.len() - 1;
        for (idx, value) in half.iter().enumerate().rev() {
            tail += 2.0 * value.abs();
            if tail >= TRUNCATION_TOLERANCE {
                latency = idx;
                break;
            }
        }
        if latency < nfft / 4 || nfft >= MAX_DESIGN_FFT {
            if latency >= nfft / 4 {
                warn!(
                    "Crossover at {freq} Hz, {slope} dB/oct needs a longer FIR than supported, response will be truncated"
                );
            }
            let half = half[..=latency].to_vec();
            return Prototype { half };
        }
        nfft *= 2;
    }
}

#[allow(clippy::unnecessary_cast)]
fn get_prototype(samplerate: usize, freq: PrcFmt, slope: usize) -> Arc<Prototype> {
    let freq = freq as f64;
    let key = (samplerate, freq.to_bits(), slope);
    if let Some(proto) = cache().lock().unwrap().get(&key) {
        return proto.clone();
    }
    let proto = Arc::new(design_prototype(samplerate, freq, slope));
    let mut cache = cache().lock().unwrap();
    if cache.len() >= MAX_CACHED_DESIGNS {
        cache.clear();
    }
    cache.insert(key, proto.clone());
    proto
}

fn freq_and_slope(conf: &config::CrossoverParameters) -> (PrcFmt, usize) {
    match conf {
        config::CrossoverParameters::Lowpass { freq, slope }
        | config::CrossoverParameters::Highpass { freq, slope } => (*freq, *slope),
    }
}

/// Latency in samples of a crossover filter.
/// Lowpass and highpass with the same frequency and slope always have the same latency.
pub fn latency(samplerate: usize, conf: &config::CrossoverParameters) -> usize {
    let (freq, slope) = freq_and_slope(conf);
    get_prototype(samplerate, freq, slope).half.len() - 1
}

/// FIR coefficients of a crossover filter.
pub fn coefficients(samplerate: usize, conf: &config::CrossoverParameters) -> Vec<PrcFmt> {
    let (freq, slope) = freq_and_slope(conf);
    let proto = get_prototype(samplerate, freq, slope);
    let center = proto.half.len() - 1;
    let mut lowpass = vec![0.0; 2 * center + 1];
    for (k, value) in proto.half.iter().enumerate() {
        lowpass[center + k] = *value;
        lowpass[center - k] = *value;
    }
    match conf {
        config::CrossoverParameters::Lowpass { .. } => {
            lowpass.iter().map(|v| *v as PrcFmt).collect()
        }
        config::CrossoverParameters::Highpass { .. } => {
            // Complementary highpass, delta minus lowpass.
            let mut highpass: Vec<PrcFmt> = lowpass.iter().map(|v| -*v as PrcFmt).collect();
            highpass[center] = (1.0 - lowpass[center]) as PrcFmt;
            highpass
        }
    }
}

pub struct Crossover {
    conv: FftConv,
    samplerate: usize,
}

impl Crossover {
    pub fn from_config(
        name: &str,
        data_length: usize,
        samplerate: usize,
        conf: config::CrossoverParameters,
    ) -> Self {
        let coeffs = coefficients(samplerate, &conf);
        debug!(
            "Crossover {name}: {} taps, latency {} samples",
            coeffs.len(),
            coeffs.len() / 2
        );
        Crossover {
            conv: FftConv::new(name, data_length, &coeffs),
            samplerate,
        }
    }
}

impl Filter for Crossover {
    fn name(&self) -> &str {
        self.conv.name()
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        self.conv.process_waveform(waveform)
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Crossover {
            parameters: conf, ..
        } = conf
        {
            let values = coefficients(self.samplerate, &conf);
            self.conv.update_parameters(config::Filter::Conv {
                description: None,
                parameters: config::ConvParameters::Values { values },
            });
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a crossover config.
pub fn validate_config(samplerate: usize, conf: &config::CrossoverParameters) -> Res<()> {
    let (freq, slope) = freq_and_slope(conf);
    let maxfreq = samplerate as PrcFmt / 2.0;
    if freq <= 0.0 {
        return Err(config::ConfigError::new("Frequency must be > 0").into());
    } else if freq >= maxfreq {
        return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
    }
    if !(MIN_SLOPE..=MAX_SLOPE).contains(&slope) || !slope.is_multiple_of(6) {
        return Err(config::ConfigError::new(
            "Slope must be a multiple of 6 dB/octave, between 12 and 96",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex;

    fn magnitude_db(coeffs: &[PrcFmt], samplerate: usize, freq: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / samplerate as f64;
        let sum: Complex<f64> = coeffs
            .iter()
            .enumerate()
            .map(|(n, c)| Complex::from_polar(*c as f64, -w * n as f64))
            .sum();
        20.0 * sum.norm().log10()
    }

    fn params(highpass: bool, freq: PrcFmt, slope: usize) -> config::CrossoverParameters {
        if highpass {
            config::CrossoverParameters::Highpass { freq, slope }
        } else {
            config::CrossoverParameters::Lowpass { freq, slope }
        }
    }

    #[test]
    fn sum_is_pure_delay() {
        for slope in [12, 18, 24, 48, 96] {
            let lp = coefficients(48000, &params(false, 1000.0, slope));
            let hp = coefficients(48000, &params(true, 1000.0, slope));
            assert_eq!(lp.len(), hp.len());
            let center = lp.len() / 2;
            for (n, (l, h)) in lp.iter().zip(hp.iter()).enumerate() {
                let expected = if n == center { 1.0 } else { 0.0 };
                assert!((l + h - expected).abs() < 1.0e-6);
            }
        }
    }

    #[test]
    fn minus_6_db_at_crossover() {
        for slope in [12, 24, 48, 96] {
            for highpass in [false, true] {
                let coeffs = coefficients(48000, &params(highpass, 2000.0, slope));
                let db = magnitude_db(&coeffs, 48000, 2000.0);
                assert!((db + 6.02).abs() < 0.05, "slope {slope}: {db} dB");
            }
        }
    }

    #[test]
    fn matches_target_response() {
        let fs = 48000;
        for slope in [12, 18, 24, 48, 96] {
            for fc in [80.0, 2500.0] {
                let coeffs = coefficients(fs, &params(false, fc, slope));
                let target = lowpass_magnitude(fs, fc as f64, slope / 6, 1 << 12);
                for (k, mag) in target.iter().enumerate().skip(1).step_by(7) {
                    let freq = k as f64 * fs as f64 / (1 << 12) as f64;
                    let actual = 10.0_f64.powf(magnitude_db(&coeffs, fs, freq) / 20.0);
                    assert!(
                        (actual - mag).abs() < 2.0e-5,
                        "slope {slope}, fc {fc}, f {freq}: {actual} vs {mag}"
                    );
                }
            }
        }
    }

    // Keep in sync with pycamilladsp-plot, tests/test_crossover.py
    #[test]
    fn known_latencies() {
        for (fs, freq, slope, expected) in [
            (44100, 80.0, 24, 1375),
            (48000, 80.0, 24, 1496),
            (48000, 40.0, 12, 2199),
            (48000, 80.0, 48, 2795),
            (48000, 80.0, 96, 5566),
            (48000, 2500.0, 96, 180),
            (48000, 1000.0, 18, 369),
            (48000, 10000.0, 30, 18),
        ] {
            assert_eq!(latency(fs, &params(false, freq, slope)), expected);
            assert_eq!(latency(fs, &params(true, freq, slope)), expected);
        }
    }

    #[test]
    fn same_latency_for_lowpass_and_highpass() {
        let lp = latency(44100, &params(false, 80.0, 48));
        let hp = latency(44100, &params(true, 80.0, 48));
        assert_eq!(lp, hp);
        assert!(lp > 0);
    }

    #[test]
    fn validation() {
        assert!(validate_config(48000, &params(false, 1000.0, 24)).is_ok());
        assert!(validate_config(48000, &params(false, 1000.0, 25)).is_err());
        assert!(validate_config(48000, &params(false, 1000.0, 0)).is_err());
        assert!(validate_config(48000, &params(false, 1000.0, 6)).is_err());
        assert!(validate_config(48000, &params(false, 1000.0, 102)).is_err());
        assert!(validate_config(48000, &params(false, 30000.0, 24)).is_err());
    }
}
