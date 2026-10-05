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

//! Fader settings, as given by a configuration.

use crate::config;

/// Number of independent volume faders, the main fader and the four aux faders.
pub const NUM_FADERS: usize = 5;

/// Ramp time and volume limit of one fader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaderSettings {
    pub ramp_time_ms: f32,
    pub limit: f32,
}

/// Settings for an aux fader that no Volume filter uses. With nothing to ramp,
/// its level follows the target directly.
pub const UNUSED_AUX_FADER: FaderSettings = FaderSettings {
    ramp_time_ms: 0.0,
    limit: 50.0,
};

/// Collect the settings of every fader, and describe the first conflict if two
/// Volume filters on the same fader disagree. The first filter wins.
fn collect_fader_settings(
    conf: &config::Configuration,
) -> ([FaderSettings; NUM_FADERS], Option<String>) {
    let mut settings = [UNUSED_AUX_FADER; NUM_FADERS];
    settings[0] = FaderSettings {
        ramp_time_ms: conf.devices.volume_ramp_time_ms(),
        limit: conf.devices.volume_limit(),
    };
    let mut set_by: [Option<&str>; NUM_FADERS] = [None; NUM_FADERS];
    let mut conflict = None;
    let (Some(pipeline), Some(filters)) = (&conf.pipeline, &conf.filters) else {
        return (settings, conflict);
    };
    for step in pipeline {
        let config::PipelineStep::Filter(step) = step else {
            continue;
        };
        if step.is_bypassed() || step.channels.as_ref().is_some_and(|ch| ch.is_empty()) {
            continue;
        }
        for name in &step.names {
            let Some(config::Filter::Volume { parameters, .. }) = filters.get(name) else {
                continue;
            };
            let fader = parameters.fader as usize;
            let these = FaderSettings {
                ramp_time_ms: parameters.ramp_time_ms(),
                limit: parameters.limit(),
            };
            match set_by[fader] {
                None => {
                    settings[fader] = these;
                    set_by[fader] = Some(name);
                }
                Some(first) if conflict.is_none() && settings[fader] != these => {
                    conflict = Some(format!(
                        "Volume filters '{first}' and '{name}' use the same fader {:?}, \
                        but have different ramp_time_ms or limit",
                        parameters.fader
                    ));
                }
                Some(_) => {}
            }
        }
    }
    (settings, conflict)
}

/// The ramp time and limit of every fader in `conf`. Fader 0 takes them from
/// the devices section, the aux faders from the Volume filters that use them.
pub fn fader_settings(conf: &config::Configuration) -> [FaderSettings; NUM_FADERS] {
    collect_fader_settings(conf).0
}

/// Check that all Volume filters sharing a fader agree on its settings.
pub fn validate_fader_settings(conf: &config::Configuration) -> Result<(), config::ConfigError> {
    match collect_fader_settings(conf).1 {
        Some(msg) => Err(config::ConfigError::new(&msg)),
        None => Ok(()),
    }
}
