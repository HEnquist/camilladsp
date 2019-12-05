// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;

// Sample format
type SampleFormat = i16;
type ProcessingFormat = f64;

/// Holder of the biquad coefficients, utilizes normalized form
#[derive(Clone, Copy, Debug)]
pub struct Coefficients {
    // Denominator coefficients
    pub a1: ProcessingFormat,
    pub a2: ProcessingFormat,

    // Nominator coefficients
    pub b0: ProcessingFormat,
    pub b1: ProcessingFormat,
    pub b2: ProcessingFormat,
}

impl Coefficients {
    pub fn new(a1: ProcessingFormat, a2: ProcessingFormat, b0: ProcessingFormat, b1: ProcessingFormat, b2: ProcessingFormat) -> Self {
        Coefficients {
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
pub struct BiquadDF2T {
    pub s1: ProcessingFormat,
    pub s2: ProcessingFormat,
    coeffs: Coefficients,
}


impl BiquadDF2T {
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn new(coefficients: Coefficients) -> Self {
        BiquadDF2T {
            s1: 0.0,
            s2: 0.0,
            coeffs: coefficients,
        }
    }
}


impl Filter for BiquadDF2T {
    fn process_single(&mut self, input: ProcessingFormat) -> ProcessingFormat {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }

    fn process_multi(&mut self, input: Vec<ProcessingFormat>) -> Vec<ProcessingFormat> {
        let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<ProcessingFormat>>();
        out
    }
}
