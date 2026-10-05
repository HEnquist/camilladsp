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
use crate::config::TimeUnit;
use crate::utils::time::time_to_samples;

/// Validate the attack and release times of a lookahead limiter.
pub fn validate_times(
    attack: f64,
    attack_unit: TimeUnit,
    release: f64,
    samplerate: usize,
) -> Res<()> {
    if attack < 0.0 {
        let msg = "Attack time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    let attack_samples = time_to_samples(attack, attack_unit, samplerate).round() as usize;
    if attack_samples > samplerate {
        let msg = "Lookahead limiter attack time must be less than or equal to 1 second.";
        return Err(config::ConfigError::new(msg).into());
    }
    if release < 0.0 {
        let msg = "Release time must be greater than or equal to 0.";
        return Err(config::ConfigError::new(msg).into());
    }
    Ok(())
}

pub fn validate_config(config: &config::LookaheadLimiterParameters, samplerate: usize) -> Res<()> {
    validate_times(
        config.attack.get(),
        config.attack_unit(),
        config.release.get(),
        samplerate,
    )
}
