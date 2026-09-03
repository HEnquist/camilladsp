// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

use crate::config;
use crate::filters::Filter;
use crate::filters::biquad;

// Sample format
//type SmpFmt = i16;
use crate::CamillaFloat;
use crate::Res;

#[derive(Clone, Debug)]
pub struct BiquadCombo {
    samplerate: usize,
    pub name: String,
    filters: Vec<biquad::Biquad>,
}

impl BiquadCombo {
    fn butterworth_q(order: usize) -> Vec<f64> {
        let odd = !order.is_multiple_of(2);
        let pi = std::f64::consts::PI;
        let n_so = order / 2;
        let mut qvalues = Vec::with_capacity(n_so + usize::from(odd));
        for n in 0..n_so {
            let q = 1.0 / (2.0 * (pi / (order as f64) * (n as f64 + 0.5)).sin());
            qvalues.push(q);
        }
        if odd {
            qvalues.push(-1.0);
        }
        qvalues
    }

    fn make_highpass(fs: usize, freq: f64, qvalues: Vec<f64>) -> Vec<biquad::Biquad> {
        let mut filters = Vec::with_capacity(qvalues.len());
        for q in qvalues.iter() {
            let filtconf = if q >= &0.0 {
                config::BiquadParameters::Highpass { freq, q: *q }
            } else {
                config::BiquadParameters::HighpassFO { freq }
            };
            let coeffs = biquad::BiquadCoefficients::from_config(fs, filtconf);
            let filt = biquad::Biquad::new("", fs, coeffs);
            filters.push(filt);
        }
        filters
    }

    fn make_lowpass(fs: usize, freq: f64, qvalues: Vec<f64>) -> Vec<biquad::Biquad> {
        let mut filters = Vec::with_capacity(qvalues.len());
        for q in qvalues.iter() {
            let filtconf = if q >= &0.0 {
                config::BiquadParameters::Lowpass { freq, q: *q }
            } else {
                config::BiquadParameters::LowpassFO { freq }
            };
            let coeffs = biquad::BiquadCoefficients::from_config(fs, filtconf);
            let filt = biquad::Biquad::new("", fs, coeffs);
            filters.push(filt);
        }
        filters
    }

