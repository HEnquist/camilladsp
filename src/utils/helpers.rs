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

// Inplace recalculation of values positive values 0..1 to dB.
pub fn linear_to_db(values: &mut [f32]) {
    values.iter_mut().for_each(|val| {
        if *val == 0.0 {
            *val = -1000.0;
        } else {
            *val = 20.0 * val.log10();
        }
    });
}
