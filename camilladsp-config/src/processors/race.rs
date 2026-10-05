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

/// Validate the RACE processor config, to give a helpful message intead of a panic.
pub fn validate_race(config: &config::RACEParameters) -> Res<()> {
    let channels = config.channels;
    if config.attenuation <= 0.0 {
        let msg = "Attenuation value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.delay <= 0.0 {
        let msg = "Delay value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.channel_a == config.channel_b {
        let msg = "Channels a and b must be different";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.channel_a >= channels {
        let msg = format!(
            "Invalid channel a to process: {}, max is: {}.",
            config.channel_a,
            channels - 1
        );
        return Err(config::ConfigError::new(&msg).into());
    }
    if config.channel_b >= channels {
        let msg = format!(
            "Invalid channel b to process: {}, max is: {}.",
            config.channel_b,
            channels - 1
        );
        return Err(config::ConfigError::new(&msg).into());
    }
    Ok(())
}
