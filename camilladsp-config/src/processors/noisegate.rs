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
use crate::processors::check_channel_lists;

/// Validate the noise gate config, to give a helpful message intead of a panic.
///
/// Issue paths are relative to the parameters.
pub fn validate_noise_gate(config: &config::NoiseGateParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    if config.attack <= 0.0 {
        let msg = "Attack value must be larger than zero.";
        issues.invalid(issue_path!["attack"], msg);
    }
    if config.release <= 0.0 {
        let msg = "Release value must be larger than zero.";
        issues.invalid(issue_path!["release"], msg);
    }
    check_channel_lists(
        &mut issues,
        config.channels,
        &config.monitor_channels(),
        &config.process_channels(),
    );
    issues.into_result(())
}
