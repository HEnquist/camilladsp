use crate::filters::Filter;
use config;
use fftw::array::AlignedVec;
use fftw::plan::*;
use fftw::types::*;
use filters;
//use helpers::{multiply_add_elements, multiply_elements};

// Sample format
use PrcFmt;
#[cfg(feature = "32bit")]
pub type ComplexFmt = c32;
#[cfg(not(feature = "32bit"))]
pub type ComplexFmt = c64;
use Res;

// -- Duplcated from helpers.rs, needed until fftw updates to num-complex 0.3
pub fn multiply_elements(
    result: &mut [ComplexFmt],
    slice_a: &[ComplexFmt],
    slice_b: &[ComplexFmt],
) {
    let len = result.len();
    let mut res = &mut result[..len];
    let mut val_a = &slice_a[..len];
    let mut val_b = &slice_b[..len];

    while res.len() >= 8 {
        res[0] = val_a[0] * val_b[0];
        res[1] = val_a[1] * val_b[1];
        res[2] = val_a[2] * val_b[2];
        res[3] = val_a[3] * val_b[3];
        res[4] = val_a[4] * val_b[4];
        res[5] = val_a[5] * val_b[5];
        res[6] = val_a[6] * val_b[6];
        res[7] = val_a[7] * val_b[7];
        res = &mut res[8..];
        val_a = &val_a[8..];
        val_b = &val_b[8..];
    }
    for (r, val) in res
        .iter_mut()
        .zip(val_a.iter().zip(val_b.iter()).map(|(a, b)| *a * *b))
    {
        *r = val;
    }
}

// element-wise add product, result = result + slice_a * slice_b
pub fn multiply_add_elements(
    result: &mut [ComplexFmt],
    slice_a: &[ComplexFmt],
    slice_b: &[ComplexFmt],
) {
    let len = result.len();
    let mut res = &mut result[..len];
    let mut val_a = &slice_a[..len];
    let mut val_b = &slice_b[..len];

    while res.len() >= 8 {
        res[0] += val_a[0] * val_b[0];
        res[1] += val_a[1] * val_b[1];
        res[2] += val_a[2] * val_b[2];
        res[3] += val_a[3] * val_b[3];
        res[4] += val_a[4] * val_b[4];
        res[5] += val_a[5] * val_b[5];
        res[6] += val_a[6] * val_b[6];
        res[7] += val_a[7] * val_b[7];
        res = &mut res[8..];
        val_a = &val_a[8..];
        val_b = &val_b[8..];
    }
    for (r, val) in res
        .iter_mut()
        .zip(val_a.iter().zip(val_b.iter()).map(|(a, b)| *a * *b))
    {
        *r += val;
    }
}
// -- Duplcated from helpers.rs, needed until fftw updates to num-complex 0.3

pub struct FftConv {
    name: String,
    npoints: usize,
    nsegments: usize,
    overlap: Vec<PrcFmt>,
    coeffs_f: Vec<AlignedVec<ComplexFmt>>,
    #[cfg(feature = "32bit")]
    fft: R2CPlan32,
    #[cfg(not(feature = "32bit"))]
    fft: R2CPlan64,
    #[cfg(feature = "32bit")]
    ifft: C2RPlan32,
    #[cfg(not(feature = "32bit"))]
    ifft: C2RPlan64,
    input_buf: AlignedVec<PrcFmt>,
    input_f: Vec<AlignedVec<ComplexFmt>>,
    temp_buf: AlignedVec<ComplexFmt>,
    output_buf: AlignedVec<PrcFmt>,
    index: usize,
}

