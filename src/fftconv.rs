// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use rustfft::algorithm::Radix4;
use rustfft::FFT;
use rustfft::num_complex::Complex;
use rustfft::num_traits::Zero;




// Sample format
type PrcFmt = f64;


pub struct FFTConv {
    overlap: Vec<PrcFmt>,
    coeffs_f: Vec<Complex<PrcFmt>>,
    fft: Box<rustfft::FFT<PrcFmt>>,
    ifft: Box<rustfft::FFT<PrcFmt>>,
    input: Vec<Complex<PrcFmt>>,
    output: Vec<Complex<PrcFmt>>,
}


impl FFTConv {
    pub fn new(data_length:  usize, coeffs: &Vec<PrcFmt>) -> Self {
        let mut input:  Vec<Complex<PrcFmt>> = vec![Complex::zero(); data_length];
        let mut output: Vec<Complex<PrcFmt>> = vec![Complex::zero(); data_length];
        let mut coeffs_c: Vec<Complex<PrcFmt>> = vec![Complex::zero(); data_length];
        let mut coeffs_f: Vec<Complex<PrcFmt>> = vec![Complex::zero(); data_length];
        let fft = Radix4::new(data_length, false);
        let ifft = Radix4::new(data_length, true);

        for n in 0..coeffs.len() {
            coeffs_c[n] = Complex::from(coeffs[n]);
        }
        fft.process(&mut coeffs_c, &mut coeffs_f);
        //fft.process(&mut input, &mut output);
        FFTConv {
            overlap: vec![0.0; data_length],
            coeffs_f: coeffs_f,
            fft: Box::new(fft),
            ifft: Box::new(ifft),
            input: input,
            output: output,
        }
    }
}


impl Filter for FFTConv {
    fn process_waveform(&mut self, input: Vec<PrcFmt>) -> Vec<PrcFmt> {
        for n in 0..input.len() {
            self.input[n] = Complex::<PrcFmt>::from(input[n]);
        }
        self.fft.process(&mut self.input, &mut self.output);
        for n in 0..self.output.len() {
            self.input[n] = self.output[n]*self.coeffs_f[n];
        }
        self.ifft.process(&mut self.input, &mut self.output);
        let mut filtered: Vec<PrcFmt> = vec![0.0; input.len()];
        for n in 0..self.output.len() {
            filtered[n] = self.output[n].re;
        }
        filtered
    }
}
