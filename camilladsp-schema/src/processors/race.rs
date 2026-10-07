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
use crate::config::{Issues, issue_path};
use crate::processors::check_channel_count;

/// Validate the RACE processor config, to give a helpful message intead of a panic.
///
/// Issue paths are relative to the parameters.
pub fn validate_race(config: &config::RACEParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let channels = config.channels;
    if config.attenuation <= 0.0 {
        let msg = "Attenuation value must be larger than zero.";
        issues.invalid(issue_path!["attenuation"], msg);
    }
    if config.delay <= 0.0 {
        let msg = "Delay value must be larger than zero.";
        issues.invalid(issue_path!["delay"], msg);
    }
    if config.channel_a == config.channel_b {
        let msg = "Channels a and b must be different";
        issues.invalid(issue_path!["channel_b"], msg);
    }
    if check_channel_count(&mut issues, channels) {
        if config.channel_a >= channels {
            let msg = format!(
                "Invalid channel a to process: {}, max is: {}.",
                config.channel_a,
                channels - 1
            );
            issues.invalid(issue_path!["channel_a"], msg);
        }
        if config.channel_b >= channels {
            let msg = format!(
                "Invalid channel b to process: {}, max is: {}.",
                config.channel_b,
                channels - 1
            );
            issues.invalid(issue_path!["channel_b"], msg);
        }
    }
    issues.into_result(())
}
