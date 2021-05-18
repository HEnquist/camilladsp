// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use config;

// Sample format
//type SmpFmt = i16;
use NewValue;
use PrcFmt;
use Res;

/// Struct to hold the biquad coefficients
#[derive(Clone, Copy, Debug)]
pub struct BiquadCoefficients {
    pub a1: PrcFmt,
    pub a2: PrcFmt,
    pub b0: PrcFmt,
    pub b1: PrcFmt,
    pub b2: PrcFmt,
}

impl BiquadCoefficients {
    pub fn new(a1: PrcFmt, a2: PrcFmt, b0: PrcFmt, b1: PrcFmt, b2: PrcFmt) -> Self {
        BiquadCoefficients { a1, a2, b0, b1, b2 }
    }

    pub fn normalize(
        a0: PrcFmt,
        a1: PrcFmt,
        a2: PrcFmt,
        b0: PrcFmt,
        b1: PrcFmt,
        b2: PrcFmt,
    ) -> Self {
        let a1n = a1 / a0;
        let a2n = a2 / a0;
        let b0n = b0 / a0;
        let b1n = b1 / a0;
        let b2n = b2 / a0;
        debug!("a1={} a2={} b0={} b1={} b2={}", a1n, a2n, b0n, b1n, b2n);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
                let alpha = sn * ((2.0 as PrcFmt).ln() / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0 + (alpha * ampl);
                let b1 = -2.0 * cs;
                let b2 = 1.0 - (alpha * ampl);
                let a0 = 1.0 + (alpha / ampl);
                let a1 = -2.0 * cs;
                let a2 = 1.0 - (alpha / ampl);
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }

            config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, q, gain }) => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let tn = (omega / 2.0).tan();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
                let b0 = ampl * tn + ampl.powi(2);
                let b1 = ampl * tn - ampl.powi(2);
                let b2 = 0.0;
                let a0 = ampl * tn + 1.0;
                let a1 = ampl * tn - 1.0;
                let a2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, q, gain }) => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let tn = (omega / 2.0).tan();
                let ampl = PrcFmt::new(10.0).powf(gain / 40.0);
                let b0 = ampl.powi(2) * tn + ampl;
                let b1 = ampl.powi(2) * tn - ampl;
                let b2 = 0.0;
                let a0 = tn + ampl;
                let a1 = tn - ampl;
                let a2 = 0.0;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::LowpassFO { freq } => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * ((2.0 as PrcFmt).ln() / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0;
                let b1 = -2.0 * cs;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Bandpass(config::NotchWidth::Q { freq, q }) => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * ((2.0 as PrcFmt).ln() / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Allpass(config::NotchWidth::Q { freq, q }) => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn * ((2.0 as PrcFmt).ln() / 2.0 * bandwidth * omega / sn).sinh();
                let b0 = 1.0 - alpha;
                let b1 = -2.0 * cs;
                let b2 = 1.0 + alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::AllpassFO { freq } => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
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
                let d0i = (2.0 * (std::f64::consts::PI as PrcFmt) * freq_act).powi(2);
                let d1i = (2.0 * (std::f64::consts::PI as PrcFmt) * freq_act) / q_act;
                let c0i = (2.0 * (std::f64::consts::PI as PrcFmt) * freq_target).powi(2);
                let c1i = (2.0 * (std::f64::consts::PI as PrcFmt) * freq_target) / q_target;
                let fc = (freq_target + freq_act) / 2.0;

                let gn = 2.0 * (std::f64::consts::PI as PrcFmt) * fc
                    / ((std::f64::consts::PI as PrcFmt) * fc / (fs as PrcFmt)).tan();
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

#[derive(Clone, Debug)]
pub struct Biquad {
    samplerate: usize,
    pub s1: PrcFmt,
    pub s2: PrcFmt,
    coeffs: BiquadCoefficients,
    pub name: String,
}

impl Biquad {
    /// Creates a Direct Form 2 Transposed biquad filter from a set of coefficients
    pub fn new(name: String, samplerate: usize, coefficients: BiquadCoefficients) -> Self {
        Biquad {
            samplerate,
            s1: 0.0,
            s2: 0.0,
            coeffs: coefficients,
            name,
        }
    }

    /// Process a single sample
    fn process_single(&mut self, input: PrcFmt) -> PrcFmt {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }
}

impl Filter for Biquad {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for item in waveform.iter_mut() {
            *item = self.process_single(*item);
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Biquad { parameters: conf } = conf {
            let coeffs = BiquadCoefficients::from_config(self.samplerate, conf);
            self.coeffs = coeffs;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

pub fn validate_config(samplerate: usize, parameters: &config::BiquadParameters) -> Res<()> {
    let maxfreq = samplerate as PrcFmt / 2.0;
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
        | config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { q, .. }) => {
            if *q <= 0.0 {
                return Err(config::ConfigError::new("Q must be > 0").into());
            }
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
        | config::BiquadParameters::Allpass(config::NotchWidth::Bandwidth { bandwidth, .. }) => {
            if *bandwidth <= 0.0 {
                return Err(config::ConfigError::new("Bandwidth must be > 0").into());
            }
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
    let coeffs = BiquadCoefficients::from_config(samplerate, parameters.clone());
    if !coeffs.is_stable() {
        return Err(config::ConfigError::new("Unstable filter specified").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use biquad::{validate_config, Biquad, BiquadCoefficients};
    use config::{BiquadParameters, NotchWidth, PeakingWidth, ShelfSteepness};
    use filters::Filter;
    use num_complex::Complex;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn is_close_relative(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left / right - 1.0).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    fn gain_and_phase(coeffs: BiquadCoefficients, f: PrcFmt, fs: usize) -> (PrcFmt, PrcFmt) {
        let pi = std::f64::consts::PI as PrcFmt;
        let z = (Complex::i() * 2.0 * pi * f / (fs as PrcFmt)).exp();
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
        let mut filter = Biquad::new("test".to_string(), 44100, coeffs);
        filter.process_waveform(&mut wave).unwrap();
        assert!(compare_waveforms(wave, expected, 1e-3));
    }

    #[test]
    fn make_lowpass() {
        let conf = BiquadParameters::Lowpass {
            freq: 100.0,
            q: 0.707,
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
            q: 0.707,
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
            q: 0.707,
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
            q: 0.707,
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
            q: 1.4142,
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
            q: 1.4142,
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
            q: 1.4142,
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
}
