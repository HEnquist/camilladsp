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

/// Clamp the lower limit of the dB value to -200 dB,
/// which is below the dynamic range of 32-bit integers
/// and should be sufficient for all practical purposes.
pub fn linear_to_db(value: f32) -> f32 {
    if value.abs() < 4.66e-10 {
        -200.0
    } else {
        20.0 * value.log10()
    }
}

/// Convert a dB value to a linear amplitude ratio.
///
/// Generic over the float type: setup code calls this with `f64`, while the
/// compressor calls it per sample at the processing precision, where converting
/// up to `f64` and back would cost more than the extra precision is worth.
pub fn db_to_linear<T: num_traits::Float>(value: T) -> T {
    let ten = T::from(10.0).unwrap();
    let twenty = T::from(20.0).unwrap();
    ten.powf(value / twenty)
}

/// Compute a gain factor from a gain value that may be linear or dB, optionally inverted or muted.
pub fn gain_from_value<T: num_traits::Float>(
    gain_value: T,
    linear: bool,
    inverted: bool,
    mute: bool,
) -> T {
    let mut gain = if linear {
        gain_value
    } else {
        db_to_linear(gain_value)
    };
    if inverted {
        gain = -gain;
    }
    if mute { T::zero() } else { gain }
}

/// Convert a slice of linear amplitude values (0..1) to dB in place.
pub fn linear_to_db_inplace(values: &mut [f32]) {
    values.iter_mut().for_each(|val| {
        *val = linear_to_db(*val);
    });
}
