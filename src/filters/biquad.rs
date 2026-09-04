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

//mod filters;

use crate::config;
use crate::filters::Filter;

// Sample format
//type SmpFmt = i16;
use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;

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
                BiquadCoefficients::new(a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Highpass { freq, q } => {
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
                let tn_z = (std::f64::consts::PI * params.freq_z / (fs as f64)).tan();
                let tn_p = (std::f64::consts::PI * params.freq_p / (fs as f64)).tan();
                let alpha = tn_p / params.q_p;
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

/// Coefficients converted to the processing precision.
///
/// [`BiquadCoefficients`] is computed in `f64` regardless of what
/// [`CamillaFloat`] is, because coefficient math happens once per config load
/// and the extra precision is free there. It matters most for low-frequency
/// filters, where the coefficients cluster close to the limits of what an f32
/// mantissa resolves. Only the finished values cross into the processing
/// precision, here.
#[derive(Clone, Copy, Debug, Default)]
struct RuntimeCoefficients {
    a1: CamillaFloat,
    a2: CamillaFloat,
    b0: CamillaFloat,
    b1: CamillaFloat,
    b2: CamillaFloat,
}

impl From<BiquadCoefficients> for RuntimeCoefficients {
    fn from(coeffs: BiquadCoefficients) -> Self {
        RuntimeCoefficients {
            a1: coeffs.a1.to_camilla_float(),
            a2: coeffs.a2.to_camilla_float(),
            b0: coeffs.b0.to_camilla_float(),
            b1: coeffs.b1.to_camilla_float(),
            b2: coeffs.b2.to_camilla_float(),
        }
    }
}

/// Computes `a * b + c`, fused into a single instruction where the hardware
/// has fused multiply-add.
///
/// Fusing shortens the biquad feedback path and is measurably faster. It is
/// gated because on targets without hardware FMA `mul_add` lowers to a libm
/// `fma()` call, which is far slower than a separate multiply and add. The
/// gate covers aarch64, where FMA is always present, and x86 built with
/// `+fma`. Plain x86-64 falls back to `mulsd`/`addsd`, and 32-bit arm to a
/// single unfused `vmla.f64`, so the fallback costs nothing there.
///
/// Note that the fused form rounds once instead of twice, so a build that
/// takes this path differs from one that does not in the last few ulp.
#[inline(always)]
fn mul_add(a: CamillaFloat, b: CamillaFloat, c: CamillaFloat) -> CamillaFloat {
    if cfg!(any(
        all(target_arch = "aarch64", target_feature = "neon"),
        target_feature = "fma",
    )) {
        a.mul_add(b, c)
    } else {
        a * b + c
    }
}

#[derive(Clone, Debug)]
pub struct Biquad {
    samplerate: usize,
    pub s1: CamillaFloat,
    pub s2: CamillaFloat,
    coeffs: RuntimeCoefficients,
    pub name: String,
}

impl Biquad {
    /// Creates a Direct Form 2 Transposed biquad filter from a set of coefficients
    pub fn new(name: &str, samplerate: usize, coefficients: BiquadCoefficients) -> Self {
        Biquad {
            samplerate,
            s1: 0.0,
            s2: 0.0,
            coeffs: coefficients.into(),
            name: name.to_string(),
        }
    }

    /// Replace the coefficients, keeping the filter state.
    ///
    /// A compiled cascade reloads through this so the coefficients are computed
    /// once for the step rather than once per channel.
    pub fn set_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coeffs = coefficients.into();
    }

    /// Process a single sample.
    pub fn process_single(&mut self, input: CamillaFloat) -> CamillaFloat {
        let out = mul_add(self.coeffs.b0, input, self.s1);
        self.s1 = mul_add(
            -self.coeffs.a1,
            out,
            mul_add(self.coeffs.b1, input, self.s2),
        );
        self.s2 = mul_add(-self.coeffs.a2, out, self.coeffs.b2 * input);
        out
    }

    /// Flush stored subnormal numbers to zero.
    fn flush_subnormals(&mut self) {
        if self.s1.is_subnormal() {
            trace!("Biquad filter '{}', flushing subnormal s1", self.name);
            self.s1 = 0.0;
        }
        if self.s2.is_subnormal() {
            trace!("Biquad filter '{}', flushing subnormal s2", self.name);
            self.s2 = 0.0;
        }
    }
}

impl Filter for Biquad {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        for item in waveform.iter_mut() {
            *item = self.process_single(*item);
        }
        self.flush_subnormals();
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Biquad {
            parameters: conf, ..
        } = conf
        {
            self.set_coefficients(BiquadCoefficients::from_config(self.samplerate, conf));
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

// ---------------------------------------------------------------------------
// The canon
// ---------------------------------------------------------------------------
//
// A biquad is latency-bound, not throughput-bound. Each output feeds the next
// sample's state, so the core stalls on its own feedback path while the FP
// units sit idle. The fix is to keep several independent recurrences in
// flight, and there are exactly two places to find them:
//
// - Several channels at the same cascade position are independent.
// - Stage `k` working on sample `n - k` is independent of stage `k + 1`
//   working on sample `n - k - 1`. Skewing the stages that way gives one
//   independent chain per stage, and it is the only axis available to a
//   single channel.
//
// These are the same mechanism, `S` recurrences skewed in time against `C`
// recurrences side by side, so one kernel runs both at once and keeps `C * S`
// in flight. Treating them as alternatives caps the width at `max(C, S)`.
//
// It is called a canon after the musical form, where voices enter one after
// another on the same line, as in the round Frère Jacques. The stages here do
// exactly that: entering in turn during the ramp-up, all singing at once in
// the steady state, and dropping out in turn as it drains. The literature more
// often calls the shape a wavefront, as in wavefront parallelism or a
// wavefront pipeline, and a compiler would call the transformation software
// pipelining, where the three phases go by prologue, kernel and epilogue.
//
// Both are pure scheduling. Every stage still sees its samples in order with
// identical arithmetic, so the output is bit-identical to running the stages
// one at a time, whatever `C` and `S` are. That is what makes this safe to do
// by default, and it is what `bit_identical_over_the_grid` pins down.

/// Widest channel group the kernel is instantiated for.
///
/// This rarely decides anything: [`WIDTH_BUDGET`] caps the group at
/// `budget / stages`, which is already four or fewer for every depth above
/// one. It binds only on a run of single biquads across more than four
/// channels, and there four measured better than eight, 0.71 us per
/// stage-channel against 0.81, holding from 8 channels to 32.
///
/// That is the same finding as [`MAX_DEPTH`] from the other side. Width bought
/// from channels costs more than width bought from depth, and eight channels
/// wide costs 0.81 where eight stages deep costs 0.38, so there is nothing to
/// be had by buying more of the dearer one.
pub const MAX_CHANNELS: usize = 4;

/// Deepest canon the kernel is instantiated for. Longer cascades are split
/// into several passes.
///
/// Depth is the axis that pays. Measured over 1024 samples on a 32 stage
/// cascade, cost per stage falls monotonically with no sign of a floor:
///
/// ```text
///   S      1      2      3      4      5      6      7      8
///  us   2.588  1.306  0.937  0.706  0.615  0.533  0.458  0.382
/// ```
///
/// Eight is a cap, not a knee: the curve is still falling there. What lies
/// past it is unmeasured here, and the two shapes the earlier notes tried
/// disagreed, a 16 stage cascade gaining 5% at depth 16 while a 32 stage one
/// preferred depth 12. Two costs do grow with depth for certain, a kernel
/// instantiation apiece and a longer ramp-up and drain, so going deeper means
/// measuring those too and not just the steady state.
///
/// Depth also beats the channel axis at equal width: eight stages of one
/// channel costs 0.382 us per stage-channel where four stages of two channels
/// costs 0.502. So [`choose_split`] spends depth first and only buys channels with
/// what is left, which is what serves a cascade too shallow to fill the
/// budget on its own.
pub const MAX_DEPTH: usize = 8;

/// Independent recurrences to keep in flight, counted as `C * S`.
///
/// The one number the rest of the sizing reads from. `benches/biquad_canon.rs`
/// sweeps the whole `(C, S)` grid; set this from what that measures.
///
/// It is really a register budget, so it looked like it would have to differ
/// per target: a width of eight wants around 64 live doubles, which aarch64's
/// 32 registers already cannot hold and x86-64's 16 vector registers miss by
/// far more. Measured, it does not. Zen 4 spills and still lands within about
/// 10% of the throughput bound, because the spills hit L1 and that is cheap
/// beside the feedback stall the width exists to hide. One value serves both.
pub const WIDTH_BUDGET: usize = 8;

/// Splits `channels` by `depth` into a group width and a canon depth.
///
/// Both axes fill the same FP pipeline, but not at the same price: at equal
/// width, depth measured 1.3x better than channels, so it is spent first and
/// channels only take up the slack. A cascade deep enough to fill the budget
/// on its own therefore runs one channel at a time, and a shallow one, which
/// is where the channel axis earns its keep, spreads across as many channels
/// as the budget still allows.
///
/// Cheap enough to call per chunk, which is the point: `depth` changes on
/// reload when a graphic equalizer band crosses flat or a crossover changes
/// order, and a cached split would go stale.
///
/// Chunk length is deliberately not an input, though it could be. A canon pays
/// `S - 1` iterations per pass to fill and drain, which is a larger share of
/// the work the shorter the chunk, so a short chunk wants a shallower and
/// wider split than this gives it. On 16 biquads per channel this choice costs
/// 2x at 16 frames and 23% at 32, ties at 64 and wins from 128 up. Chunks that
/// short are a fringe case, so this was left out on purpose rather than
/// overlooked. Add it only with numbers from a configuration someone runs.
pub fn choose_split(channels: usize, depth: usize) -> (usize, usize) {
    let stages = depth.clamp(1, MAX_DEPTH);
    let group = channels
        .clamp(1, MAX_CHANNELS)
        .min(WIDTH_BUDGET / stages)
        .max(1);
    (group, stages)
}

/// Runs one cascade per channel over one waveform per channel.
///
/// `channel_of[i]` names the waveform that `cascades[i]` filters, and `live`
/// lists the cascades to actually run. Positions rather than references,
/// because a caller cannot hand over several disjoint `&mut` into a
/// `Vec<Vec<_>>` without somewhere to put them, and this runs on the audio path
/// where that would mean allocating every chunk.
///
/// `live` is also how a channel carrying no audio is left out. An unused
/// capture channel arrives as an empty waveform, and the kernel walks the
/// channels of a group together, so one empty waveform among them would
/// otherwise decide the length for all of them and silently leave the rest
/// unfiltered.
///
/// Every cascade named in `live` must be the same length, and every waveform
/// they name must be the same length. Both hold by construction for a compiled
/// filter step: a config step applies one `names` list to all its channels, and
/// a chunk's waveforms are either `frames` long or empty.
///
/// A cascade deeper than [`MAX_DEPTH`] runs as several passes, and each pass
/// asks [`choose_split`] again for its own depth rather than inheriting the width the
/// full depth asked for. A short final pass then has budget to spare and
/// spends it on channels. Without that, a 9 stage cascade runs its last stage
/// one channel at a time, which is the slowest point on either axis: four
/// channels of 9 stages measured 23 us that way against 15 this way, over 1024
/// frames. A depth that divides evenly is unaffected.
pub fn process_cascades(
    cascades: &mut [Vec<Biquad>],
    waveforms: &mut [Vec<CamillaFloat>],
    channel_of: &[usize],
    live: &[usize],
) {
    debug_assert!(lengths_are_uniform(cascades, waveforms, channel_of, live));
    let depth = live.first().map_or(0, |&i| cascades[i].len());
    let (_, stages) = choose_split(live.len(), depth);
    // Passes outside, channel groups inside, so a pass can be grouped
    // differently from the one before it. Every channel still sees its stages
    // in order, and the channels are independent, so this stays bit-identical.
    let mut start = 0;
    while start < depth {
        let take = (depth - start).min(stages);
        let (group, _) = choose_split(live.len(), take);
        for members in live.chunks(group) {
            dispatch_pass(cascades, waveforms, channel_of, members, start, take);
        }
        start += take;
    }
}

/// [`process_cascades`] with the split given rather than chosen, and used for
/// every pass. For sizing benchmarks, which need to ask for a split that
/// [`choose_split`] would not pick.
pub fn process_cascades_with_split(
    cascades: &mut [Vec<Biquad>],
    waveforms: &mut [Vec<CamillaFloat>],
    channel_of: &[usize],
    live: &[usize],
    group: usize,
    stages: usize,
) {
    debug_assert!((1..=MAX_CHANNELS).contains(&group));
    debug_assert!((1..=MAX_DEPTH).contains(&stages));
    debug_assert!(lengths_are_uniform(cascades, waveforms, channel_of, live));
    for members in live.chunks(group) {
        let depth = cascades[members[0]].len();
        let mut start = 0;
        while start < depth {
            let take = (depth - start).min(stages);
            dispatch_pass(cascades, waveforms, channel_of, members, start, take);
            start += take;
        }
    }
}

/// The shape the kernel relies on: every live cascade the same length, and
/// every waveform they name the same length.
///
/// `live[0]` is safe on an empty `live` because `all` returns without reaching
/// the closure, not because of where this is called from. Only called from
/// `debug_assert!`, so it costs nothing in a release build.
fn lengths_are_uniform(
    cascades: &[Vec<Biquad>],
    waveforms: &[Vec<CamillaFloat>],
    channel_of: &[usize],
    live: &[usize],
) -> bool {
    live.iter()
        .all(|&i| cascades[i].len() == cascades[live[0]].len())
        && live
            .iter()
            .all(|&i| waveforms[channel_of[i]].len() == waveforms[channel_of[live[0]]].len())
}

/// Turns a runtime `(channels, depth)` into a call to the kernel instantiated
/// for it.
///
/// The whole `MAX_CHANNELS` by `MAX_DEPTH` grid is instantiated, thirty-two
/// kernels. Every one of them is reachable: [`choose_split`] can ask for any depth up
/// to `MAX_DEPTH`, and a cascade whose length is not a multiple of it ends on a
/// shorter pass.
fn dispatch_pass(
    cascades: &mut [Vec<Biquad>],
    waveforms: &mut [Vec<CamillaFloat>],
    channel_of: &[usize],
    members: &[usize],
    start: usize,
    depth: usize,
) {
    macro_rules! by_depth {
        ($c:literal) => {
            match depth {
                1 => run_group::<$c, 1>(cascades, waveforms, channel_of, members, start),
                2 => run_group::<$c, 2>(cascades, waveforms, channel_of, members, start),
                3 => run_group::<$c, 3>(cascades, waveforms, channel_of, members, start),
                4 => run_group::<$c, 4>(cascades, waveforms, channel_of, members, start),
                5 => run_group::<$c, 5>(cascades, waveforms, channel_of, members, start),
                6 => run_group::<$c, 6>(cascades, waveforms, channel_of, members, start),
                7 => run_group::<$c, 7>(cascades, waveforms, channel_of, members, start),
                8 => run_group::<$c, 8>(cascades, waveforms, channel_of, members, start),
                _ => unreachable!("depth is clamped to MAX_DEPTH"),
            }
        };
    }
    match members.len() {
        1 => by_depth!(1),
        2 => by_depth!(2),
        3 => by_depth!(3),
        4 => by_depth!(4),
        _ => unreachable!("chunks yields at most MAX_CHANNELS"),
    }
}

/// Runs one cascade over one waveform.
///
/// The exception rather than the rule: for a filter that has only its own
/// channel to work with, so all the width has to come from cascade depth. A
/// step reaching several channels should use [`process_cascades`], which can
/// spend the channel axis too.
pub fn process_mono_cascade(stages: &mut [Biquad], waveform: &mut [CamillaFloat]) {
    let (_, depth) = choose_split(1, stages.len());
    for in_pass in stages.chunks_mut(depth) {
        match in_pass.len() {
            1 => run_mono::<1>(in_pass, waveform),
            2 => run_mono::<2>(in_pass, waveform),
            3 => run_mono::<3>(in_pass, waveform),
            4 => run_mono::<4>(in_pass, waveform),
            5 => run_mono::<5>(in_pass, waveform),
            6 => run_mono::<6>(in_pass, waveform),
            7 => run_mono::<7>(in_pass, waveform),
            8 => run_mono::<8>(in_pass, waveform),
            _ => unreachable!("depth is clamped to MAX_DEPTH"),
        }
    }
}

fn run_mono<const S: usize>(stages: &mut [Biquad], waveform: &mut [CamillaFloat]) {
    let mut voices = Voices::<1, S>::default();
    load(&mut voices, 0, stages);
    let (s1, s2) = canon::<1, S>(voices, [waveform]);
    store(&s1, &s2, 0, stages);
}

/// Loads one pass into registers, runs it, and stores the state back.
///
/// The load and store happen once per pass, never on the per-sample path, so
/// the strided reads out of `cascades` cost nothing measurable. That is what
/// leaves the buffer layout free to be chosen for the reload path instead.
fn run_group<const C: usize, const S: usize>(
    cascades: &mut [Vec<Biquad>],
    waveforms: &mut [Vec<CamillaFloat>],
    channel_of: &[usize],
    members: &[usize],
    start: usize,
) {
    let mut voices = Voices::<C, S>::default();
    for c in 0..C {
        load(&mut voices, c, &cascades[members[c]][start..start + S]);
    }

    // Handed over as a fixed-size array so the kernel subscripts it with a
    // constant, which is what keeps the bounds checks out of the sample loop.
    let channels: [usize; C] = std::array::from_fn(|c| channel_of[members[c]]);
    let waves = waveforms
        .get_disjoint_mut(channels)
        .expect("each cascade of a group filters a different channel")
        .map(|w| &mut w[..]);

    let (s1, s2) = canon::<C, S>(voices, waves);

    for c in 0..C {
        store(&s1, &s2, c, &mut cascades[members[c]][start..start + S]);
    }
}

fn load<const C: usize, const S: usize>(voices: &mut Voices<C, S>, c: usize, stages: &[Biquad]) {
    for (k, stage) in stages.iter().enumerate().take(S) {
        voices.b0[c][k] = stage.coeffs.b0;
        voices.b1[c][k] = stage.coeffs.b1;
        voices.b2[c][k] = stage.coeffs.b2;
        voices.a1[c][k] = stage.coeffs.a1;
        voices.a2[c][k] = stage.coeffs.a2;
        voices.s1[c][k] = stage.s1;
        voices.s2[c][k] = stage.s2;
    }
}

fn store<const C: usize, const S: usize>(
    s1: &[[CamillaFloat; S]; C],
    s2: &[[CamillaFloat; S]; C],
    c: usize,
    stages: &mut [Biquad],
) {
    for (k, stage) in stages.iter_mut().enumerate().take(S) {
        stage.s1 = s1[c][k];
        stage.s2 = s2[c][k];
        stage.flush_subnormals();
    }
}

/// The voices of one canon: every stage of every channel in the pass, held by
/// value so the whole thing can live in registers.
///
/// One array per coefficient rather than one array of whole stages.
/// Holding a canon takes seven floats per stage and aarch64 has 32 FP
/// registers to hold them in, so past a width of four something has to live in
/// memory and the layout decides what. Measured either way it came out the
/// same, so this is not the reason the kernel is fast; it is kept because it
/// leaves the choice to the compiler a value at a time rather than a stage at
/// a time, which is the shape that cannot get worse.
struct Voices<const C: usize, const S: usize> {
    s1: [[CamillaFloat; S]; C],
    s2: [[CamillaFloat; S]; C],
    b0: [[CamillaFloat; S]; C],
    b1: [[CamillaFloat; S]; C],
    b2: [[CamillaFloat; S]; C],
    a1: [[CamillaFloat; S]; C],
    a2: [[CamillaFloat; S]; C],
}

impl<const C: usize, const S: usize> Default for Voices<C, S> {
    fn default() -> Self {
        let zeros = [[0.0 as CamillaFloat; S]; C];
        Voices {
            s1: zeros,
            s2: zeros,
            b0: zeros,
            b1: zeros,
            b2: zeros,
            a1: zeros,
            a2: zeros,
        }
    }
}

/// The sample loop, on values rather than on anything the caller can reach.
///
/// State and coefficients arrive and leave by value so nothing in the loop
/// sits behind a pointer the compiler has to reason about. Reaching the stages
/// through a trait object here instead measured 10.28 us against 6.39 us for a
/// 16 stage cascade over 1024 samples, a 60% penalty for two opaque calls that
/// run once per pass.
///
/// `pipe[c][k]` holds stage `k`'s output from the previous iteration, which
/// stage `k + 1` consumes in this one. Walking the stages downwards leaves
/// that value unread until it has been used, so one array serves as the whole
/// skew buffer.
///
/// The canon takes `n + S - 1` iterations for `n` samples: a ramp-up while
/// the deeper stages have not been reached, a steady state with every stage
/// busy, and a drain once the input is exhausted. Stages not yet or no longer
/// active must be **skipped, not fed zeros**, or their state advances past the
/// end of the chunk. That is the one bug here that would be silent and
/// cumulative.
///
/// Neither loop counter here is a plain subscript, which is why the lint is
/// off for the whole function. `i` is the canon position and doubles as
/// the bound on how many stages have been reached, and `c` steps several
/// arrays plus a waveform in lockstep. Iterating any one of them, which is
/// what the lint asks for, would walk the wrong axis and undo the scheduling.
#[allow(clippy::needless_range_loop)]
fn canon<const C: usize, const S: usize>(
    voices: Voices<C, S>,
    waveforms: [&mut [CamillaFloat]; C],
) -> ([[CamillaFloat; S]; C], [[CamillaFloat; S]; C]) {
    let Voices {
        mut s1,
        mut s2,
        b0,
        b1,
        b2,
        a1,
        a2,
    } = voices;

    // One stage of one channel advanced by one sample. Identical arithmetic to
    // `Biquad::process_single`, on locals that stay in registers.
    macro_rules! stage {
        ($c:expr, $k:expr, $x:expr) => {{
            let (c, k) = ($c, $k);
            let input = $x;
            let out = mul_add(b0[c][k], input, s1[c][k]);
            s1[c][k] = mul_add(-a1[c][k], out, mul_add(b1[c][k], input, s2[c][k]));
            s2[c][k] = mul_add(-a2[c][k], out, b2[c][k] * input);
            out
        }};
    }

    let n = waveforms[0].len();
    // Reborrowing at exactly `n` is what tells the compiler that every
    // `waves[c][i]` with `i < n` is in range.
    let waves = waveforms.map(|w| &mut w[..n]);
    let mut pipe = [[0.0 as CamillaFloat; S]; C];

    // The bounds below read like special cases for a waveform shorter than the
    // cascade, and they are, but they are also what keeps this fast. They are
    // how the loops tell the compiler their indices are inside the slice:
    // `(S - 1).min(n)` says `i < n`, and the guarded store says the same for
    // the drain. Rewriting them into the form a guaranteed minimum length
    // would allow, an unguarded `waveform[n - S + first]`, measured 6.40 us
    // against 13.35 for a 16 stage cascade over 1024 samples. The indices stop
    // being provably in range, and the panic paths that appear block the
    // optimisation of everything around them. Stating `assert!(n >= S)`
    // instead recovers only part of it, 10.15 us. Leave them alone.
    //
    // Ramp-up: stage `k` has seen no sample yet while `k > i`.
    let ramp = (S - 1).min(n);
    for i in 0..ramp {
        for k in (1..=i).rev() {
            for c in 0..C {
                pipe[c][k] = stage!(c, k, pipe[c][k - 1]);
            }
        }
        for c in 0..C {
            pipe[c][0] = stage!(c, 0, waves[c][i]);
        }
    }

    // Steady state: every stage busy on a different sample.
    for i in (S - 1)..n {
        for k in (1..S).rev() {
            for c in 0..C {
                pipe[c][k] = stage!(c, k, pipe[c][k - 1]);
            }
        }
        for c in 0..C {
            pipe[c][0] = stage!(c, 0, waves[c][i]);
        }
        for c in 0..C {
            waves[c][i - (S - 1)] = pipe[c][S - 1];
        }
    }

    // Drain: stage `k` is finished once `i - k` reaches `n`.
    for i in n..(n + S - 1) {
        let first = i - n + 1;
        let last = (S - 1).min(i);
        for k in (first..=last).rev() {
            for c in 0..C {
                pipe[c][k] = stage!(c, k, pipe[c][k - 1]);
            }
        }
        if i >= S - 1 {
            for c in 0..C {
                waves[c][i - (S - 1)] = pipe[c][S - 1];
            }
        }
    }

    (s1, s2)
}

pub fn validate_config(samplerate: usize, parameters: &config::BiquadParameters) -> Res<()> {
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
            if *freq <= 0.0 {
                return Err(config::ConfigError::new("Frequency must be > 0").into());
            } else if *freq >= maxfreq {
                return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
            }
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
        | config::BiquadParameters::GeneralNotch(config::GeneralNotchParams { q_p: q, .. })
            if *q <= 0.0 =>
        {
            return Err(config::ConfigError::new("Q must be > 0").into());
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
            return Err(config::ConfigError::new("Bandwidth must be > 0").into());
        }
        _ => {}
    }
    // Check slope
    match parameters {
        config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope { slope, .. })
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope { slope, .. }) => {
            if *slope <= 0.0 {
                return Err(config::ConfigError::new("Slope must be > 0").into());
            } else if *slope > 12.0 {
                return Err(config::ConfigError::new("Slope must be <= 12.0").into());
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
        if *freq_act <= 0.0 || *freq_target <= 0.0 {
            return Err(config::ConfigError::new("Frequency must be > 0").into());
        } else if *freq_act >= maxfreq || *freq_target >= maxfreq {
            return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
        }
        if *q_act <= 0.0 || *q_target <= 0.0 {
            return Err(config::ConfigError::new("Q must be > 0").into());
        }
    }
    // Check GeneralNotch frequencies
    if let config::BiquadParameters::GeneralNotch(params) = parameters {
        if params.freq_p <= 0.0 || params.freq_z <= 0.0 {
            return Err(config::ConfigError::new("Pole and zero frequencies must be > 0").into());
        } else if params.freq_p >= maxfreq || params.freq_z >= maxfreq {
            return Err(config::ConfigError::new(
                "Pole and zero frequencies must be < samplerate/2",
            )
            .into());
        }
    }
    let coeffs = BiquadCoefficients::from_config(samplerate, parameters.clone());
    if !coeffs.is_stable() {
        return Err(config::ConfigError::new("Unstable filter specified").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::CamillaFloat;
    use crate::config::{
        BiquadParameters, GeneralNotchParams, NotchWidth, PeakingWidth, ShelfSteepness,
    };
    use crate::filters::Filter;
    use crate::filters::biquad::{
        Biquad, BiquadCoefficients, MAX_CHANNELS, MAX_DEPTH, WIDTH_BUDGET, choose_split,
        process_cascades, process_cascades_with_split, validate_config,
    };
    use num_complex::Complex;

    fn is_close(left: f64, right: f64, maxdiff: f64) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn is_close_relative(left: f64, right: f64, maxdiff: f64) -> bool {
        println!("{left} - {right}");
        (left / right - 1.0).abs() < maxdiff
    }

    // Waveforms are at processing precision, unlike the coefficients above.
    fn compare_waveforms(
        left: Vec<CamillaFloat>,
        right: Vec<CamillaFloat>,
        maxdiff: CamillaFloat,
    ) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            println!("{val_l} - {val_r}");
            if (val_l - val_r).abs() >= maxdiff {
                return false;
            }
        }
        true
    }

    fn gain_and_phase(coeffs: BiquadCoefficients, f: f64, fs: usize) -> (f64, f64) {
        let pi = std::f64::consts::PI;
        let z = (Complex::i() * 2.0 * pi * f / (fs as f64)).exp();
        let a = (coeffs.b0 + coeffs.b1 * z.powi(-1) + coeffs.b2 * z.powi(-2))
            / (1.0 + coeffs.a1 * z.powi(-1) + coeffs.a2 * z.powi(-2));
        let (magn, ang) = a.to_polar();
        let gain = 20.0 * magn.log10();
        let phase = 180.0 / pi * ang;
        (gain, phase)
    }

    #[test]
    fn check_result() {
        let conf = BiquadParameters::Lowpass {
            freq: 10000.0,
            q: 0.5,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![0.215, 0.461, 0.281, 0.039, 0.004, 0.0, 0.0, 0.0];
        let mut filter = Biquad::new("test", 44100, coeffs);
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-3));
    }

    #[test]
    fn make_lowpass() {
        let conf = BiquadParameters::Lowpass {
            freq: 100.0,
            q: std::f64::consts::FRAC_1_SQRT_2,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 10.0, 44100);
        assert!(is_close(gain_f0, -3.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, -24.0, 0.2));
    }

    #[test]
    fn make_highpass() {
        let conf = BiquadParameters::Highpass {
            freq: 100.0,
            q: std::f64::consts::FRAC_1_SQRT_2,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 25.0, 44100);
        assert!(is_close(gain_f0, -3.0, 0.1));
        assert!(is_close(gain_lf, -24.0, 0.2));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn make_lowpass_fo() {
        let conf = BiquadParameters::LowpassFO { freq: 100.0 };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 10.0, 44100);
        assert!(is_close(gain_f0, -3.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, -12.3, 0.1));
    }

    #[test]
    fn make_highpass_fo() {
        let conf = BiquadParameters::HighpassFO { freq: 100.0 };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 800.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 25.0, 44100);
        assert!(is_close(gain_f0, -3.0, 0.1));
        assert!(is_close(gain_lf, -12.3, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn make_peaking() {
        let conf = BiquadParameters::Peaking(PeakingWidth::Q {
            freq: 100.0,
            gain: 7.0,
            q: 3.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 25.0, 44100);
        assert!(is_close(gain_f0, 7.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn make_bandpass() {
        let conf = BiquadParameters::Bandpass(NotchWidth::Q {
            freq: 100.0,
            q: 1.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 25.0, 44100);
        assert!(is_close(gain_f0, 0.0, 0.1));
        assert!(is_close(gain_lf, -12.0, 0.3));
        assert!(is_close(gain_hf, -12.0, 0.3));
    }

    #[test]
    fn make_notch() {
        let conf = BiquadParameters::Notch(NotchWidth::Q {
            freq: 100.0,
            q: 3.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 400.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 25.0, 44100);
        assert!(gain_f0 < -40.0);
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn make_generalnotch_hp() {
        let conf = BiquadParameters::GeneralNotch(GeneralNotchParams {
            freq_p: 2000.0,
            freq_z: 1000.0,
            q_p: 1.0,
            normalize_at_dc: Some(false),
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_fp, _) = gain_and_phase(coeffs, 1000.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 20000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        println!("{gain_fp} {gain_hf} {gain_lf}");
        assert!(gain_fp < -40.0);
        assert!(is_close(gain_lf, -12.1, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn make_generalnotch_lp() {
        let conf = BiquadParameters::GeneralNotch(GeneralNotchParams {
            freq_p: 500.0,
            freq_z: 1000.0,
            q_p: 1.0,
            normalize_at_dc: Some(true),
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_fp, _) = gain_and_phase(coeffs, 1000.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 20000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        println!("{gain_fp} {gain_hf} {gain_lf}");
        assert!(gain_fp < -40.0);
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, -12.1, 0.1));
    }

    #[test]
    fn make_allpass() {
        let conf = BiquadParameters::Allpass(NotchWidth::Q {
            freq: 100.0,
            q: 3.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, phase_f0) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, phase_hf) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, phase_lf) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, 0.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
        assert!(is_close(phase_f0.abs(), 180.0, 0.5));
        assert!(is_close(phase_lf, 0.0, 0.5));
        assert!(is_close(phase_hf, 0.0, 0.5));
    }

    #[test]
    fn make_allpass_fo() {
        let conf = BiquadParameters::AllpassFO { freq: 100.0 };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, phase_f0) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, phase_hf) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, phase_lf) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, 0.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
        assert!(is_close(phase_f0.abs(), 90.0, 0.5));
        assert!(is_close(phase_lf, 0.0, 2.0));
        assert!(is_close(phase_hf.abs(), 180.0, 2.0));
    }

    #[test]
    fn make_highshelf() {
        let conf = BiquadParameters::Highshelf(ShelfSteepness::Slope {
            freq: 100.0,
            slope: 6.0,
            gain: -24.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_f0h, _) = gain_and_phase(coeffs, 200.0, 44100);
        let (gain_f0l, _) = gain_and_phase(coeffs, 50.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, -12.0, 0.1));
        assert!(is_close(gain_f0h, -18.0, 1.0));
        assert!(is_close(gain_f0l, -6.0, 1.0));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, -24.0, 0.1));
    }

    #[test]
    fn make_lowshelf() {
        let conf = BiquadParameters::Lowshelf(ShelfSteepness::Slope {
            freq: 100.0,
            slope: 6.0,
            gain: -24.0,
        });
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_f0h, _) = gain_and_phase(coeffs, 200.0, 44100);
        let (gain_f0l, _) = gain_and_phase(coeffs, 50.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, -12.0, 0.1));
        assert!(is_close(gain_f0h, -6.0, 1.0));
        assert!(is_close(gain_f0l, -18.0, 1.0));
        assert!(is_close(gain_lf, -24.0, 0.1));
        assert!(is_close(gain_hf, -0.0, 0.1));
    }

    #[test]
    fn lowshelf_slope_vs_q() {
        let conf_slope = BiquadParameters::Lowshelf(ShelfSteepness::Slope {
            freq: 100.0,
            slope: 12.0,
            gain: -24.0,
        });
        let conf_q = BiquadParameters::Lowshelf(ShelfSteepness::Q {
            freq: 100.0,
            q: std::f64::consts::FRAC_1_SQRT_2,
            gain: -24.0,
        });
        let coeffs_slope = BiquadCoefficients::from_config(44100, conf_slope);
        let coeffs_q = BiquadCoefficients::from_config(44100, conf_q);
        assert!(is_close_relative(coeffs_slope.a1, coeffs_q.a1, 0.001));
        assert!(is_close_relative(coeffs_slope.a2, coeffs_q.a2, 0.001));
        assert!(is_close_relative(coeffs_slope.b0, coeffs_q.b0, 0.001));
        assert!(is_close_relative(coeffs_slope.b1, coeffs_q.b1, 0.001));
        assert!(is_close_relative(coeffs_slope.b2, coeffs_q.b2, 0.001));
    }

    #[test]
    fn highshelf_slope_vs_q() {
        let conf_slope = BiquadParameters::Highshelf(ShelfSteepness::Slope {
            freq: 100.0,
            slope: 12.0,
            gain: -24.0,
        });
        let conf_q = BiquadParameters::Highshelf(ShelfSteepness::Q {
            freq: 100.0,
            q: std::f64::consts::FRAC_1_SQRT_2,
            gain: -24.0,
        });
        let coeffs_slope = BiquadCoefficients::from_config(44100, conf_slope);
        let coeffs_q = BiquadCoefficients::from_config(44100, conf_q);
        assert!(is_close_relative(coeffs_slope.a1, coeffs_q.a1, 0.001));
        assert!(is_close_relative(coeffs_slope.a2, coeffs_q.a2, 0.001));
        assert!(is_close_relative(coeffs_slope.b0, coeffs_q.b0, 0.001));
        assert!(is_close_relative(coeffs_slope.b1, coeffs_q.b1, 0.001));
        assert!(is_close_relative(coeffs_slope.b2, coeffs_q.b2, 0.001));
    }

    #[test]
    fn bandpass_bw_vs_q() {
        let conf_bw = BiquadParameters::Bandpass(NotchWidth::Bandwidth {
            freq: 100.0,
            bandwidth: 1.0,
        });
        let conf_q = BiquadParameters::Bandpass(NotchWidth::Q {
            freq: 100.0,
            q: std::f64::consts::SQRT_2,
        });
        let coeffs_bw = BiquadCoefficients::from_config(44100, conf_bw);
        let coeffs_q = BiquadCoefficients::from_config(44100, conf_q);
        assert!(is_close_relative(coeffs_bw.a1, coeffs_q.a1, 0.001));
        assert!(is_close_relative(coeffs_bw.a2, coeffs_q.a2, 0.001));
        assert!(is_close_relative(coeffs_bw.b0, coeffs_q.b0, 0.001));
        assert_eq!(coeffs_bw.b1, 0.0);
        assert_eq!(coeffs_q.b1, 0.0);
        assert!(is_close_relative(coeffs_bw.b2, coeffs_q.b2, 0.001));
    }

    #[test]
    fn notch_bw_vs_q() {
        let conf_bw = BiquadParameters::Notch(NotchWidth::Bandwidth {
            freq: 100.0,
            bandwidth: 1.0,
        });
        let conf_q = BiquadParameters::Notch(NotchWidth::Q {
            freq: 100.0,
            q: std::f64::consts::SQRT_2,
        });
        let coeffs_bw = BiquadCoefficients::from_config(44100, conf_bw);
        let coeffs_q = BiquadCoefficients::from_config(44100, conf_q);
        assert!(is_close_relative(coeffs_bw.a1, coeffs_q.a1, 0.001));
        assert!(is_close_relative(coeffs_bw.a2, coeffs_q.a2, 0.001));
        assert!(is_close_relative(coeffs_bw.b0, coeffs_q.b0, 0.001));
        assert!(is_close_relative(coeffs_bw.b1, coeffs_q.b1, 0.001));
        assert!(is_close_relative(coeffs_bw.b2, coeffs_q.b2, 0.001));
    }

    #[test]
    fn allpass_bw_vs_q() {
        let conf_bw = BiquadParameters::Allpass(NotchWidth::Bandwidth {
            freq: 100.0,
            bandwidth: 1.0,
        });
        let conf_q = BiquadParameters::Allpass(NotchWidth::Q {
            freq: 100.0,
            q: std::f64::consts::SQRT_2,
        });
        let coeffs_bw = BiquadCoefficients::from_config(44100, conf_bw);
        let coeffs_q = BiquadCoefficients::from_config(44100, conf_q);
        assert!(is_close_relative(coeffs_bw.a1, coeffs_q.a1, 0.001));
        assert!(is_close_relative(coeffs_bw.a2, coeffs_q.a2, 0.001));
        assert!(is_close_relative(coeffs_bw.b0, coeffs_q.b0, 0.001));
        assert!(is_close_relative(coeffs_bw.b1, coeffs_q.b1, 0.001));
        assert!(is_close_relative(coeffs_bw.b2, coeffs_q.b2, 0.001));
    }

    #[test]
    fn make_highshelf_fo() {
        let conf = BiquadParameters::HighshelfFO {
            freq: 100.0,
            gain: -12.0,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, -6.0, 0.1));
        assert!(is_close(gain_lf, 0.0, 0.1));
        assert!(is_close(gain_hf, -12.0, 0.1));
    }

    #[test]
    fn make_lowshelf_fo() {
        let conf = BiquadParameters::LowshelfFO {
            freq: 100.0,
            gain: -12.0,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_f0, _) = gain_and_phase(coeffs, 100.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 10000.0, 44100);
        let (gain_lf, _) = gain_and_phase(coeffs, 1.0, 44100);
        assert!(is_close(gain_f0, -6.0, 0.1));
        assert!(is_close(gain_lf, -12.0, 0.1));
        assert!(is_close(gain_hf, -0.0, 0.1));
    }
    #[test]
    fn make_lt() {
        let conf = BiquadParameters::LinkwitzTransform {
            freq_act: 100.0,
            q_act: 1.2,
            freq_target: 25.0,
            q_target: 0.7,
        };
        let coeffs = BiquadCoefficients::from_config(44100, conf);
        assert!(coeffs.is_stable());
        let (gain_10, _) = gain_and_phase(coeffs, 10.0, 44100);
        let (gain_87, _) = gain_and_phase(coeffs, 87.0, 44100);
        let (gain_123, _) = gain_and_phase(coeffs, 123.0, 44100);
        let (gain_hf, _) = gain_and_phase(coeffs, 10000.0, 44100);
        assert!(is_close(gain_10, 23.9, 0.1));
        assert!(is_close(gain_87, 0.0, 0.1));
        assert!(is_close(gain_123, -2.4, 0.1));
        assert!(is_close(gain_hf, 0.0, 0.1));
    }

    #[test]
    fn check_freq_q() {
        let fs = 48000;
        let okconf1 = BiquadParameters::Peaking(PeakingWidth::Q {
            freq: 1000.0,
            q: 2.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &okconf1).is_ok());
        let badconf1 = BiquadParameters::Peaking(PeakingWidth::Q {
            freq: 1000.0,
            q: 0.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &badconf1).is_err());
        let badconf2 = BiquadParameters::Peaking(PeakingWidth::Q {
            freq: 25000.0,
            q: 1.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &badconf2).is_err());
        let badconf3 = BiquadParameters::Peaking(PeakingWidth::Q {
            freq: 0.0,
            q: 1.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &badconf3).is_err());
    }

    #[test]
    fn check_slope() {
        let fs = 48000;
        let okconf1 = BiquadParameters::Highshelf(ShelfSteepness::Slope {
            freq: 1000.0,
            slope: 5.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &okconf1).is_ok());
        let badconf1 = BiquadParameters::Highshelf(ShelfSteepness::Slope {
            freq: 1000.0,
            slope: 0.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &badconf1).is_err());
        let badconf2 = BiquadParameters::Highshelf(ShelfSteepness::Slope {
            freq: 1000.0,
            slope: 15.0,
            gain: 1.23,
        });
        assert!(validate_config(fs, &badconf2).is_err());
    }

    // ---------------------------------------------------------------------
    // The canon
    // ---------------------------------------------------------------------

    /// A cascade of `stages` biquads, each with different coefficients so a
    /// stage running out of order or twice cannot go unnoticed.
    fn cascade(stages: usize, seed: usize) -> Vec<Biquad> {
        (0..stages)
            .map(|k| {
                let conf = BiquadParameters::Peaking(PeakingWidth::Q {
                    freq: 100.0 + 137.0 * ((k + 3 * seed) as f64),
                    q: 0.7 + 0.05 * (k as f64),
                    gain: 1.5 + 0.25 * (seed as f64),
                });
                Biquad::new("t", 44100, BiquadCoefficients::from_config(44100, conf))
            })
            .collect()
    }

    fn signal(n: usize, seed: usize) -> Vec<CamillaFloat> {
        (0..n)
            .map(|i| (0.017 * ((7 * i + 13 * seed) as CamillaFloat)).sin() * 0.5)
            .collect()
    }

    /// Cascade `i` filters waveform `i`, and every channel is live. The shape a
    /// compiled step has when no capture channel is unused.
    fn straight(channels: usize) -> Vec<usize> {
        (0..channels).collect()
    }

    /// What the kernel has to reproduce exactly: every stage run over the whole
    /// waveform in order, one at a time.
    fn sequential(cascades: &mut [Vec<Biquad>], waveforms: &mut [Vec<CamillaFloat>]) {
        for (cascade, waveform) in cascades.iter_mut().zip(waveforms.iter_mut()) {
            for stage in cascade.iter_mut() {
                stage.process_waveform(waveform);
            }
        }
    }

    /// Bit equality, not float equality: `-0.0 == 0.0` compares equal but is a
    /// different value, and this test exists to catch exactly that kind of drift.
    fn assert_same(left: &[Vec<CamillaFloat>], right: &[Vec<CamillaFloat>], what: &str) {
        assert_eq!(left.len(), right.len(), "{what}: channel count");
        for (ch, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(l.len(), r.len(), "{what}: length of channel {ch}");
            for (i, (a, b)) in l.iter().zip(r.iter()).enumerate() {
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "{what}: channel {ch} sample {i}, {a} against {b}"
                );
            }
        }
    }

    fn assert_same_state(left: &[Vec<Biquad>], right: &[Vec<Biquad>], what: &str) {
        for (ch, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            for (k, (a, b)) in l.iter().zip(r.iter()).enumerate() {
                assert_eq!(
                    (a.s1.to_bits(), a.s2.to_bits()),
                    (b.s1.to_bits(), b.s2.to_bits()),
                    "{what}: state of channel {ch} stage {k}"
                );
            }
        }
    }

    /// The whole point of the kernel: whatever `(C, S)` it is dispatched at, the
    /// output and the leftover state match running the stages one at a time.
    /// This is what makes the scheduling safe to apply by default.
    #[test]
    fn bit_identical_over_the_grid() {
        for channels in 1..=MAX_CHANNELS {
            // Depths past MAX_DEPTH exercise the split into several passes.
            for depth in 1..=(MAX_DEPTH + 3) {
                for n in [0, 1, 2, 3, 7, 8, 9, 64, 257] {
                    let mut want_c: Vec<Vec<Biquad>> =
                        (0..channels).map(|c| cascade(depth, c)).collect();
                    let mut want_w: Vec<Vec<CamillaFloat>> =
                        (0..channels).map(|c| signal(n, c)).collect();
                    sequential(&mut want_c, &mut want_w);

                    for group in 1..=MAX_CHANNELS {
                        for stages in 1..=MAX_DEPTH {
                            let mut got_c: Vec<Vec<Biquad>> =
                                (0..channels).map(|c| cascade(depth, c)).collect();
                            let mut got_w: Vec<Vec<CamillaFloat>> =
                                (0..channels).map(|c| signal(n, c)).collect();
                            let ids = straight(channels);
                            process_cascades_with_split(
                                &mut got_c, &mut got_w, &ids, &ids, group, stages,
                            );
                            let what = format!(
                                "channels {channels} depth {depth} n {n} at C {group} S {stages}"
                            );
                            assert_same(&got_w, &want_w, &what);
                            assert_same_state(&got_c, &want_c, &what);
                        }
                    }
                }
            }
        }
    }

    /// The split the pipeline will actually ask for has to agree too.
    ///
    /// The depths past `MAX_DEPTH` are the ones that run as several passes, and
    /// 9, 10 and 12 are there because their last pass is shallower than the
    /// ones before it and so gets grouped across more channels. That is the
    /// case where two passes of the same cascade run at different widths.
    #[test]
    fn chosen_split_is_bit_identical() {
        for channels in 1..=8 {
            for depth in [1, 2, 3, 4, 7, 8, 9, 10, 12, 16, 31] {
                let mut want_c: Vec<Vec<Biquad>> =
                    (0..channels).map(|c| cascade(depth, c)).collect();
                let mut want_w: Vec<Vec<CamillaFloat>> =
                    (0..channels).map(|c| signal(1024, c)).collect();
                sequential(&mut want_c, &mut want_w);

                let mut got_c: Vec<Vec<Biquad>> =
                    (0..channels).map(|c| cascade(depth, c)).collect();
                let mut got_w: Vec<Vec<CamillaFloat>> =
                    (0..channels).map(|c| signal(1024, c)).collect();
                let ids = straight(channels);
                process_cascades(&mut got_c, &mut got_w, &ids, &ids);
                let what = format!("channels {channels} depth {depth}");
                assert_same(&got_w, &want_w, &what);
                assert_same_state(&got_c, &want_c, &what);
            }
        }
    }

    /// The drain has to leave the state exactly where a sequential run would, or
    /// the error accumulates from one chunk to the next instead of showing up
    /// once. Depth 12 also puts a pass boundary inside every chunk, so this
    /// crosses both boundaries at the same time.
    #[test]
    fn state_survives_chunk_boundaries() {
        const DEPTH: usize = 12;
        const CHANNELS: usize = 3;
        const CHUNK: usize = 5;
        const CHUNKS: usize = 9;

        let mut want_c: Vec<Vec<Biquad>> = (0..CHANNELS).map(|c| cascade(DEPTH, c)).collect();
        let mut want_w: Vec<Vec<CamillaFloat>> =
            (0..CHANNELS).map(|c| signal(CHUNK * CHUNKS, c)).collect();
        sequential(&mut want_c, &mut want_w);

        let mut got_c: Vec<Vec<Biquad>> = (0..CHANNELS).map(|c| cascade(DEPTH, c)).collect();
        let source: Vec<Vec<CamillaFloat>> =
            (0..CHANNELS).map(|c| signal(CHUNK * CHUNKS, c)).collect();
        let ids = straight(CHANNELS);
        let mut got_w: Vec<Vec<CamillaFloat>> = vec![Vec::new(); CHANNELS];
        for chunk in 0..CHUNKS {
            let mut piece: Vec<Vec<CamillaFloat>> = source
                .iter()
                .map(|w| w[chunk * CHUNK..(chunk + 1) * CHUNK].to_vec())
                .collect();
            process_cascades(&mut got_c, &mut piece, &ids, &ids);
            for (whole, part) in got_w.iter_mut().zip(piece) {
                whole.extend(part);
            }
        }
        assert_same(&got_w, &want_w, "chunked");
        assert_same_state(&got_c, &want_c, "chunked");
    }

    /// An unused capture channel arrives as an empty waveform. The kernel walks
    /// the channels of a group together, so leaving one in would decide the
    /// length for the whole group; `live` is how the caller keeps it out, and
    /// the channels that do carry audio must come out filtered anyway.
    #[test]
    fn an_empty_channel_does_not_silence_its_group() {
        const CHANNELS: usize = 4;
        // Channels 1 and 3 carry no audio, as they would ahead of a mixer that
        // only reads 0 and 2.
        let live = vec![0usize, 2];

        let mut want_c: Vec<Vec<Biquad>> = (0..CHANNELS).map(|c| cascade(6, c)).collect();
        let mut want_w: Vec<Vec<CamillaFloat>> = (0..CHANNELS)
            .map(|c| {
                if live.contains(&c) {
                    signal(64, c)
                } else {
                    Vec::new()
                }
            })
            .collect();
        sequential(&mut want_c, &mut want_w);

        let mut got_c: Vec<Vec<Biquad>> = (0..CHANNELS).map(|c| cascade(6, c)).collect();
        let mut got_w: Vec<Vec<CamillaFloat>> = (0..CHANNELS)
            .map(|c| {
                if live.contains(&c) {
                    signal(64, c)
                } else {
                    Vec::new()
                }
            })
            .collect();
        process_cascades(&mut got_c, &mut got_w, &straight(CHANNELS), &live);

        assert_same(&got_w, &want_w, "one empty channel in the group");
        for &c in &live {
            assert_same_state(&got_c[c..=c], &want_c[c..=c], "live channel");
        }
        // The channels that were left out must not have advanced at all.
        for c in [1usize, 3] {
            for stage in &got_c[c] {
                assert_eq!(stage.s1, 0.0, "channel {c} was left out but ran");
                assert_eq!(stage.s2, 0.0, "channel {c} was left out but ran");
            }
        }
    }

    /// A graphic equalizer with every band flat contributes no stages at all.
    #[test]
    fn an_empty_cascade_passes_audio_through() {
        let mut cascades: Vec<Vec<Biquad>> = vec![Vec::new(), Vec::new()];
        let mut waves: Vec<Vec<CamillaFloat>> = (0..2).map(|c| signal(64, c)).collect();
        let want = waves.clone();
        let ids = straight(2);
        process_cascades(&mut cascades, &mut waves, &ids, &ids);
        assert_same(&waves, &want, "empty cascade");
    }

    #[test]
    fn split_stays_inside_the_budget() {
        for channels in 1..=16 {
            for depth in 0..=40 {
                let (group, stages) = choose_split(channels, depth);
                assert!((1..=MAX_CHANNELS).contains(&group), "{channels} {depth}");
                assert!((1..=MAX_DEPTH).contains(&stages), "{channels} {depth}");
                assert!(group <= channels.max(1), "{channels} {depth}");
                assert!(
                    group * stages <= WIDTH_BUDGET,
                    "{channels} by {depth} gave {group} by {stages}"
                );
            }
        }
    }
}
