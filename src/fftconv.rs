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
    npoints: usize,
    overlap: Vec<PrcFmt>,
    coeffs_f: Vec<Complex<PrcFmt>>,
    fft: Box<rustfft::FFT<PrcFmt>>,
    ifft: Box<rustfft::FFT<PrcFmt>>,
    input_buf: Vec<Complex<PrcFmt>>,
    output_buf: Vec<Complex<PrcFmt>>,
}


impl FFTConv {
    pub fn new(data_length:  usize, coeffs: &Vec<PrcFmt>) -> Self {
        let mut input_buf:  Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let mut output_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let mut coeffs_c: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let mut coeffs_f: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let fft = Radix4::new(2*data_length, false);
        let ifft = Radix4::new(2*data_length, true);

        for n in 0..coeffs.len() {
            coeffs_c[n] = Complex::from(coeffs[n]/2048.0);
        }
        fft.process(&mut coeffs_c, &mut coeffs_f);
        //fft.process(&mut input, &mut output);
        FFTConv {
            npoints: data_length,
            overlap: vec![0.0; data_length],
            coeffs_f: coeffs_f,
            fft: Box::new(fft),
            ifft: Box::new(ifft),
            input_buf: input_buf,
            output_buf: output_buf,
        }
    }
}


impl Filter for FFTConv {
    fn process_waveform(&mut self, input: Vec<PrcFmt>) -> Vec<PrcFmt> {
        for n in 0..self.npoints {
            self.input_buf[n] = Complex::<PrcFmt>::from(input[n]);
            self.input_buf[n+self.npoints] = Complex::zero();
        }
        self.fft.process(&mut self.input_buf, &mut self.output_buf);
        for n in 0..2*self.npoints {
            self.input_buf[n] = self.output_buf[n]*self.coeffs_f[n];
        }
        self.ifft.process(&mut self.input_buf, &mut self.output_buf);
        let mut filtered: Vec<PrcFmt> = vec![0.0; self.npoints];
        for n in 0..self.npoints {
            filtered[n] = self.output_buf[n].re + self.overlap[n];
            self.overlap[n] = self.output_buf[n+self.npoints].re;
        }
        filtered
    }
}
