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

/// Validate the noise gate config, to give a helpful message intead of a panic.
pub fn validate_noise_gate(config: &config::NoiseGateParameters) -> Res<()> {
    let channels = config.channels;
    if config.attack <= 0.0 {
        let msg = "Attack value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.release <= 0.0 {
        let msg = "Release value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    for ch in config.monitor_channels().iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid monitor channel: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    for ch in config.process_channels().iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid channel to process: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    Ok(())
}
