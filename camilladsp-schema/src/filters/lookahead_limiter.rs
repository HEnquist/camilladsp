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
use crate::config::TimeUnit;
use crate::config::{Issues, issue_path};
use crate::utils::time::time_to_samples;

/// Validate the attack and release times of a lookahead limiter.
///
/// Issue paths are the `attack` and `release` fields of the parameters.
pub fn validate_times(
    attack: f64,
    attack_unit: TimeUnit,
    release: f64,
    samplerate: usize,
) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if attack < 0.0 {
        let msg = "Attack time must be greater than or equal to 0.";
        issues.invalid(issue_path!["attack"], msg);
    } else {
        let attack_samples = time_to_samples(attack, attack_unit, samplerate).round() as usize;
        if attack_samples > samplerate {
            let msg = "Lookahead limiter attack time must be less than or equal to 1 second.";
            issues.invalid(issue_path!["attack"], msg);
        }
    }
    if release < 0.0 {
        let msg = "Release time must be greater than or equal to 0.";
        issues.invalid(issue_path!["release"], msg);
    }
    issues.into_result(())
}

/// Validate a LookaheadLimiter filter config. Issue paths are relative to the parameters.
pub fn validate_config(
    config: &config::LookaheadLimiterParameters,
    samplerate: usize,
) -> Result<(), Issues> {
    validate_times(
        config.attack.get(),
        config.attack_unit(),
        config.release.get(),
        samplerate,
    )
}
