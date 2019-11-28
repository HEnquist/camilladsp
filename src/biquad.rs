// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;

/// Holder of the biquad coefficients, utilizes normalized form
#[derive(Clone, Copy, Debug)]
pub struct Coefficients<T> {
    // Denominator coefficients
    pub a1: T,
    pub a2: T,

    // Nominator coefficients
    pub b0: T,
    pub b1: T,
    pub b2: T,
}

impl Coefficients<f32> {
    pub fn new(a1: f32, a2: f32, b0: f32, b1: f32, b2: f32) -> Self {
        Coefficients {
            a1: a1,
            a2: a2,
            b0: b0,
            b1: b1,
            b2: b2,
        }
    }
}

impl Coefficients<f64> {
    pub fn new(a1: f64, a2: f64, b0: f64, b1: f64, b2: f64) -> Self {
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
pub struct BiquadDF2T<T> {
    pub s1: T,
    pub s2: T,
    coeffs: Coefficients<T>,
}


impl BiquadDF2T<f32> {
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn new(coefficients: Coefficients<f32>) -> Self {
        BiquadDF2T {
            s1: 0.0_f32,
            s2: 0.0_f32,
            coeffs: coefficients,
        }
    }
}

impl BiquadDF2T<f64> {
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn new(coefficients: Coefficients<f64>) -> Self {
        BiquadDF2T {
            s1: 0.0_f64,
            s2: 0.0_f64,
            coeffs: coefficients,
        }
    }
}


impl Filter<f32> for BiquadDF2T<f32> {
    fn process_single(&mut self, input: f32) -> f32 {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }

    fn process_multi(&mut self, input: Vec<f32>) -> Vec<f32> {
        let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<f32>>();
        out
    }
}


impl Filter<f64> for BiquadDF2T<f64> {
    fn process_single(&mut self, input: f64) -> f64 {
        let out = self.s1 + self.coeffs.b0 * input;
        self.s1 = self.s2 + self.coeffs.b1 * input - self.coeffs.a1 * out;
        self.s2 = self.coeffs.b2 * input - self.coeffs.a2 * out;
        out
    }

    fn process_multi(&mut self, input: Vec<f64>) -> Vec<f64> {
        let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<f64>>();
        out
    }
}