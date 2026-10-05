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

//! The configuration side of CamillaDSP: the config types, every validation
//! rule, and reading of coefficient files. No audio or DSP dependencies.
//!
//! The modules mirror the layout of the `camilladsp` crate, which re-exports
//! everything here from the same paths.

// Full-precision `f64` literals, correct for the default build, hold more digits
// than an `f32` build can represent. Silence that only in the f32 build.
#![cfg_attr(camillafloat_f32, allow(clippy::excessive_precision))]

#[macro_use]
extern crate log;

use std::error;

/// Internal floating-point sample type: `f64` by default, `f32` in an f32 build.
///
/// `f64` is correct for nearly all use cases. `f32` is available for the few
/// setups where it measurably helps, mainly resampling and FIR convolution on
/// weak in-order CPUs, and is deliberately not a Cargo feature: features are
/// unified across the whole dependency graph, so any crate depending on this one
/// could silently flip the precision for everyone else in the build. It is a raw
/// rustc cfg instead, set with:
///
/// ```text
/// RUSTFLAGS="--cfg camillafloat_f32" cargo build --release
/// ```
#[cfg(camillafloat_f32)]
pub type CamillaFloat = f32;
/// Internal floating-point sample type: `f64` by default, `f32` in an f32 build.
///
/// See the f32 variant of this alias for how to select the other precision.
#[cfg(not(camillafloat_f32))]
pub type CamillaFloat = f64;

/// Conversion from a setup-time `f64` value to the processing precision.
///
/// Configuration values and filter coefficient math always run in `f64`, no
/// matter what [`CamillaFloat`] is, so that an f32 build gets the same
/// coefficients as an f64 one and only rounds once, on the way in. This trait
/// marks that single crossing point: a no-op in a default build, a narrowing
/// conversion in an f32 build.
pub trait ToCamillaFloat {
    /// Convert a setup value into the processing precision.
    fn to_camilla_float(self) -> CamillaFloat;
}

#[cfg(camillafloat_f32)]
impl ToCamillaFloat for f64 {
    #[inline]
    fn to_camilla_float(self) -> CamillaFloat {
        self as f32
    }
}

#[cfg(not(camillafloat_f32))]
impl ToCamillaFloat for f64 {
    #[inline]
    fn to_camilla_float(self) -> CamillaFloat {
        self
    }
}

/// Conversion from the processing precision down to `f32`.
///
/// Signal levels, volumes and spectrum data are reported as `f32` whatever
/// [`CamillaFloat`] is. Implemented for both float types and written as a method
/// rather than an `as` cast, so that the direction which is a no-op in a given
/// build does not need a blanket `clippy::unnecessary_cast` allow over a whole
/// file, which would also hide genuinely redundant casts.
pub trait ToF32 {
    /// Convert to `f32` for reporting.
    fn to_f32(self) -> f32;
}

impl ToF32 for f64 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
}

impl ToF32 for f32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
}

/// Conversion from the processing precision up to `f64`.
///
/// Analysis that must stay numerically robust whatever [`CamillaFloat`] is,
/// such as the biquad state guard, works in `f64` throughout. Written as a
/// method rather than an `as` cast for the same reason as [`ToF32`]: the
/// direction that is a no-op in a given build would otherwise need a blanket
/// `clippy::unnecessary_cast` allow over a whole file.
pub trait ToF64 {
    /// Convert up to `f64` for analysis.
    fn to_f64(self) -> f64;
}

impl ToF64 for f32 {
    #[inline]
    fn to_f64(self) -> f64 {
        self as f64
    }
}

impl ToF64 for f64 {
    #[inline]
    fn to_f64(self) -> f64 {
        self
    }
}

/// Convenience `Result` type used throughout CamillaDSP.
pub type Res<T> = Result<T, Box<dyn error::Error>>;

/// Configuration parsing, validation, and type definitions.
pub mod config;
/// Fader settings collected from a configuration.
pub mod fader;
/// Validation of filter parameters, filter coefficient design and coefficient file reading.
pub mod filters;
/// Validation of mixer configs.
pub mod mixer;
/// Validation of processor parameters.
pub mod processors;
/// Time unit conversions and wav header handling.
pub mod utils;
