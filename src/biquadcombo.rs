// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use biquad;
use config;

// Sample format
//type SmpFmt = i16;
use PrcFmt;
use Res;

#[derive(Clone, Debug)]
pub struct BiquadCombo {
    samplerate: usize,
    pub name: String,
    filters: Vec<biquad::Biquad>,
}

impl BiquadCombo {
    fn butterworth_q(order: usize) -> Vec<PrcFmt> {
        let odd = order % 2 > 0;
        let pi = std::f64::consts::PI as PrcFmt;
        let n_so = order / 2;
        let mut qvalues = Vec::new();
        for n in 0..n_so {
            let q = 1.0 / (2.0 * (pi / (order as PrcFmt) * (n as PrcFmt + 0.5)).sin());
            qvalues.push(q);
        }
        if odd {
            qvalues.push(-1.0);
        }
        qvalues
    }

    fn make_highpass(fs: usize, freq: PrcFmt, qvalues: Vec<PrcFmt>) -> Vec<biquad::Biquad> {
        let mut filters = Vec::new();
        for q in qvalues.iter() {
            let filtconf = if q >= &0.0 {
                config::BiquadParameters::Highpass { freq, q: *q }
            } else {
                config::BiquadParameters::HighpassFO { freq }
            };
            let coeffs = biquad::BiquadCoefficients::from_config(fs, filtconf);
            let filt = biquad::Biquad::new("".to_string(), fs, coeffs);
            filters.push(filt);
        }
        filters
    }

    fn make_lowpass(fs: usize, freq: PrcFmt, qvalues: Vec<PrcFmt>) -> Vec<biquad::Biquad> {
        let mut filters = Vec::new();
        for q in qvalues.iter() {
            let filtconf = if q >= &0.0 {
                config::BiquadParameters::Lowpass { freq, q: *q }
            } else {
                config::BiquadParameters::LowpassFO { freq }
            };
            let coeffs = biquad::BiquadCoefficients::from_config(fs, filtconf);
            let filt = biquad::Biquad::new("".to_string(), fs, coeffs);
            filters.push(filt);
        }
        filters
    }

    fn linkwitzriley_q(order: usize) -> Vec<PrcFmt> {
        let mut q_temp = BiquadCombo::butterworth_q(order / 2);
        let mut qvalues;
        if order % 4 > 0 {
            q_temp.pop();
            qvalues = q_temp.clone();
            qvalues.append(&mut q_temp);
            qvalues.push(0.5);
        } else {
            qvalues = q_temp.clone();
            qvalues.append(&mut q_temp);
        }
        qvalues
    }