    fn linkwitzriley_q(order: usize) -> Vec<f64> {
        let mut q_temp = BiquadCombo::butterworth_q(order / 2);
        let mut qvalues;
        if !order.is_multiple_of(4) {
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

    fn make_tilt(fs: usize, gain: f64) -> Vec<biquad::Biquad> {
        let gain_low = -gain / 2.0;
        let gain_high = gain / 2.0;
        let lsconf = config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q {
            freq: 110.0,
            q: 0.35,
            gain: gain_low,
        });
        let hsconf = config::BiquadParameters::Highshelf(config::ShelfSteepness::Q {
            freq: 3500.0,
            q: 0.35,
            gain: gain_high,
        });
        let mut filters = Vec::with_capacity(2);
        let lscoeffs = biquad::BiquadCoefficients::from_config(fs, lsconf);
        let lsfilt = biquad::Biquad::new("", fs, lscoeffs);
        filters.push(lsfilt);
        let hscoeffs = biquad::BiquadCoefficients::from_config(fs, hsconf);
        let hsfilt = biquad::Biquad::new("", fs, hscoeffs);
        filters.push(hsfilt);
        filters
    }

    fn make_npeq(samplerate: usize, bands: &[config::PeqBand]) -> Vec<biquad::Biquad> {
        let sections = npeq_sections(bands);
        let mut filters = Vec::with_capacity(sections.len());
        for (params, band) in sections.into_iter().zip(bands.iter()) {
            // A band with no significant gain does nothing, so leave it out.
            if band.gain.abs() <= 0.001 {
                continue;
            }
            let coeffs = biquad::BiquadCoefficients::from_config(samplerate, params);
            filters.push(biquad::Biquad::new("", samplerate, coeffs));
        }
        filters
    }

    fn make_graphic(
        samplerate: usize,
        freq_min: f32,
        freq_max: f32,
        gains: &[f32],
    ) -> Vec<biquad::Biquad> {
        let bands = gains.len();
        let mut filters = Vec::with_capacity(bands);

        let f_min_log = freq_min.log2();
        let f_max_log = freq_max.log2();
        let bw = (f_max_log - f_min_log) / bands as f32;
        for (band, gain) in gains.iter().enumerate() {
            if gain.abs() > 0.001 {
                let freq_log = f_min_log + (band as f32 + 0.5) * bw;
                let freq = 2.0_f32.powf(freq_log);
                let filtconf = config::BiquadParameters::Peaking(config::PeakingWidth::Bandwidth {
                    freq: freq as f64,
                    bandwidth: bw as f64,
                    gain: *gain as f64,
                });
                let coeffs = biquad::BiquadCoefficients::from_config(samplerate, filtconf);
                let filt = biquad::Biquad::new("", samplerate, coeffs);
                filters.push(filt);
            }
        }
        filters
    }

    pub fn from_config(
        name: &str,
        samplerate: usize,
        parameters: config::BiquadComboParameters,
    ) -> Self {
        let name = name.to_string();
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
            config::BiquadComboParameters::Tilt { gain } => {
                let filters = BiquadCombo::make_tilt(samplerate, gain);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
            config::BiquadComboParameters::NPointPeq { bands } => {
                let filters = BiquadCombo::make_npeq(samplerate, &bands);
                BiquadCombo {
                    samplerate,
                    name,
                    filters,
                }
            }
            config::BiquadComboParameters::GraphicEqualizer(params) => {
                let filters = BiquadCombo::make_graphic(
                    samplerate,
                    params.freq_min(),
                    params.freq_max(),
                    &params.gains,
                );
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
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        for filter in self.filters.iter_mut() {
            filter.process_waveform(waveform);
        }
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::BiquadCombo {
            parameters: conf, ..
        } = conf
        {
            let name = self.name.clone();
            *self = BiquadCombo::from_config(&name, self.samplerate, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Expand the bands of an NPointPeq into biquad parameters.
/// The first band becomes a low shelf and the last a high shelf,
/// with the ones in between as peaking filters.
fn npeq_sections(bands: &[config::PeqBand]) -> Vec<config::BiquadParameters> {
    let last = bands.len().saturating_sub(1);
    bands
        .iter()
        .enumerate()
        .map(|(n, band)| {
            let config::PeqBand { freq, q, gain } = *band;
            if n == 0 {
                config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, q, gain })
            } else if n == last {
                config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, q, gain })
            } else {
                config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq, q, gain })
            }
        })
        .collect()
}

/// Validate a BiquadCombo convolution config.
pub fn validate_config(samplerate: usize, conf: &config::BiquadComboParameters) -> Res<()> {
    let maxfreq = samplerate as f64 / 2.0;
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
        config::BiquadComboParameters::Tilt { gain } => {
            if *gain <= -100.0 {
                return Err(config::ConfigError::new("Gain must be > -100").into());
            } else if *gain >= 100.0 {
                return Err(config::ConfigError::new("Gain must be < 100").into());
            }
            Ok(())
        }
        config::BiquadComboParameters::NPointPeq { bands } => {
            if bands.len() < 2 {
                return Err(config::ConfigError::new(
                    "At least two bands are needed, for the low and high shelves",
                )
                .into());
            }
            for params in npeq_sections(bands).iter() {
                biquad::validate_config(samplerate, params)?;
            }
            // The first band becomes the low shelf and the last the high shelf,
            // so the bands have to be listed with rising frequency.
            for pair in bands.windows(2) {
                if pair[1].freq < pair[0].freq {
                    return Err(config::ConfigError::new(
                        "Band frequencies must not decrease along the list",
                    )
                    .into());
                }
            }
            Ok(())
        }
        config::BiquadComboParameters::GraphicEqualizer(params) => {
            if params.freq_min() <= 0.0 || params.freq_max() <= 0.0 {
                return Err(config::ConfigError::new("Min and max requencies must be > 0").into());
            } else if params.freq_min() >= maxfreq as f32 || params.freq_max() >= maxfreq as f32 {
                return Err(config::ConfigError::new(
                    "Min and max frequencies must be < samplerate/2",
                )
                .into());
            }
            if params.freq_min() >= params.freq_max() {
                return Err(config::ConfigError::new(
                    "Min frequency must be lower than max frequency",
                )
                .into());
            }
            for gain in params.gains.iter() {
                if *gain > 40.0 || *gain < -40.0 {
                    return Err(config::ConfigError::new(
                        "Equalizer gains must be withing +- 40 dB",
                    )
                    .into());
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config;
    use crate::filters::Filter;
    use crate::filters::biquad;
    use crate::filters::biquadcombo;

    fn is_close(left: f64, right: f64, maxdiff: f64) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn compare_vecs(left: Vec<f64>, right: Vec<f64>, maxdiff: f64) -> bool {
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

    fn band(freq: f64, gain: f64) -> config::PeqBand {
        config::PeqBand { freq, q: 0.7, gain }
    }

    fn npeq(bands: Vec<config::PeqBand>) -> config::BiquadComboParameters {
        config::BiquadComboParameters::NPointPeq { bands }
    }

    #[test]
    fn npeq_band_roles() {
        // First band is a low shelf, last is a high shelf, the rest are peaking.
        let bands = vec![
            band(100.0, 1.0),
            band(400.0, 1.0),
            band(1000.0, 1.0),
            band(8000.0, 1.0),
        ];
        let sections = biquadcombo::npeq_sections(&bands);
        assert!(matches!(
            sections[0],
            config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq: 100.0, .. })
        ));
        assert!(matches!(
            sections[1],
            config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq: 400.0, .. })
        ));
        assert!(matches!(
            sections[2],
            config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq: 1000.0, .. })
        ));
        assert!(matches!(
            sections[3],
            config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq: 8000.0, .. })
        ));
    }

    #[test]
    fn npeq_two_bands_are_both_shelves() {
        let sections = biquadcombo::npeq_sections(&[band(100.0, 1.0), band(8000.0, 1.0)]);
        assert_eq!(sections.len(), 2);
        assert!(matches!(sections[0], config::BiquadParameters::Lowshelf(_)));
        assert!(matches!(
            sections[1],
            config::BiquadParameters::Highshelf(_)
        ));
    }

    #[test]
    fn npeq_section_count() {
        let conf = npeq(vec![
            band(100.0, 1.0),
            band(400.0, -0.5),
            band(1000.0, 1.5),
            band(2500.0, -0.25),
            band(8000.0, 0.5),
        ]);
        let combo = biquadcombo::BiquadCombo::from_config("test", 44100, conf);
        assert_eq!(combo.filters.len(), 5);
    }

    #[test]
    fn npeq_skips_zero_gain_bands() {
        let conf = npeq(vec![
            band(100.0, 0.0),
            band(400.0, -0.5),
            // Below the 0.001 dB threshold, so this one is dropped as well.
            band(1000.0, 0.0005),
            band(8000.0, 0.0),
        ]);
        let combo = biquadcombo::BiquadCombo::from_config("test", 44100, conf);
        assert_eq!(combo.filters.len(), 1);
    }

    #[test]
    fn npeq_zero_gain_band_keeps_the_roles_of_the_others() {
        // Dropping a zero gain band must not promote its neighbour to a shelf.
        let bands = vec![band(100.0, 1.0), band(1000.0, 1.0), band(8000.0, 0.0)];
        let with_zero = biquadcombo::BiquadCombo::from_config("a", 44100, npeq(bands));
        // The same two active bands, but now the peak really is the last band.
        let as_listed = biquadcombo::BiquadCombo::from_config(
            "b",
            44100,
            npeq(vec![band(100.0, 1.0), band(1000.0, 1.0)]),
        );
        assert_eq!(with_zero.filters.len(), 2);
        assert_eq!(as_listed.filters.len(), 2);
        let mut with_zero = with_zero;
        let mut as_listed = as_listed;
        let mut wave_a = vec![0.0; 256];
        wave_a[0] = 1.0;
        let mut wave_b = wave_a.clone();
        with_zero.process_waveform(&mut wave_a);
        as_listed.process_waveform(&mut wave_b);
        // In the first the 1 kHz band is peaking, in the second it is a high shelf.
        assert_ne!(wave_a, wave_b);
    }

    #[test]
    fn npeq_all_zero_gain_is_passthrough() {
        let conf = npeq(vec![
            band(100.0, 0.0),
            band(1000.0, -0.0),
            band(8000.0, 0.0),
        ]);
        let mut combo = biquadcombo::BiquadCombo::from_config("test", 44100, conf);
        assert!(combo.filters.is_empty());
        let mut wave = vec![1.0, 0.5, -0.25, 0.0];
        let expected = wave.clone();
        combo.process_waveform(&mut wave);
        assert_eq!(wave, expected);
    }

    #[test]
    fn npeq_matches_separate_biquads() {
        let fs = 44100;
        let conf = npeq(vec![
            band(125.0, 1.0),
            band(400.0, -0.5),
            band(1000.0, 1.5),
            band(2500.0, -0.25),
            band(8000.0, 0.5),
        ]);
        let mut combo = biquadcombo::BiquadCombo::from_config("test", fs, conf);
        // The same equalizer, spelled out as separate biquads in the same order.
        let separate = [
            config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q {
                freq: 125.0,
                q: 0.7,
                gain: 1.0,
            }),
            config::BiquadParameters::Peaking(config::PeakingWidth::Q {
                freq: 400.0,
                q: 0.7,
                gain: -0.5,
            }),
            config::BiquadParameters::Peaking(config::PeakingWidth::Q {
                freq: 1000.0,
                q: 0.7,
                gain: 1.5,
            }),
            config::BiquadParameters::Peaking(config::PeakingWidth::Q {
                freq: 2500.0,
                q: 0.7,
                gain: -0.25,
            }),
            config::BiquadParameters::Highshelf(config::ShelfSteepness::Q {
                freq: 8000.0,
                q: 0.7,
                gain: 0.5,
            }),
        ];
        // An impulse, so the two are compared over their full responses.
        let mut wave_combo = vec![0.0; 1024];
        wave_combo[0] = 1.0;
        let mut wave_separate = wave_combo.clone();
        combo.process_waveform(&mut wave_combo);
        for params in separate {
            let coeffs = biquad::BiquadCoefficients::from_config(fs, params);
            let mut filt = biquad::Biquad::new("", fs, coeffs);
            filt.process_waveform(&mut wave_separate);
        }
        assert_eq!(wave_combo, wave_separate);
    }

    #[test]
    fn check_npeq_band_count() {
        let fs = 44100;
        assert!(biquadcombo::validate_config(fs, &npeq(vec![])).is_err());
        assert!(biquadcombo::validate_config(fs, &npeq(vec![band(1000.0, 1.0)])).is_err());
        let two = npeq(vec![band(100.0, 1.0), band(8000.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &two).is_ok());
    }

    #[test]
    fn check_npeq_frequency_order() {
        let fs = 44100;
        let rising = npeq(vec![
            band(100.0, 1.0),
            band(400.0, 1.0),
            band(1000.0, 1.0),
            band(8000.0, 1.0),
        ]);
        assert!(biquadcombo::validate_config(fs, &rising).is_ok());
        // Equal frequencies are allowed, the list only has to not decrease.
        let equal = npeq(vec![band(400.0, 1.0), band(400.0, 1.0), band(400.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &equal).is_ok());
        let falling = npeq(vec![band(100.0, 1.0), band(2000.0, 1.0), band(500.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &falling).is_err());
        let shelves_swapped = npeq(vec![band(8000.0, 1.0), band(100.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &shelves_swapped).is_err());
    }

    #[test]
    fn check_npeq_bands() {
        let fs = 44100;
        // Bands are checked even when their gain is zero and they get skipped.
        let bad_freq = npeq(vec![band(-5.0, 0.0), band(8000.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &bad_freq).is_err());
        let bad_q = npeq(vec![
            band(100.0, 1.0),
            config::PeqBand {
                freq: 1000.0,
                q: 0.0,
                gain: 1.0,
            },
            band(8000.0, 1.0),
        ]);
        assert!(biquadcombo::validate_config(fs, &bad_q).is_err());
        let above_nyquist = npeq(vec![band(100.0, 1.0), band(30000.0, 1.0)]);
        assert!(biquadcombo::validate_config(fs, &above_nyquist).is_err());
    }
}