impl FftConv {
    /// Create a new FFT colvolution filter.
    pub fn new(name: String, data_length: usize, coeffs: &[PrcFmt]) -> Self {
        let input_buf = AlignedVec::<PrcFmt>::new(2 * data_length);
        let temp_buf = AlignedVec::<ComplexFmt>::new(data_length + 1);
        let output_buf = AlignedVec::<PrcFmt>::new(2 * data_length);
        #[cfg(feature = "32bit")]
        let mut fft: R2CPlan32 = R2CPlan::aligned(&[2 * data_length], Flag::MEASURE).unwrap();
        #[cfg(not(feature = "32bit"))]
        let mut fft: R2CPlan64 = R2CPlan::aligned(&[2 * data_length], Flag::MEASURE).unwrap();
        let ifft = C2RPlan::aligned(&[2 * data_length], Flag::MEASURE).unwrap();

        let nsegments = ((coeffs.len() as PrcFmt) / (data_length as PrcFmt)).ceil() as usize;

        let input_f = vec![AlignedVec::<ComplexFmt>::new(data_length + 1); nsegments];
        let mut coeffs_f = vec![AlignedVec::<ComplexFmt>::new(data_length + 1); nsegments];
        let mut coeffs_al = vec![AlignedVec::<PrcFmt>::new(2 * data_length); nsegments];

        debug!("Conv {} is using {} segments", name, nsegments);

        for (n, coeff) in coeffs.iter().enumerate() {
            coeffs_al[n / data_length][n % data_length] = coeff / (2.0 * data_length as PrcFmt);
        }

        for (segment, segment_f) in coeffs_al.iter_mut().zip(coeffs_f.iter_mut()) {
            fft.r2c(segment, segment_f).unwrap();
        }

        FftConv {
            name,
            npoints: data_length,
            nsegments,
            overlap: vec![0.0; data_length],
            coeffs_f,
            fft,
            ifft,
            input_f,
            input_buf,
            output_buf,
            temp_buf,
            index: 0,
        }
    }

    pub fn from_config(name: String, data_length: usize, conf: config::ConvParameters) -> Self {
        let values = match conf {
            config::ConvParameters::Values { values, length } => {
                filters::pad_vector(&values, length)
            }
            config::ConvParameters::Raw {
                filename,
                format,
                read_bytes_lines,
                skip_bytes_lines,
            } => filters::read_coeff_file(&filename, &format, read_bytes_lines, skip_bytes_lines)
                .unwrap(),
            config::ConvParameters::Wav { filename, channel } => {
                filters::read_wav(&filename, channel).unwrap()
            }
        };
        FftConv::new(name, data_length, &values)
    }
}

impl Filter for FftConv {
    fn name(&self) -> String {
        self.name.clone()
    }

    /// Process a waveform by FT, then multiply transform with transform of filter, and then transform back.
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        // Copy to input buffer
        self.input_buf[0..self.npoints].copy_from_slice(waveform);

        // FFT and store result in history, update index
        self.index = (self.index + 1) % self.nsegments;
        self.fft
            .r2c(&mut self.input_buf, self.input_f[self.index].as_slice_mut())
            .unwrap();

        // Loop through history of input FTs, multiply with filter FTs, accumulate result
        let segm = 0;
        let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
        multiply_elements(
            &mut self.temp_buf,
            &self.input_f[hist_idx],
            &self.coeffs_f[segm],
        );
        for segm in 1..self.nsegments {
            let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
            multiply_add_elements(
                &mut self.temp_buf,
                &self.input_f[hist_idx],
                &self.coeffs_f[segm],
            );
        }

