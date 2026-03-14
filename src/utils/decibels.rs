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

pub fn linear_to_db(value: f32) -> f32 {
    if value == 0.0 {
        -1000.0
    } else {
        20.0 * value.log10()
    }
}

pub fn db_to_linear(value: PrcFmt) -> PrcFmt {
    (10.0 as PrcFmt).powf(value / 20.0)
}

pub fn gain_from_value(gain_value: PrcFmt, linear: bool, inverted: bool, mute: bool) -> PrcFmt {
    let mut gain = if linear {
        gain_value
    } else {
        db_to_linear(gain_value)
    };
    if inverted {
        gain = -gain;
    }
    if mute { 0.0 } else { gain }
}

// Inplace recalculation of values positive values 0..1 to dB.
pub fn linear_to_db_inplace(values: &mut [f32]) {
    values.iter_mut().for_each(|val| {
        *val = linear_to_db(*val);
    });
}
