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

/// Validate a Delay config. Issue paths are relative to the parameters.
pub fn validate_delay_config(conf: &config::DelayParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if conf.delay < 0.0 {
        issues.invalid(issue_path!["delay"], "Delay cannot be negative");
    }
    issues.into_result(())
}

/// Validate a Volume config. Issue paths are relative to the parameters.
pub fn validate_volume_config(conf: &config::VolumeParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if conf.ramp_time_ms() < 0.0 {
        issues.invalid(issue_path!["ramp_time_ms"], "Ramp time cannot be negative");
    }
    issues.into_result(())
}

/// Validate a Gain config. Issue paths are relative to the parameters.
pub fn validate_gain_config(conf: &config::GainParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if conf.scale() == config::GainScale::Decibel {
        if conf.gain < -150.0 {
            issues.invalid(issue_path!["gain"], "Gain must be larger than -150 dB");
        } else if conf.gain > 150.0 {
            issues.invalid(issue_path!["gain"], "Gain must be less than +150 dB");
        }
    } else if conf.gain < -10.0 {
        issues.invalid(issue_path!["gain"], "Linear gain must be larger than -10.0");
    } else if conf.gain > 10.0 {
        issues.invalid(issue_path!["gain"], "Linear gain must be less than +10.0");
    }
    issues.into_result(())
}
