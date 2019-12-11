// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;

// Sample format
type SmpFmt = i16;
type PrcFmt = f64;

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
}

/// Internal states and coefficients of the Direct Form 2 Transposed form
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

    fn process_single(&mut self, input: PrcFmt) -> PrcFmt {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }
}


impl Filter for Biquad {
    fn process_waveform(&mut self, input: Vec<PrcFmt>) -> Vec<PrcFmt> {
        let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<PrcFmt>>();
        out
    }
}
