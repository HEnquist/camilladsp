use crate::filters::Filter;
use config;
use rand::thread_rng;
use rand_distr::{Distribution, Triangular};

use PrcFmt;
use Res;

#[derive(Clone, Debug)]
pub struct Dither {
    pub name: String,
    pub scalefact: PrcFmt,
    pub amplitude: PrcFmt,
    buffer: Vec<PrcFmt>,
    filter: Vec<PrcFmt>,
    idx: usize,
    filterlen: usize,
}

impl Dither {
    pub fn new(name: String, bits: usize, filter: Vec<PrcFmt>, amplitude: PrcFmt) -> Self {
        let scalefact = (2.0 as PrcFmt).powi((bits - 1) as i32);
        let buffer = vec![0.0; filter.len()];
        let idx = 0;
        let filterlen = filter.len();
        Dither {
            name,
            scalefact,
            amplitude,
            buffer,
            filter,
            idx,
            filterlen,
        }
    }

    pub fn from_config(name: String, conf: config::DitherParameters) -> Self {
        match conf {
            config::DitherParameters::Simple { bits } => {
                let filter = vec![0.8];
                let amplitude = 1.0;
                Dither::new(name, bits, filter, amplitude)
            }
            config::DitherParameters::Uniform { bits, amplitude } => {
                let filter = Vec::new();
                Dither::new(name, bits, filter, amplitude)
            }
            config::DitherParameters::Lipshitz { bits } => {
                let filter = vec![2.033, -2.165, 1.959, -1.590, 0.6149];
                let amplitude = 1.0;
                Dither::new(name, bits, filter, amplitude)
            }
            config::DitherParameters::None { bits } => {
                let filter = Vec::new();
                let amplitude = 0.0;
                Dither::new(name, bits, filter, amplitude)
            }
        }
    }
}

impl Filter for Dither {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        //rand_nbrs = np.random.triangular(-1, 0, 1, len(wave_in))

        if self.filterlen > 0 {
            let rng = thread_rng();
            let dith_rng = Triangular::new(-1.0, 1.0, 0.0).unwrap();
            let dith_iter = dith_rng.sample_iter(rng);
            for (item, dith) in waveform.iter_mut().zip(dith_iter) {
                let scaled = *item * self.scalefact;
                let mut filt_buf = 0.0;
                for (n, coeff) in self.filter.iter().enumerate() {
                    filt_buf += coeff * self.buffer[(n + self.idx) % self.filterlen];
                }
                if self.idx > 0 {
                    self.idx -= 1;
                } else {
                    self.idx = self.filterlen - 1;
                }
                let scaled_plus_err = scaled + filt_buf;
                let result = scaled_plus_err + dith;
                //xe = scaled + (buf0 * fir[0] + buf1 * fir[1] + buf2 * fir[2] + buf3 * fir[3] + buf4 * fir[4])*2.0
                //result = xe + d

                let result_r = result.round();
                self.buffer[self.idx] = scaled_plus_err - result_r;
                *item = result_r / self.scalefact;
            }
        } else if self.amplitude > 0.0 {
            let rng = thread_rng();
            let dith_rng = Triangular::new(-self.amplitude, self.amplitude, 0.0).unwrap();
            let dith_iter = dith_rng.sample_iter(rng);
            for (item, dith) in waveform.iter_mut().zip(dith_iter) {
                let scaled = *item * self.scalefact + dith;
                *item = scaled.round() / self.scalefact;
            }
        } else {
            for item in waveform.iter_mut() {
                let scaled = *item * self.scalefact;
                *item = scaled.round() / self.scalefact;
            }
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Dither { parameters: conf } = conf {
            let name = self.name.clone();
            *self = Dither::from_config(name, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use config::DitherParameters;
    use dither::Dither;
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
    fn test_quantize() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let mut dith = Dither::new("test".to_string(), 8, Vec::new(), 0.0);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 128.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_uniform() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let mut dith = Dither::new("test".to_string(), 8, Vec::new(), 1.0);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 64.0
        ));
        assert!(!compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 256.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_simple() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Simple { bits: 8 };
        let mut dith = Dither::from_config("test".to_string(), conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 32.0
        ));
        assert!(!compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 256.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }

    #[test]
    fn test_lip() {
        let mut waveform = vec![-1.0, -0.5, -1.0 / 3.0, 0.0, 1.0 / 3.0, 0.5, 1.0];
        let waveform2 = waveform.clone();
        let conf = DitherParameters::Lipshitz { bits: 8 };
        let mut dith = Dither::from_config("test".to_string(), conf);
        dith.process_waveform(&mut waveform).unwrap();
        assert!(compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 16.0
        ));
        assert!(!compare_waveforms(
            waveform.clone(),
            waveform2.clone(),
            1.0 / 256.0
        ));
        assert!(is_close(
            (128.0 * waveform[2]).round(),
            128.0 * waveform[2],
            1e-9
        ));
    }
}
