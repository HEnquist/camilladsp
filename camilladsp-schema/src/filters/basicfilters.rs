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
    // The same range as the main fader has in the devices section.
    if conf.limit() > 50.0 {
        issues.invalid(issue_path!["limit"], "Volume limit cannot be above +50 dB");
    } else if conf.limit() < -150.0 {
        issues.invalid(
            issue_path!["limit"],
            "Volume limit cannot be less than -150 dB",
        );
    }
    issues.into_result(())
}

/// Check a gain value, in dB or linear depending on `scale`.
///
/// Shared by the Gain filter and the mixer sources, which take the same kind of gain.
pub fn check_gain(issues: &mut Issues, field: &str, gain: f64, scale: config::GainScale) {
    if scale == config::GainScale::Decibel {
        if gain < -150.0 {
            issues.invalid(issue_path![field], "Gain must be larger than -150 dB");
        } else if gain > 150.0 {
            issues.invalid(issue_path![field], "Gain must be less than +150 dB");
        }
    } else if gain < -10.0 {
        issues.invalid(issue_path![field], "Linear gain must be larger than -10.0");
    } else if gain > 10.0 {
        issues.invalid(issue_path![field], "Linear gain must be less than +10.0");
    }
}

/// Validate a Gain config. Issue paths are relative to the parameters.
pub fn validate_gain_config(conf: &config::GainParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    check_gain(&mut issues, "gain", conf.gain.get(), conf.scale());
    issues.into_result(())
}
