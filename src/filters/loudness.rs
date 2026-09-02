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
use crate::filters::basicfilters::Gain;
use crate::filters::biquad;
use std::sync::Arc;

use crate::CamillaFloat;
use crate::ProcessingParameters;
use crate::Res;
use crate::ToF32;

pub struct Loudness {
    pub name: String,
    current_volume: CamillaFloat,
    processing_params: Arc<ProcessingParameters>,
    reference_level: f32,
    high_boost: f32,
    low_boost: f32,
    high_freq: f64,
    low_freq: f64,
    high_q: f64,
    low_q: f64,
    high_biquad: biquad::Biquad,
    low_biquad: biquad::Biquad,
    fader: usize,
    active: bool,
    gain: Option<Gain>,
}

/// Below this the shelf spreads out so far that it no longer reaches its
/// nominal boost within the audio band.
const MIN_Q: f64 = 0.1;
/// Above this the shelf overshoots badly at the corner frequency. At the
/// largest allowed boost of 20 dB, a Q of 2.0 already peaks 5.6 dB above the
/// shelf level.
const MAX_Q: f64 = 2.0;

fn rel_boost(level: f32, reference: f32) -> f32 {
    let rel_boost = (reference - level) / 20.0;
    rel_boost.clamp(0.0, 1.0)
}

fn highshelf_conf(freq: f64, q: f64, gain: f64) -> config::BiquadParameters {
    config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, q, gain })
}

fn lowshelf_conf(freq: f64, q: f64, gain: f64) -> config::BiquadParameters {
    config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, q, gain })
}

impl Loudness {
    pub fn from_config(
        name: &str,
        conf: config::LoudnessParameters,
        samplerate: usize,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        info!("Create loudness filter");
        let fader = conf.fader();
        let current_volume = processing_params.target_volume(fader);
        let relboost = rel_boost(current_volume, conf.reference_level);
        let active = relboost > 0.01;
        let high_boost = (relboost * conf.high_boost()) as f64;
        let low_boost = (relboost * conf.low_boost()) as f64;
        let highshelf_conf = highshelf_conf(conf.high_freq(), conf.high_q(), high_boost);
        let lowshelf_conf = lowshelf_conf(conf.low_freq(), conf.low_q(), low_boost);
        let gain = if conf.attenuate_mid() {
            let max_gain = low_boost.max(high_boost);
            let gain_params = config::GainParameters {
                gain: -max_gain,
                inverted: None,
                mute: None,
                scale: None,
            };
            Some(Gain::from_config("midgain", gain_params))
        } else {
            None
        };

        let high_biquad_coeffs =
            biquad::BiquadCoefficients::from_config(samplerate, highshelf_conf);
        let low_biquad_coeffs = biquad::BiquadCoefficients::from_config(samplerate, lowshelf_conf);
        let high_biquad = biquad::Biquad::new("highshelf", samplerate, high_biquad_coeffs);
        let low_biquad = biquad::Biquad::new("lowshelf", samplerate, low_biquad_coeffs);
        Loudness {
            name: name.to_string(),
            current_volume: current_volume as CamillaFloat,
            reference_level: conf.reference_level,
            high_boost: conf.high_boost(),
            low_boost: conf.low_boost(),
            high_freq: conf.high_freq(),
            low_freq: conf.low_freq(),
            high_q: conf.high_q(),
            low_q: conf.low_q(),
            high_biquad,
            low_biquad,
            processing_params,
            fader,
            active,
            gain,
        }
    }
}

impl Filter for Loudness {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        // Written by `Volume` while it processes, so the two are
        // order-sensitive across channels. See `parallelize_filters` in
        // pipeline.rs, which changes that order and says why the one chunk of
        // lag that can result is accepted.
        let shared_vol = self.processing_params.current_volume(self.fader);

