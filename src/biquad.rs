// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use config;

// Sample format
//type SmpFmt = i16;
type PrcFmt = f64;

use std::error;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

/// Holder of the biquad coefficients, utilizes normalized form
#[derive(Clone, Copy, Debug)]
pub struct BiquadCoefficients {
    // Denominator coefficients
    pub a1: PrcFmt,
    pub a2: PrcFmt,

    // Nominator coefficients
    pub b0: PrcFmt,
    pub b1: PrcFmt,
    pub b2: PrcFmt,
}

impl BiquadCoefficients {
    pub fn new(a1: PrcFmt, a2: PrcFmt, b0: PrcFmt, b1: PrcFmt, b2: PrcFmt) -> Self {
        BiquadCoefficients {
            a1: a1,
            a2: a2,
            b0: b0,
            b1: b1,
            b2: b2,
        }
    }

    pub fn normalize(a0: PrcFmt, a1: PrcFmt, a2: PrcFmt, b0: PrcFmt, b1: PrcFmt, b2: PrcFmt) -> Self {
        BiquadCoefficients {
            a1: a1 / a0,
            a2: a2 / a0,
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
        }
    }

    pub fn from_vec(coeffs: &Vec<PrcFmt>) -> Self {
        BiquadCoefficients {
            a1: coeffs[0],
            a2: coeffs[1],
            b0: coeffs[2],
            b1: coeffs[3],
            b2: coeffs[4],
        }
    }

    pub fn from_config(fs: usize, parameters: config::BiquadParameters) -> Self {
        match parameters {
            config::BiquadParameters::Free {a1, a2, b0, b1, b2} => {
                BiquadCoefficients::new(a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Highpass { freq, Q} => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * Q);
                let b0 = (1.0 + cs) / 2.0;
                let b1 = -(1.0 + cs);
                let b2 = (1.0 + cs) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowpass { freq, Q } => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let alpha = sn / (2.0 * Q);
                let b0 = (1.0 - cs) / 2.0;
                let b1 = 1.0 - cs;
                let b2 = (1.0 - cs) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cs;
                let a2 = 1.0 - alpha;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Peaking { freq, gain, Q} => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let A = (10.0 as PrcFmt).powf(gain / 40.0);
                let alpha = sn / (2.0 * Q);
                let b0 = 1.0 + (alpha * A);
                let b1 = -2.0 * cs;
                let b2 = 1.0 - (alpha * A);
                let a0 = 1.0 + (alpha / A);
                let a1 = -2.0 * cs;
                let a2 = 1.0 - (alpha / A);
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }

            config::BiquadParameters::Highshelf { freq, slope, gain} => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let A = (10.0 as PrcFmt).powf(gain / 40.0);
                let alpha = sn / 2.0 * ((A + 1.0 / A) * (1.0 / slope - 1.0) + 2.0).sqrt();
                let beta = 2.0 * A.sqrt() * alpha;
                let b0 = A * ((A + 1.0) + (A - 1.0) * cs + beta);
                let b1 = -2.0 * A * ((A - 1.0) + (A + 1.0) * cs);
                let b2 = A * ((A + 1.0) + (A - 1.0) * cs - beta);
                let a0 = (A + 1.0) - (A - 1.0) * cs + beta;
                let a1 = 2.0 * ((A - 1.0) - (A + 1.0) * cs);
                let a2 = (A + 1.0) - (A - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
            config::BiquadParameters::Lowshelf  { freq, slope, gain} => {
                let omega = 2.0 * (std::f64::consts::PI as PrcFmt) * freq / (fs as PrcFmt);
                let sn = omega.sin();
                let cs = omega.cos();
                let A = (10.0 as PrcFmt).powf(gain / 40.0);
                let alpha = sn / 2.0 * ((A + 1.0 / A) * (1.0 / slope - 1.0) + 2.0).sqrt();
                let beta = 2.0 * A.sqrt() * alpha;
                let b0 = A * ((A + 1.0) - (A - 1.0) * cs + beta);
                let b1 = 2.0 * A * ((A - 1.0) - (A + 1.0) * cs);
                let b2 = A * ((A + 1.0) - (A - 1.0) * cs - beta);
                let a0 = (A + 1.0) + (A - 1.0) * cs + beta;
                let a1 = -2.0 * ((A - 1.0) + (A + 1.0) * cs);
                let a2 = (A + 1.0) + (A - 1.0) * cs - beta;
                BiquadCoefficients::normalize(a0, a1, a2, b0, b1, b2)
            }
                
        }
        //let omega = 2 * M_PI * freq / srate;
        //double sn = sin(omega);
        //double cs = cos(omega);
    
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Biquad {
    pub s1: PrcFmt,
    pub s2: PrcFmt,
    coeffs: BiquadCoefficients,
}


impl Biquad {
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn new(coefficients: BiquadCoefficients) -> Self {
        Biquad {
            s1: 0.0,
            s2: 0.0,
            coeffs: coefficients,
        }
    }

    pub fn from_config(fs: usize, parameters: config::BiquadParameters) -> Self {
        Biquad::new(BiquadCoefficients::from_config(fs, parameters))
    }

    fn process_single(&mut self, input: PrcFmt) -> PrcFmt {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }
}


impl Filter for Biquad {
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for n in 0..waveform.len() {
            waveform[n] = self.process_single(waveform[n]);
        }
        //let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<PrcFmt>>();
        Ok(())
    }
}
