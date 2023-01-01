use crate::biquad;
use crate::config;
use crate::filters::Filter;
use std::sync::Arc;

use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::Res;

pub struct Loudness {
    pub name: String,
    current_volume: PrcFmt,
    processing_params: Arc<ProcessingParameters>,
    reference_level: f32,
    high_boost: f32,
    low_boost: f32,
    high_biquad: biquad::Biquad,
    low_biquad: biquad::Biquad,
    fader: usize,
    active: bool,
}

fn rel_boost(level: f32, reference: f32) -> f32 {
    let rel_boost = (reference - level) / 20.0;
    rel_boost.clamp(0.0, 1.0)
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
        let highshelf_conf = config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope {
            freq: 3500.0,
            slope: 12.0,
            gain: (relboost * conf.high_boost()) as PrcFmt,
        });
        let lowshelf_conf = config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope {
            freq: 70.0,
            slope: 12.0,
            gain: (relboost * conf.low_boost()) as PrcFmt,
        });
        let high_biquad_coeffs =
            biquad::BiquadCoefficients::from_config(samplerate, highshelf_conf);
        let low_biquad_coeffs = biquad::BiquadCoefficients::from_config(samplerate, lowshelf_conf);
        let high_biquad = biquad::Biquad::new("highshelf", samplerate, high_biquad_coeffs);
        let low_biquad = biquad::Biquad::new("lowshelf", samplerate, low_biquad_coeffs);
        Loudness {
            name: name.to_string(),
            current_volume: current_volume as PrcFmt,
            reference_level: conf.reference_level,
            high_boost: conf.high_boost(),
            low_boost: conf.low_boost(),
            high_biquad,
            low_biquad,
            processing_params,
            fader,
            active,
        }
    }
}

impl Filter for Loudness {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        let shared_vol = self.processing_params.current_volume(self.fader);

        // Volume setting changed
        if (shared_vol - self.current_volume as f32).abs() > 0.01 {
            self.current_volume = shared_vol as PrcFmt;
            let relboost = rel_boost(self.current_volume as f32, self.reference_level);
            self.active = relboost > 0.001;
            info!(
                "Updating loudness biquads, relative boost {}%",
                100.0 * relboost
            );
            let highshelf_conf =
                config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope {
                    freq: 3500.0,
                    slope: 12.0,
                    gain: (relboost * self.high_boost) as PrcFmt,
                });
            let lowshelf_conf = config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope {
                freq: 70.0,
                slope: 12.0,
                gain: (relboost * self.low_boost) as PrcFmt,
            });
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
        }
        if self.active {
            trace!("Applying loudness biquads");
            self.high_biquad.process_waveform(waveform).unwrap();
            self.low_biquad.process_waveform(waveform).unwrap();
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Loudness {
            parameters: conf, ..
        } = conf
        {
            self.fader = conf.fader();
            let current_volume = self.processing_params.current_volume(self.fader);
            let relboost = rel_boost(current_volume, conf.reference_level);
            self.active = relboost > 0.001;
            let highshelf_conf =
                config::BiquadParameters::Highshelf(config::ShelfSteepness::Slope {
                    freq: 3500.0,
                    slope: 12.0,
                    gain: (relboost * conf.high_boost()) as PrcFmt,
                });
            let lowshelf_conf = config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope {
                freq: 70.0,
                slope: 12.0,
                gain: (relboost * conf.low_boost()) as PrcFmt,
            });
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
            self.reference_level = conf.reference_level;
            self.high_boost = conf.high_boost();
            self.low_boost = conf.low_boost();
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a Loudness config.
pub fn validate_config(conf: &config::LoudnessParameters) -> Res<()> {
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
    }
    Ok(())
}
