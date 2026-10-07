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

/// Validate a Dither config. Issue paths are relative to the parameters.
pub fn validate_config(conf: &config::DitherParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let bits = match conf {
        config::DitherParameters::None { bits }
        | config::DitherParameters::Flat { bits, .. }
        | config::DitherParameters::Highpass { bits }
        | config::DitherParameters::Fweighted441 { bits }
        | config::DitherParameters::FweightedLong441 { bits }
        | config::DitherParameters::FweightedShort441 { bits }
        | config::DitherParameters::Gesemann441 { bits }
        | config::DitherParameters::Gesemann48 { bits }
        | config::DitherParameters::Lipshitz441 { bits }
        | config::DitherParameters::LipshitzLong441 { bits }
        | config::DitherParameters::Shibata441 { bits }
        | config::DitherParameters::ShibataHigh441 { bits }
        | config::DitherParameters::ShibataLow441 { bits }
        | config::DitherParameters::Shibata48 { bits }
        | config::DitherParameters::ShibataHigh48 { bits }
        | config::DitherParameters::ShibataLow48 { bits }
        | config::DitherParameters::Shibata882 { bits }
        | config::DitherParameters::ShibataLow882 { bits }
        | config::DitherParameters::Shibata96 { bits }
        | config::DitherParameters::ShibataLow96 { bits }
        | config::DitherParameters::Shibata192 { bits }
        | config::DitherParameters::ShibataLow192 { bits } => bits,
    };
    if *bits <= 1 {
        issues.invalid(issue_path!["bits"], "Dither bit depth must be at least 2");
    }

    if let config::DitherParameters::Flat { amplitude, .. } = conf {
        if *amplitude < 0.0 {
            issues.invalid(
                issue_path!["amplitude"],
                "Dither amplitude cannot be negative",
            );
        }
        if *amplitude > 100.0 {
            issues.invalid(
                issue_path!["amplitude"],
                "Dither amplitude must be less than 100",
            );
        }
    }

    issues.into_result(())
}
