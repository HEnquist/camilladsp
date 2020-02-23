use crate::filters::Filter;
use config;
use filters;
use rustfft::algorithm::Radix4;
use rustfft::num_complex::Complex;
use rustfft::num_traits::Zero;
use rustfft::FFT;

// Sample format
use PrcFmt;
use Res;

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
    /// Create a new FFT colvolution filter.
    pub fn new(data_length: usize, coeffs: &[PrcFmt]) -> Self {
        let input_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2 * data_length];
        let temp_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2 * data_length];
        let output_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2 * data_length];
        let mut coeffs_c: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2 * data_length];
        let mut coeffs_f: Vec<Complex<PrcFmt>> = vec![Complex::zero(); 2 * data_length];
        let fft = Radix4::new(2 * data_length, false);
        let ifft = Radix4::new(2 * data_length, true);

        if coeffs.len() > data_length {
            eprintln!(
                "Warning! Filter impulse response is longer than buffer and will be truncated."
            )
        }
        for n in 0..coeffs.len() {
            coeffs_c[n] = Complex::from(coeffs[n] / (2.0 * data_length as PrcFmt));
        }
        fft.process(&mut coeffs_c, &mut coeffs_f);
        FFTConv {
            npoints: data_length,
            overlap: vec![0.0; data_length],
            coeffs_f,
            fft: Box::new(fft),
            ifft: Box::new(ifft),
            input_buf,
            output_buf,
            temp_buf,
        }
    }

    pub fn from_config(data_length: usize, conf: config::ConvParameters) -> Self {
        let values = match conf {
            config::ConvParameters::Values { values } => values,
            config::ConvParameters::File { filename } => {
                filters::read_coeff_file(&filename).unwrap()
            }
        };
        FFTConv::new(data_length, &values)
    }
}

impl Filter for FFTConv {
    /// Process a waveform by FT, then multiply transform with transform of filter, and then transform back.
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        //for n in 0..self.npoints {
        for (n, item) in waveform.iter_mut().enumerate().take(self.npoints) {
            self.input_buf[n] = Complex::<PrcFmt>::from(*item);
            //self.input_buf[n+self.npoints] = Complex::zero();
        }
        self.fft.process(&mut self.input_buf, &mut self.output_buf);
        for n in 0..2 * self.npoints {
            self.temp_buf[n] = self.output_buf[n] * self.coeffs_f[n];
        }
        self.ifft.process(&mut self.temp_buf, &mut self.output_buf);
        //let mut filtered: Vec<PrcFmt> = vec![0.0; self.npoints];
        for (n, item) in waveform.iter_mut().enumerate().take(self.npoints) {
            *item = self.output_buf[n].re + self.overlap[n];
            self.overlap[n] = self.output_buf[n + self.npoints].re;
        }
        Ok(())
    }
}

/// Validate a FFT convolution config.
pub fn validate_config(conf: &config::ConvParameters) -> Res<()> {
    match conf {
        config::ConvParameters::Values { .. } => Ok(()),
        config::ConvParameters::File { filename } => {
            let _ = filters::read_coeff_file(&filename)?;
            Ok(())
        }
    }
}
