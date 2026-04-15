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

use crate::PrcFmt;
use crate::config::TimeUnit;

/// Convert a time value with a given unit to a number of samples (floating point).
/// The result is exact, not rounded.
pub fn time_to_samples(value: PrcFmt, unit: TimeUnit, samplerate: usize) -> PrcFmt {
    match unit {
        TimeUnit::Microseconds => value / 1_000_000.0 * samplerate as PrcFmt,
        TimeUnit::Milliseconds => value / 1000.0 * samplerate as PrcFmt,
        TimeUnit::Millimetres => value / 1000.0 * samplerate as PrcFmt / 343.0,
        TimeUnit::Samples => value,
    }
}
