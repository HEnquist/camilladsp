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
use crate::config::{Issues, issue_path};

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

/// Collect the settings of every fader, and report each Volume filter that
/// disagrees with the first one on the same fader. The first filter wins.
fn collect_fader_settings(conf: &config::Configuration) -> ([FaderSettings; NUM_FADERS], Issues) {
    let mut settings = [UNUSED_AUX_FADER; NUM_FADERS];
    settings[0] = FaderSettings {
        ramp_time_ms: conf.devices.volume_ramp_time_ms(),
        limit: conf.devices.volume_limit(),
    };
    let mut set_by: [Option<&str>; NUM_FADERS] = [None; NUM_FADERS];
    let mut conflicts = Issues::new();
    let (Some(pipeline), Some(filters)) = (&conf.pipeline, &conf.filters) else {
        return (settings, conflicts);
    };
    for (idx, step) in pipeline.iter().enumerate() {
        let config::PipelineStep::Filter(step) = step else {
            continue;
        };
        if step.is_bypassed() || step.channels.as_ref().is_some_and(|ch| ch.is_empty()) {
            continue;
        }
        for (n, name) in step.names.iter().enumerate() {
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
                Some(first) if settings[fader] != these => {
                    let msg = format!(
                        "Volume filters '{first}' and '{name}' use the same fader {:?}, \
                        but have different ramp_time_ms or limit",
                        parameters.fader
                    );
                    conflicts.invalid(issue_path!["pipeline", idx, "names", n], msg);
                }
                Some(_) => {}
            }
        }
    }
    (settings, conflicts)
}

/// The ramp time and limit of every fader in `conf`. Fader 0 takes them from
/// the devices section, the aux faders from the Volume filters that use them.
pub fn fader_settings(conf: &config::Configuration) -> [FaderSettings; NUM_FADERS] {
    collect_fader_settings(conf).0
}

/// Check that all Volume filters sharing a fader agree on its settings.
///
/// Issue paths are from the root of the config, at the pipeline step that uses
/// a disagreeing filter.
pub fn validate_fader_settings(conf: &config::Configuration) -> Result<(), Issues> {
    collect_fader_settings(conf).1.into_result(())
}
