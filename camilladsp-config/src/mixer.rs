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

/// Validate the mixer config, to give a helpful message intead of a panic.
pub fn validate_mixer(mixer_config: &config::Mixer) -> Res<()> {
    let chan_in = mixer_config.channels.input();
    let chan_out = mixer_config.channels.output();
    let mut output_channels: Vec<usize> = Vec::with_capacity(chan_out);
    let mut input_channels: Vec<usize> = Vec::with_capacity(chan_in);
    for mapping in mixer_config.mapping.iter() {
        if mapping.dest >= chan_out {
            let msg = format!(
                "Invalid destination channel {}, max is {}.",
                mapping.dest,
                chan_out - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
        if output_channels.contains(&mapping.dest) {
            let msg = format!(
                "There is more than one mapping for destination channel {}",
                mapping.dest,
            );
            return Err(config::ConfigError::new(&msg).into());
        }
        output_channels.push(mapping.dest);
        input_channels.clear();
        for source in mapping.sources.iter() {
            if source.channel >= chan_in {
                let msg = format!(
                    "Invalid source channel {}, max is {}.",
                    source.channel,
                    chan_in - 1
                );
                return Err(config::ConfigError::new(&msg).into());
            }
            if input_channels.contains(&source.channel) {
                let msg = format!(
                    "Input channel {} is listed mote than once for destination channel {}",
                    source.channel, mapping.dest,
                );
                return Err(config::ConfigError::new(&msg).into());
            }
        }
    }
    Ok(())
}

/// Get a vector showing which input channels are used
pub fn used_input_channels(mixer_config: &config::Mixer) -> Vec<bool> {
    let chan_in = mixer_config.channels.input();
    let mut used_channels = vec![false; chan_in];
    for mapping in mixer_config.mapping.iter() {
        if !mapping.is_mute() {
            for source in mapping.sources.iter() {
                if !source.is_mute() {
                    used_channels[source.channel] = true;
                }
            }
        }
    }
    used_channels
}
