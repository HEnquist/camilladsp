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
use crate::config::Issues;
use crate::filters::lookahead_limiter::validate_times;
use crate::processors::check_channel_lists;

/// Validate the lookahead limiter config, to give a helpful message intead of a panic.
///
/// Issue paths are relative to the parameters.
pub fn validate_lookahead_limiter(
    config: &config::LookaheadLimiterProcessorParameters,
    samplerate: usize,
) -> Result<(), Issues> {
    let mut issues = Issues::new();
    issues.nest_result(
        Vec::new(),
        validate_times(
            config.attack.get(),
            config.attack_unit,
            config.release.get(),
            samplerate,
        ),
    );
    check_channel_lists(
        &mut issues,
        config.channels,
        &config.monitor_channels(),
        &config.process_channels(),
    );
    issues.into_result(())
}
