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

use crate::Res;
use crate::config;

/// Below this the shelf spreads out so far that it no longer reaches its
/// nominal boost within the audio band.
const MIN_Q: f64 = 0.1;
/// Above this the shelf overshoots badly at the corner frequency. At the
/// largest allowed boost of 20 dB, a Q of 2.0 already peaks 5.6 dB above the
/// shelf level.
const MAX_Q: f64 = 2.0;

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
