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

//! The config types, loading and validation live in the `camilladsp-schema` crate.
pub use camilladsp_schema::config::*;

/// Shorthand for [`FiniteF64::expect_finite`], for building config values in tests and in the
/// filter builders that generate sections from constants.
macro_rules! finite {
    ($value:expr) => {
        $crate::config::FiniteF64::expect_finite($value)
    };
}

/// The [`FiniteF32`] counterpart of [`finite!`]. Only the tests build `f32` config values
/// directly, everything else reads them through a getter.
#[cfg(test)]
macro_rules! finite32 {
    ($value:expr) => {
        $crate::config::FiniteF32::expect_finite($value)
    };
}

pub(crate) use finite;
#[cfg(test)]
pub(crate) use finite32;
