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

/// Below this the shelf spreads out so far that it no longer reaches its
/// nominal boost within the audio band.
const MIN_Q: f64 = 0.1;
/// Above this the shelf overshoots badly at the corner frequency. At the
/// largest allowed boost of 20 dB, a Q of 2.0 already peaks 5.6 dB above the
/// shelf level.
const MAX_Q: f64 = 2.0;

/// Validate a Loudness config. Issue paths are relative to the parameters.
pub fn validate_config(samplerate: usize, conf: &config::LoudnessParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if conf.reference_level > 20.0 {
        issues.invalid(
            issue_path!["reference_level"],
            "Reference level must be less than 20",
        );
    } else if conf.reference_level < -100.0 {
        issues.invalid(
            issue_path!["reference_level"],
            "Reference level must be higher than -100",
        );
    }
    if conf.high_boost() < 0.0 {
        issues.invalid(
            issue_path!["high_boost"],
            "High boost cannot be less than 0",
        );
    } else if conf.high_boost() > 20.0 {
        issues.invalid(
            issue_path!["high_boost"],
            "High boost cannot be larger than 20",
        );
    }
    if conf.low_boost() < 0.0 {
        issues.invalid(issue_path!["low_boost"], "Low boost cannot be less than 0");
    } else if conf.low_boost() > 20.0 {
        issues.invalid(
            issue_path!["low_boost"],
            "Low boost cannot be larger than 20",
        );
    }
    if conf.low_freq() <= 0.0 {
        issues.invalid(issue_path!["low_freq"], "Low freq must be > 0");
    }
    if conf.high_freq() >= samplerate as f64 / 2.0 {
        issues.invalid(issue_path!["high_freq"], "High freq must be < samplerate/2");
    } else if conf.high_freq() <= conf.low_freq() {
        issues.invalid(
            issue_path!["high_freq"],
            "High freq must be higher than low freq",
        );
    }
    if !(MIN_Q..=MAX_Q).contains(&conf.high_q()) {
        issues.invalid(
            issue_path!["high_q"],
            format!("High Q must be between {MIN_Q:.1} and {MAX_Q:.1}"),
        );
    }
    if !(MIN_Q..=MAX_Q).contains(&conf.low_q()) {
        issues.invalid(
            issue_path!["low_q"],
            format!("Low Q must be between {MIN_Q:.1} and {MAX_Q:.1}"),
        );
    }
    issues.into_result(())
}