        // IFFT result, store result anv overlap
        self.ifft
            .c2r(&mut self.temp_buf, &mut self.output_buf)
            .unwrap();
        for (n, item) in waveform.iter_mut().enumerate().take(self.npoints) {
            *item = self.output_buf[n] + self.overlap[n];
        }
        self.overlap
            .copy_from_slice(&self.output_buf[self.npoints..]);
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Conv { parameters: conf } = conf {
            let coeffs = match conf {
                config::ConvParameters::Values { values, length } => {
                    filters::pad_vector(&values, length)
                }
                config::ConvParameters::Raw {
                    filename,
                    format,
                    read_bytes_lines,
                    skip_bytes_lines,
                } => {
                    filters::read_coeff_file(&filename, &format, read_bytes_lines, skip_bytes_lines)
                        .unwrap()
                }
                config::ConvParameters::Wav { filename, channel } => {
                    filters::read_wav(&filename, channel).unwrap()
                }
            };

            let nsegments = ((coeffs.len() as PrcFmt) / (self.npoints as PrcFmt)).ceil() as usize;

            if nsegments == self.nsegments {
                // Same length, lets keep history
            } else {
                // length changed, clearing history
                self.nsegments = nsegments;
                let input_f = vec![AlignedVec::<ComplexFmt>::new(self.npoints + 1); nsegments];
                self.input_f = input_f;
            }

            let mut coeffs_f = vec![AlignedVec::<ComplexFmt>::new(self.npoints + 1); nsegments];
            let mut coeffs_al = vec![AlignedVec::<PrcFmt>::new(2 * self.npoints); nsegments];

            debug!("conv using {} segments", nsegments);

            for (n, coeff) in coeffs.iter().enumerate() {
                coeffs_al[n / self.npoints][n % self.npoints] =
                    coeff / (2.0 * self.npoints as PrcFmt);
            }

            for (segment, segment_f) in coeffs_al.iter_mut().zip(coeffs_f.iter_mut()) {
                self.fft.r2c(segment, segment_f).unwrap();
            }
            self.coeffs_f = coeffs_f;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a FFT convolution config.
pub fn validate_config(conf: &config::ConvParameters) -> Res<()> {
    match conf {
        config::ConvParameters::Values { .. } => Ok(()),
        config::ConvParameters::Raw {
            filename,
            format,
            read_bytes_lines,
            skip_bytes_lines,
        } => {
            let coeffs =
                filters::read_coeff_file(&filename, &format, *read_bytes_lines, *skip_bytes_lines)?;
            if coeffs.is_empty() {
                return Err(config::ConfigError::new("Conv coefficients are empty").into());
            }
            Ok(())
        }
        config::ConvParameters::Wav { filename, channel } => {
            let coeffs = filters::read_wav(&filename, *channel)?;
            if coeffs.is_empty() {
                return Err(config::ConfigError::new("Conv coefficients are empty").into());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use config::ConvParameters;
    use fftconv_fftw::FftConv;
    use filters::Filter;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn check_result() {
        let coeffs = vec![0.5, 0.5];
        let conf = ConvParameters::Values {
            values: coeffs,
            length: 0,
        };
        let mut filter = FftConv::from_config("test".to_string(), 8, conf);
        let mut wave1 = vec![1.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
        let expected = vec![0.5, 1.0, 1.0, 0.5, 0.0, -0.5, -0.5, 0.0];
        filter.process_waveform(&mut wave1).unwrap();
        assert!(compare_waveforms(wave1, expected, 1e-7));
    }

    #[test]
    fn check_result_segmented() {
        let mut coeffs = Vec::<PrcFmt>::new();
        for m in 0..32 {
            coeffs.push(m as PrcFmt);
        }
        let mut filter = FftConv::new("test".to_owned(), 8, &coeffs);
        let mut wave1 = vec![0.0 as PrcFmt; 8];
        let mut wave2 = vec![0.0 as PrcFmt; 8];
        let mut wave3 = vec![0.0 as PrcFmt; 8];
        let mut wave4 = vec![0.0 as PrcFmt; 8];
        let mut wave5 = vec![0.0 as PrcFmt; 8];

        wave1[0] = 1.0;
        filter.process_waveform(&mut wave1).unwrap();
        filter.process_waveform(&mut wave2).unwrap();
        filter.process_waveform(&mut wave3).unwrap();
        filter.process_waveform(&mut wave4).unwrap();
        filter.process_waveform(&mut wave5).unwrap();

        let exp1 = Vec::from(&coeffs[0..8]);
        let exp2 = Vec::from(&coeffs[8..16]);
        let exp3 = Vec::from(&coeffs[16..24]);
        let exp4 = Vec::from(&coeffs[24..32]);
        let exp5 = vec![0.0 as PrcFmt; 8];

        assert!(compare_waveforms(wave1, exp1, 1e-5));
        assert!(compare_waveforms(wave2, exp2, 1e-5));
        assert!(compare_waveforms(wave3, exp3, 1e-5));
        assert!(compare_waveforms(wave4, exp4, 1e-5));
        assert!(compare_waveforms(wave5, exp5, 1e-5));
    }
}
