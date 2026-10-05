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

use crate::config::{Issues, issue_path};

/// Dynamic range compressor processor.
pub mod compressor;
/// Multichannel lookahead limiter processor.
pub mod lookahead_limiter;
/// Noise gate processor.
pub mod noisegate;
/// RACE (Recursive Ambiophonic Crosstalk Elimination) processor.
pub mod race;

/// Check that a processor has channels at all. Its count is also checked against
/// the pipeline, but the channel indexes cannot be checked against zero.
fn check_channel_count(issues: &mut Issues, channels: usize) -> bool {
    if channels == 0 {
        issues.invalid(
            issue_path!["channels"],
            "Channels must be larger than zero.",
        );
        return false;
    }
    true
}

/// Check that the monitored and processed channels of a processor exist.
fn check_channel_lists(
    issues: &mut Issues,
    channels: usize,
    monitor_channels: &[usize],
    process_channels: &[usize],
) {
    if !check_channel_count(issues, channels) {
        return;
    }
    let max = channels - 1;
    for (n, ch) in monitor_channels.iter().enumerate() {
        if *ch >= channels {
            let msg = format!("Invalid monitor channel: {}, max is: {}.", *ch, max);
            issues.invalid(issue_path!["monitor_channels", n], msg);
        }
    }
    for (n, ch) in process_channels.iter().enumerate() {
        if *ch >= channels {
            let msg = format!("Invalid channel to process: {}, max is: {}.", *ch, max);
            issues.invalid(issue_path!["process_channels", n], msg);
        }
    }
}
