// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use rustfft::algorithm::Radix4;
use rustfft::FFT;
use rustfft::num_complex::Complex;
use rustfft::num_traits::Zero;
use std::error;
use config;
use filters;



// Sample format
type PrcFmt = f64;

pub type Res<T> = Result<T, Box<dyn error::Error>>;

pub struct FFTConv {
    npoints: usize,
    overlap: Vec<PrcFmt>,
    coeffs_f: Vec<Complex<PrcFmt>>,
    fft: Box<dyn rustfft::FFT<PrcFmt>>,
    ifft: Box<dyn rustfft::FFT<PrcFmt>>,
    input_buf: Vec<Complex<PrcFmt>>,
    temp_buf: Vec<Complex<PrcFmt>>,
    output_buf: Vec<Complex<PrcFmt>>,
}


impl FFTConv {
    pub fn new(data_length:  usize, coeffs: &Vec<PrcFmt>) -> Self {
        let input_buf:  Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let temp_buf:  Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let output_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let mut coeffs_c: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let mut coeffs_f: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2*data_length];
        let fft = Radix4::new(2*data_length, false);
        let ifft = Radix4::new(2*data_length, true);

        for n in 0..coeffs.len() {
            coeffs_c[n] = Complex::from(coeffs[n]/(2 as PrcFmt * data_length as PrcFmt));
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
            temp_buf: temp_buf,
        }
    }

    pub fn from_config(data_length: usize, conf: config::ConvParameters) -> Self {
        let values = match conf {
            config::ConvParameters::Values{values} => values.clone(),
            config::ConvParameters::File{filename} => filters::read_coeff_file(&filename).unwrap(),
        };
        FFTConv::new(data_length, &values)
    }

    pub fn validate_config(conf: config::ConvParameters) -> Res<()> {
        match conf {
            config::ConvParameters::Values{values: _} => Ok(()),
            config::ConvParameters::File{filename} => {
                let _ = filters::read_coeff_file(&filename)?;
                Ok(())
            }
        }
    }
}


impl Filter for FFTConv {
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for n in 0..self.npoints {
            self.input_buf[n] = Complex::<PrcFmt>::from(waveform[n]);
            //self.input_buf[n+self.npoints] = Complex::zero();
        }
        self.fft.process(&mut self.input_buf, &mut self.output_buf);
        for n in 0..2*self.npoints {
            self.temp_buf[n] = self.output_buf[n]*self.coeffs_f[n];
        }
        self.ifft.process(&mut self.temp_buf, &mut self.output_buf);
        //let mut filtered: Vec<PrcFmt> = vec![0.0; self.npoints];
        for n in 0..self.npoints {
            waveform[n] = self.output_buf[n].re + self.overlap[n];
            self.overlap[n] = self.output_buf[n+self.npoints].re;
        }
        Ok(())
    }
}
