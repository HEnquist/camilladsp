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

/// Validate a Loudness config.
pub fn validate_delay_config(conf: &config::DelayParameters) -> Res<()> {
    if conf.delay < 0.0 {
        return Err(config::ConfigError::new("Delay cannot be negative").into());
    }
    Ok(())
}

/// Validate a Volume config.
pub fn validate_volume_config(conf: &config::VolumeParameters) -> Res<()> {
    if conf.ramp_time_ms() < 0.0 {
        return Err(config::ConfigError::new("Ramp time cannot be negative").into());
    }
    Ok(())
}

/// Validate a Gain config.
pub fn validate_gain_config(conf: &config::GainParameters) -> Res<()> {
    if conf.scale() == config::GainScale::Decibel {
        if conf.gain < -150.0 {
            return Err(config::ConfigError::new("Gain must be larger than -150 dB").into());
        } else if conf.gain > 150.0 {
            return Err(config::ConfigError::new("Gain must be less than +150 dB").into());
        }
    } else if conf.gain < -10.0 {
        return Err(config::ConfigError::new("Linear gain must be larger than -10.0").into());
    } else if conf.gain > 10.0 {
        return Err(config::ConfigError::new("Linear gain must be less than +10.0").into());
    }
    Ok(())
}