        // Volume setting changed
        if (shared_vol - self.current_volume.to_f32()).abs() > 0.01 {
            self.current_volume = shared_vol as CamillaFloat;
            let relboost = rel_boost(self.current_volume.to_f32(), self.reference_level);
            let high_boost = (relboost * self.high_boost) as f64;
            let low_boost = (relboost * self.low_boost) as f64;
            self.active = relboost > 0.001;
            debug!(
                "Updating loudness biquads, relative boost {}%",
                100.0 * relboost
            );
            let highshelf_conf = highshelf_conf(self.high_freq, self.high_q, high_boost);
            let lowshelf_conf = lowshelf_conf(self.low_freq, self.low_q, low_boost);
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
            if let Some(gain) = &mut self.gain {
                let max_gain = low_boost.max(high_boost);
                let gain_params = config::GainParameters {
                    gain: -max_gain,
                    inverted: None,
                    mute: None,
                    scale: None,
                };
                gain.update_parameters(config::Filter::Gain {
                    description: None,
                    parameters: gain_params,
                });
            }
        }
        if self.active {
            trace!("Applying loudness biquads");
            self.high_biquad.process_waveform(waveform);
            self.low_biquad.process_waveform(waveform);
            if let Some(gain) = &mut self.gain {
                gain.process_waveform(waveform);
            }
        }
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Loudness {
            parameters: conf, ..
        } = conf
        {
            self.fader = conf.fader();
            let current_volume = self.processing_params.current_volume(self.fader);
            let relboost = rel_boost(current_volume, conf.reference_level);
            let high_boost = (relboost * conf.high_boost()) as f64;
            let low_boost = (relboost * conf.low_boost()) as f64;
            self.active = relboost > 0.001;
            let highshelf_conf = highshelf_conf(conf.high_freq(), conf.high_q(), high_boost);
            let lowshelf_conf = lowshelf_conf(conf.low_freq(), conf.low_q(), low_boost);
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
            if conf.attenuate_mid() {
                let max_gain = low_boost.max(high_boost);
                let gain_params = config::GainParameters {
                    gain: -max_gain,
                    inverted: None,
                    mute: None,
                    scale: None,
                };
                if let Some(gain) = &mut self.gain {
                    gain.update_parameters(config::Filter::Gain {
                        description: None,
                        parameters: gain_params,
                    });
                } else {
                    self.gain = Some(Gain::from_config("midgain", gain_params))
                }
            } else {
                self.gain = None
            }

            self.reference_level = conf.reference_level;
            self.high_boost = conf.high_boost();
            self.low_boost = conf.low_boost();
            self.high_freq = conf.high_freq();
            self.low_freq = conf.low_freq();
            self.high_q = conf.high_q();
            self.low_q = conf.low_q();
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a Loudness config.
pub fn validate_config(samplerate: usize, conf: &config::LoudnessParameters) -> Res<()> {
    if conf.reference_level > 20.0 {
        return Err(config::ConfigError::new("Reference level must be less than 20").into());
    } else if conf.reference_level < -100.0 {
        return Err(config::ConfigError::new("Reference level must be higher than -100").into());
    } else if conf.high_boost() < 0.0 {
        return Err(config::ConfigError::new("High boost cannot be less than 0").into());
    } else if conf.low_boost() < 0.0 {
        return Err(config::ConfigError::new("Low boost cannot be less than 0").into());
    } else if conf.high_boost() > 20.0 {
        return Err(config::ConfigError::new("High boost cannot be larger than 20").into());
    } else if conf.low_boost() > 20.0 {
        return Err(config::ConfigError::new("Low boost cannot be larger than 20").into());
    } else if conf.low_freq() <= 0.0 {
        return Err(config::ConfigError::new("Low freq must be > 0").into());
    } else if conf.high_freq() >= samplerate as f64 / 2.0 {
        return Err(config::ConfigError::new("High freq must be < samplerate/2").into());
    } else if conf.high_freq() <= conf.low_freq() {
        return Err(config::ConfigError::new("High freq must be higher than low freq").into());
    } else if !(MIN_Q..=MAX_Q).contains(&conf.high_q()) {
        return Err(config::ConfigError::new(&format!(
            "High Q must be between {MIN_Q:.1} and {MAX_Q:.1}"
        ))
        .into());
    } else if !(MIN_Q..=MAX_Q).contains(&conf.low_q()) {
        return Err(config::ConfigError::new(&format!(
            "Low Q must be between {MIN_Q:.1} and {MAX_Q:.1}"
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::{BiquadParameters, LoudnessParameters, ShelfSteepness};
    use crate::filters::biquad::BiquadCoefficients;
    use crate::filters::loudness::validate_config;

    fn params() -> LoudnessParameters {
        LoudnessParameters {
            reference_level: -25.0,
            high_boost: None,
            low_boost: None,
            high_freq: None,
            low_freq: None,
            high_q: None,
            low_q: None,
            fader: None,
            attenuate_mid: None,
        }
    }

    fn is_close(left: f64, right: f64) -> bool {
        println!("{left} - {right}");
        (left - right).abs() <= 1e-12 * right.abs().max(1.0)
    }

    /// The shelves used to be specified with a fixed slope of 12 dB/octave.
    /// The default Q must give the same coefficients at any boost, so that
    /// existing configs keep sounding the same. The two formulas reach the
    /// same value by different arithmetic, so they can differ in the last bit.
    #[test]
    fn default_q_matches_the_old_slope() {
        let conf = params();
        for gain in [0.0, 1.0, 5.0, 10.0, 20.0] {
            let from_q = BiquadCoefficients::from_config(
                44100,
                BiquadParameters::Highshelf(ShelfSteepness::Q {
                    freq: conf.high_freq(),
                    q: conf.high_q(),
                    gain,
                }),
            );
            let from_slope = BiquadCoefficients::from_config(
                44100,
                BiquadParameters::Highshelf(ShelfSteepness::Slope {
                    freq: 3500.0,
                    slope: 12.0,
                    gain,
                }),
            );
            assert!(is_close(from_q.a1, from_slope.a1));
            assert!(is_close(from_q.a2, from_slope.a2));
            assert!(is_close(from_q.b0, from_slope.b0));
            assert!(is_close(from_q.b1, from_slope.b1));
            assert!(is_close(from_q.b2, from_slope.b2));
        }
    }

    #[test]
    fn defaults_are_valid() {
        assert!(validate_config(44100, &params()).is_ok());
    }

    #[test]
    fn shelves_must_not_cross() {
        let mut conf = params();
        conf.low_freq = Some(4000.0);
        assert!(validate_config(44100, &conf).is_err());
        conf.high_freq = Some(8000.0);
        assert!(validate_config(44100, &conf).is_ok());
    }

    #[test]
    fn freq_must_be_below_nyquist() {
        let mut conf = params();
        conf.high_freq = Some(9000.0);
        assert!(validate_config(44100, &conf).is_ok());
        assert!(validate_config(16000, &conf).is_err());
    }

    #[test]
    fn freq_must_be_positive() {
        let mut conf = params();
        conf.low_freq = Some(0.0);
        assert!(validate_config(44100, &conf).is_err());
    }

    #[test]
    fn q_must_be_within_limits() {
        let mut conf = params();
        conf.high_q = Some(0.0);
        assert!(validate_config(44100, &conf).is_err());
        conf.high_q = Some(1e10);
        assert!(validate_config(44100, &conf).is_err());
        conf.high_q = Some(1.5);
        assert!(validate_config(44100, &conf).is_ok());
    }
}