    pub fn from_config(
        name: String,
        samplerate: usize,
        parameters: config::BiquadComboParameters,
    ) -> Self {
        match parameters {
            config::BiquadComboParameters::LinkwitzRileyHighpass { order, freq } => {
                let qvalues = BiquadCombo::linkwitzriley_q(order);
                let filters = BiquadCombo::make_highpass(samplerate, freq, qvalues);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
            config::BiquadComboParameters::LinkwitzRileyLowpass { order, freq } => {
                let qvalues = BiquadCombo::linkwitzriley_q(order);
                let filters = BiquadCombo::make_lowpass(samplerate, freq, qvalues);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
            config::BiquadComboParameters::ButterworthHighpass { order, freq } => {
                let qvalues = BiquadCombo::butterworth_q(order);
                let filters = BiquadCombo::make_highpass(samplerate, freq, qvalues);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
            config::BiquadComboParameters::ButterworthLowpass { order, freq } => {
                let qvalues = BiquadCombo::butterworth_q(order);
                let filters = BiquadCombo::make_lowpass(samplerate, freq, qvalues);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
        }
    }
}

impl Filter for BiquadCombo {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for filter in self.filters.iter_mut() {
            filter.process_waveform(waveform)?;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::BiquadCombo { parameters: conf } = conf {
            let name = self.name.clone();
            *self = BiquadCombo::from_config(name, self.samplerate, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a BiquadCombo convolution config.
pub fn validate_config(samplerate: usize, conf: &config::BiquadComboParameters) -> Res<()> {
    let maxfreq = samplerate as PrcFmt / 2.0;
    match conf {
        config::BiquadComboParameters::LinkwitzRileyHighpass { freq, order }
        | config::BiquadComboParameters::LinkwitzRileyLowpass { freq, order } => {
            if *freq <= 0.0 {
                return Err(config::ConfigError::new("Frequency must be > 0").into());
            } else if *freq >= maxfreq {
                return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
            }
            if (*order % 2 > 0) || (*order == 0) {
                return Err(
                    config::ConfigError::new("LR order must be an even non-zero number").into(),
                );
            }
            Ok(())
        }
        config::BiquadComboParameters::ButterworthHighpass { freq, order }
        | config::BiquadComboParameters::ButterworthLowpass { freq, order } => {
            if *freq <= 0.0 {
                return Err(config::ConfigError::new("Frequency must be > 0").into());
            } else if *freq >= maxfreq {
                return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
            }
            if *order == 0 {
                return Err(
                    config::ConfigError::new("Butterworth order must be larger than zero").into(),
                );
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use biquadcombo;
    use config;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_vecs(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }
    #[test]
    fn make_butterworth_2() {
        let q = biquadcombo::BiquadCombo::butterworth_q(2);
        let expect = vec![0.707];
        assert!(q.len() == 1);
        assert!(compare_vecs(q, expect, 0.01));
    }

    #[test]
    fn make_butterworth_5() {
        let q = biquadcombo::BiquadCombo::butterworth_q(5);
        let expect = vec![1.62, 0.62, -1.0];
        assert!(q.len() == 3);
        assert!(compare_vecs(q, expect, 0.01));
    }

    #[test]
    fn make_butterworth_8() {
        let q = biquadcombo::BiquadCombo::butterworth_q(8);
        let expect = vec![2.56, 0.9, 0.6, 0.51];
        assert!(q.len() == 4);
        assert!(compare_vecs(q, expect, 0.01));
    }

    #[test]
    fn make_lr4() {
        let q = biquadcombo::BiquadCombo::linkwitzriley_q(4);
        let expect = vec![0.707, 0.707];
        assert!(q.len() == 2);
        assert!(compare_vecs(q, expect, 0.01));
    }

    #[test]
    fn make_lr6() {
        let q = biquadcombo::BiquadCombo::linkwitzriley_q(10);
        let expect = vec![1.62, 0.62, 1.62, 0.62, 0.5];
        assert!(q.len() == 5);
        assert!(compare_vecs(q, expect, 0.01));
    }

    #[test]
    fn check_lr() {
        let fs = 48000;
        let okconf = config::BiquadComboParameters::LinkwitzRileyHighpass {
            freq: 1000.0,
            order: 6,
        };
        assert!(biquadcombo::validate_config(fs, &okconf).is_ok());
        let badconf1 = config::BiquadComboParameters::LinkwitzRileyHighpass {
            freq: 1000.0,
            order: 5,
        };
        assert!(biquadcombo::validate_config(fs, &badconf1).is_err());
        let badconf2 = config::BiquadComboParameters::LinkwitzRileyHighpass {
            freq: 1000.0,
            order: 0,
        };
        assert!(biquadcombo::validate_config(fs, &badconf2).is_err());
        let badconf3 = config::BiquadComboParameters::LinkwitzRileyHighpass {
            freq: 0.0,
            order: 2,
        };
        assert!(biquadcombo::validate_config(fs, &badconf3).is_err());
        let badconf4 = config::BiquadComboParameters::LinkwitzRileyHighpass {
            freq: 25000.0,
            order: 2,
        };
        assert!(biquadcombo::validate_config(fs, &badconf4).is_err());
    }

    #[test]
    fn check_butterworth() {
        let fs = 48000;
        let okconf1 = config::BiquadComboParameters::ButterworthHighpass {
            freq: 1000.0,
            order: 6,
        };
        assert!(biquadcombo::validate_config(fs, &okconf1).is_ok());
        let okconf2 = config::BiquadComboParameters::ButterworthHighpass {
            freq: 1000.0,
            order: 5,
        };
        assert!(biquadcombo::validate_config(fs, &okconf2).is_ok());
        let badconf = config::BiquadComboParameters::ButterworthHighpass {
            freq: 1000.0,
            order: 0,
        };
        assert!(biquadcombo::validate_config(fs, &badconf).is_err());
        let badconf3 = config::BiquadComboParameters::ButterworthHighpass {
            freq: 0.0,
            order: 2,
        };
        assert!(biquadcombo::validate_config(fs, &badconf3).is_err());
        let badconf4 = config::BiquadComboParameters::ButterworthHighpass {
            freq: 25000.0,
            order: 2,
        };
        assert!(biquadcombo::validate_config(fs, &badconf4).is_err());
    }
}
