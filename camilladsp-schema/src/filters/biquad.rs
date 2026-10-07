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

// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

use crate::config;
use crate::config::{Issues, issue_path};

/// Struct to hold the biquad coefficients
#[derive(Clone, Copy, Debug)]
pub struct BiquadCoefficients {
    pub a1: f64,
    pub a2: f64,
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
}

impl BiquadCoefficients {
    pub fn new(a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> Self {
        BiquadCoefficients { a1, a2, b0, b1, b2 }
    }

    pub fn normalize(a0: f64, a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> Self {
        let a1n = a1 / a0;
        let a2n = a2 / a0;
        let b0n = b0 / a0;
        let b1n = b1 / a0;
        let b2n = b2 / a0;
        debug!("a1={a1n} a2={a2n} b0={b0n} b1={b1n} b2={b2n}");
        BiquadCoefficients {
            a1: a1n,
            a2: a2n,
            b0: b0n,
            b1: b1n,
            b2: b2n,
        }
    }

    pub fn is_stable(&self) -> bool {
        self.a2.abs() < 1.0 && (self.a1.abs() < (self.a2 + 1.0))
    }

    /// Create biquad filters from config.
    /// Filter types
    /// - Free: just coefficients
    /// - Highpass: second order highpass specified by frequency and Q-value.
    /// - Lowpass: second order lowpass specified by frequency and Q-value.
    /// - Peaking: parametric peaking filter specified by gain, frequency and Q-value.
    /// - Highshelf: shelving filter affecting high frequencies with arbitrary slope in between.
    ///   The frequency specified is the middle of the slope
    /// - Lowshelf: shelving filter affecting low frequencies with arbitrary slope in between.
    ///   The frequency specified is the middle of the slope
    pub fn from_config(fs: usize, parameters: config::BiquadParameters) -> Self {
        match parameters {
            config::BiquadParameters::Free { a1, a2, b0, b1, b2 } => {
                let (a1, a2, b0, b1, b2) = (a1.get(), a2.get(), b0.get(), b1.get(), b2.get());
                BiquadCoefficients::new(a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Highpass { freq, q } => {
                let (freq, q) = (freq.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * q);
                let b0 = (1.0 + cs) / 2.0;
                let b1 = -(1.0 + cs);
                let b2 = (1.0 + cs) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowpass { freq, q } => {
                let (freq, q) = (freq.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * q);
                let b0 = (1.0 - cs) / 2.0;
                let b1 = 1.0 - cs;
                let b2 = (1.0 - cs) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq, gain, q }) => {
                let (freq, gain, q) = (freq.get(), gain.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let alpha = sn / (2.0 * q);
                let b0 = 1.0 + (alpha * ampl);
                let b1 = -2.0 * cs;
                let b2 = 1.0 - (alpha * ampl);
                let a0 = 1.0 + (alpha / ampl);
                let a1 = -2.0 * cs;
                let a2 = 1.0 - (alpha / ampl);
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Peaking(config::PeakingWidth::Bandwidth {
                freq,
                gain,
                bandwidth,
            }) => {
                let (freq, gain, bandwidth) = (freq.get(), gain.get(), bandwidth.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let alpha = sn * (std::f64::consts::LN_2 / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0 + (alpha * ampl);
                let b1 = -2.0 * cs;
                let b2 = 1.0 - (alpha * ampl);
                let a0 = 1.0 + (alpha / ampl);
                let a1 = -2.0 * cs;
                let a2 = 1.0 - (alpha / ampl);
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }

            config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, q, gain }) => {
                let (freq, q, gain) = (freq.get(), q.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let beta = sn * ampl.sqrt() / q;
                let b0 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs + beta);
                let b1 = -2.0 * ampl * ((ampl - 1.0) + (ampl + 1.0) * cs);
                let b2 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs - beta);
                let a0 = (ampl + 1.0) - (ampl - 1.0) * cs + beta;
                let a1 = 2.0 * ((ampl - 1.0) - (ampl + 1.0) * cs);
                let a2 = (ampl + 1.0) - (ampl - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope {
                freq,
                slope,
                gain,
            }) => {
                let (freq, slope, gain) = (freq.get(), slope.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let alpha =
                    sn / 2.0 * ((ampl + 1.0 / ampl) * (1.0 / (slope / 12.0) - 1.0) + 2.0).sqrt();
                let beta = 2.0 * ampl.sqrt() * alpha;
                let b0 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs + beta);
                let b1 = -2.0 * ampl * ((ampl - 1.0) + (ampl + 1.0) * cs);
                let b2 = ampl * ((ampl + 1.0) + (ampl - 1.0) * cs - beta);
                let a0 = (ampl + 1.0) - (ampl - 1.0) * cs + beta;
                let a1 = 2.0 * ((ampl - 1.0) - (ampl + 1.0) * cs);
                let a2 = (ampl + 1.0) - (ampl - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::HighshelfFO { freq, gain } => {
                let (freq, gain) = (freq.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let tn = (omega / 2.0).tan();
                let ampl = 10.0f64.powf(gain / 40.0);
                let b0 = ampl * tn + ampl.powi(2);
                let b1 = ampl * tn - ampl.powi(2);
                let b2 = 0.0;
                let a0 = ampl * tn + 1.0;
                let a1 = ampl * tn - 1.0;
                let a2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, q, gain }) => {
                let (freq, q, gain) = (freq.get(), q.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let beta = sn * ampl.sqrt() / q;
                let b0 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs + beta);
                let b1 = 2.0 * ampl * ((ampl - 1.0) - (ampl + 1.0) * cs);
                let b2 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs - beta);
                let a0 = (ampl + 1.0) + (ampl - 1.0) * cs + beta;
                let a1 = -2.0 * ((ampl - 1.0) + (ampl + 1.0) * cs);
                let a2 = (ampl + 1.0) + (ampl - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope {
                freq,
                slope,
                gain,
            }) => {
                let (freq, slope, gain) = (freq.get(), slope.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = 10.0f64.powf(gain / 40.0);
                let alpha =
                    sn / 2.0 * ((ampl + 1.0 / ampl) * (1.0 / (slope / 12.0) - 1.0) + 2.0).sqrt();
                let beta = 2.0 * ampl.sqrt() * alpha;
                let b0 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs + beta);
                let b1 = 2.0 * ampl * ((ampl - 1.0) - (ampl + 1.0) * cs);
                let b2 = ampl * ((ampl + 1.0) - (ampl - 1.0) * cs - beta);
                let a0 = (ampl + 1.0) + (ampl - 1.0) * cs + beta;
                let a1 = -2.0 * ((ampl - 1.0) + (ampl + 1.0) * cs);
                let a2 = (ampl + 1.0) + (ampl - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::LowshelfFO { freq, gain } => {
                let (freq, gain) = (freq.get(), gain.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let tn = (omega / 2.0).tan();
                let ampl = 10.0f64.powf(gain / 40.0);
                let b0 = ampl.powi(2) * tn + ampl;
                let b1 = ampl.powi(2) * tn - ampl;
                let b2 = 0.0;
                let a0 = tn + ampl;
                let a1 = tn - ampl;
                let a2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::LowpassFO { freq } => {
                let freq = freq.get();
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let k = (omega / 2.0).tan();
                let alpha = 1.0 + k;
                let a0 = 1.0;
                let a1 = -(1.0 - k) / alpha;
                let a2 = 0.0;
                let b0 = k / alpha;
                let b1 = k / alpha;
                let b2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::HighpassFO { freq } => {
                let freq = freq.get();
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let k = (omega / 2.0).tan();
                let alpha = 1.0 + k;
                let a0 = 1.0;
                let a1 = -(1.0 - k) / alpha;
                let a2 = 0.0;
                let b0 = 1.0 / alpha;
                let b1 = -1.0 / alpha;
                let b2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Notch(config::NotchWidth::Q { freq, q }) => {
                let (freq, q) = (freq.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * q);
                let b0 = 1.0;
                let b1 = -2.0 * cs;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Notch(config::NotchWidth::Bandwidth { freq, bandwidth }) => {
                let (freq, bandwidth) = (freq.get(), bandwidth.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * (std::f64::consts::LN_2 / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0;
                let b1 = -2.0 * cs;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::GeneralNotch(params) => {
                let (freq_z, freq_p, q_p) =
                    (params.freq_z.get(), params.freq_p.get(), params.q_p.get());
                let tn_z = (std::f64::consts::PI * freq_z / (fs as f64)).tan();
                let tn_p = (std::f64::consts::PI * freq_p / (fs as f64)).tan();
                let alpha = tn_p / q_p;
                let tn2_p = tn_p.powi(2);
                let tn2_z = tn_z.powi(2);
                let gain = if params.normalize_at_dc() {
                    tn2_p / tn2_z
                } else {
                    1.0
                };
                let b0 = gain * (1.0 + tn2_z);
                let b1 = -2.0 * gain * (1.0 - tn2_z);
                let b2 = gain * (1.0 + tn2_z);
                let a0 = 1.0 + alpha + tn2_p;
                let a1 = -2.0 + 2.0 * tn2_p;
                let a2 = 1.0 - alpha + tn2_p;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Bandpass(config::NotchWidth::Q { freq, q }) => {
                let (freq, q) = (freq.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * q);
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Bandpass(config::NotchWidth::Bandwidth {
                freq,
                bandwidth,
            }) => {
                let (freq, bandwidth) = (freq.get(), bandwidth.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * (std::f64::consts::LN_2 / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Allpass(config::NotchWidth::Q { freq, q }) => {
                let (freq, q) = (freq.get(), q.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * q);
                let b0 = 1.0 - alpha;
                let b1 = -2.0 * cs;
                let b2 = 1.0 + alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Allpass(config::NotchWidth::Bandwidth {
                freq,
                bandwidth,
            }) => {
                let (freq, bandwidth) = (freq.get(), bandwidth.get());
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * (std::f64::consts::LN_2 / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0 - alpha;
                let b1 = -2.0 * cs;
                let b2 = 1.0 + alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::AllpassFO { freq } => {
                let freq = freq.get();
                let omega = 2.0 * std::f64::consts::PI * freq / (fs as f64);
                let tn = (omega / 2.0).tan();
                let alpha = (tn + 1.0) / (tn - 1.0);
                let b0 = 1.0;
                let b1 = alpha;
                let b2 = 0.0;
                let a0 = alpha;
                let a1 = 1.0;
                let a2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::LinkwitzTransform {
                freq_act,
                q_act,
                freq_target,
                q_target,
            } => {
                let (freq_act, q_act, freq_target, q_target) = (
                    freq_act.get(),
                    q_act.get(),
                    freq_target.get(),
                    q_target.get(),
                );
                let d0i = (2.0 * std::f64::consts::PI * freq_act).powi(2);
                let d1i = (2.0 * std::f64::consts::PI * freq_act) / q_act;
                let c0i = (2.0 * std::f64::consts::PI * freq_target).powi(2);
                let c1i = (2.0 * std::f64::consts::PI * freq_target) / q_target;
                let fc = (freq_target + freq_act) / 2.0;

                let gn = 2.0 * std::f64::consts::PI * fc
                    / (std::f64::consts::PI * fc / (fs as f64)).tan();
                let gn2 = gn.powi(2);
                let cci = c0i + gn * c1i + gn2;

                let b0 = (d0i + gn * d1i + gn2) / cci;
                let b1 = 2.0 * (d0i - gn2) / cci;
                let b2 = (d0i - gn * d1i + gn2) / cci;
                let a0 = 1.0;
                let a1 = 2.0 * (c0i - gn2) / cci;
                let a2 = (c0i - gn * c1i + gn2) / cci;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
        }
    }
}

/// Check a frequency against zero and the Nyquist limit.
fn check_freq(issues: &mut Issues, field: &str, freq: f64, maxfreq: f64) {
    if freq <= 0.0 {
        issues.invalid(issue_path![field], "Frequency must be > 0");
    } else if freq >= maxfreq {
        issues.invalid(issue_path![field], "Frequency must be < samplerate/2");
    }
}

/// Validate biquad parameters. Issue paths are relative to the parameters.
///
/// The stability check needs the coefficients, which are only meaningful once
/// every parameter is in range, so it only runs when nothing else was found.
pub fn validate_config(
    samplerate: usize,
    parameters: &config::BiquadParameters,
) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let maxfreq = samplerate as f64 / 2.0;
    // Check frequency
    match parameters {
        config::BiquadParameters::Highpass { freq, .. }
        | config::BiquadParameters::Lowpass { freq, .. }
        | config::BiquadParameters::HighpassFO { freq, .. }
        | config::BiquadParameters::LowpassFO { freq, .. }
        | config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq, .. })
        | config::BiquadParameters::Peaking(config::PeakingWidth::Bandwidth { freq, .. })
        | config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, .. })
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, .. })
        | config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope { freq, .. })
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope { freq, .. })
        | config::BiquadParameters::HighshelfFO { freq, .. }
        | config::BiquadParameters::LowshelfFO { freq, .. }
        | config::BiquadParameters::Notch(config::NotchWidth::Q { freq, .. })
        | config::BiquadParameters::Bandpass(config::NotchWidth::Q { freq, .. })
        | config::BiquadParameters::Allpass(config::NotchWidth::Q { freq, .. })
        | config::BiquadParameters::Notch(config::NotchWidth::Bandwidth { freq, .. })
        | config::BiquadParameters::Bandpass(config::NotchWidth::Bandwidth { freq, .. })
        | config::BiquadParameters::Allpass(config::NotchWidth::Bandwidth { freq, .. })
        | config::BiquadParameters::AllpassFO { freq, .. } => {
            check_freq(&mut issues, "freq", freq.get(), maxfreq);
        }
        _ => {}
    }
    // Check Q
    match parameters {
        config::BiquadParameters::Highpass { q, .. }
        | config::BiquadParameters::Lowpass { q, .. }
        | config::BiquadParameters::Peaking(config::PeakingWidth::Q { q, .. })
        | config::BiquadParameters::Notch(config::NotchWidth::Q { q, .. })
        | config::BiquadParameters::Bandpass(config::NotchWidth::Q { q, .. })
        | config::BiquadParameters::Allpass(config::NotchWidth::Q { q, .. })
        | config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { q, .. })
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { q, .. })
            if *q <= 0.0 =>
        {
            issues.invalid(issue_path!["q"], "Q must be > 0");
        }
        config::BiquadParameters::GeneralNotch(config::GeneralNotchParams { q_p, .. })
            if *q_p <= 0.0 =>
        {
            issues.invalid(issue_path!["q_p"], "Q must be > 0");
        }
        _ => {}
    }
    // Check Bandwidth
    match parameters {
        config::BiquadParameters::Peaking(config::PeakingWidth::Bandwidth {
            bandwidth, ..
        })
        | config::BiquadParameters::Notch(config::NotchWidth::Bandwidth { bandwidth, .. })
        | config::BiquadParameters::Bandpass(config::NotchWidth::Bandwidth { bandwidth, .. })
        | config::BiquadParameters::Allpass(config::NotchWidth::Bandwidth { bandwidth, .. })
            if *bandwidth <= 0.0 =>
        {
            issues.invalid(issue_path!["bandwidth"], "Bandwidth must be > 0");
        }
        _ => {}
    }
    // Check slope
    match parameters {
        config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope { slope, .. })
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope { slope, .. }) => {
            if *slope <= 0.0 {
                issues.invalid(issue_path!["slope"], "Slope must be > 0");
            } else if *slope > 12.0 {
                issues.invalid(issue_path!["slope"], "Slope must be <= 12.0");
            }
        }
        _ => {}
    }
    // Check LT
    if let config::BiquadParameters::LinkwitzTransform {
        freq_act,
        q_act,
        freq_target,
        q_target,
    } = parameters
    {
        check_freq(&mut issues, "freq_act", freq_act.get(), maxfreq);
        check_freq(&mut issues, "freq_target", freq_target.get(), maxfreq);
        if *q_act <= 0.0 {
            issues.invalid(issue_path!["q_act"], "Q must be > 0");
        }
        if *q_target <= 0.0 {
            issues.invalid(issue_path!["q_target"], "Q must be > 0");
        }
    }
    // Check GeneralNotch frequencies
    if let config::BiquadParameters::GeneralNotch(params) = parameters {
        for (field, freq) in [("freq_p", params.freq_p), ("freq_z", params.freq_z)] {
            if freq <= 0.0 {
                issues.invalid(issue_path![field], "Pole and zero frequencies must be > 0");
            } else if freq >= maxfreq {
                issues.invalid(
                    issue_path![field],
                    "Pole and zero frequencies must be < samplerate/2",
                );
            }
        }
    }
    if issues.is_empty() {
        let coeffs = BiquadCoefficients::from_config(samplerate, parameters.clone());
        if !coeffs.is_stable() {
            issues.invalid(issue_path![], "Unstable filter specified");
        }
    }
    issues.into_result(())
}
