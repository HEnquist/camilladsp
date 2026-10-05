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

use crate::Res;
use crate::config;

/// Check that the poles of the filter are inside the unit circle.
///
/// This is the step-down procedure, also known as the Schur-Cohn stability test,
/// see for example Julius O. Smith III, "Introduction to Digital Filters with Audio
/// Applications", section "Computing Reflection Coefficients to Check Filter Stability":
/// <https://ccrma.stanford.edu/~jos/filters/Computing_Reflection_Coefficients_Check.html>
/// The denominator polynomial is peeled down one order at a time, and the reflection
/// coefficient of each step is the highest order coefficient of the polynomial at that step.
/// All poles are inside the unit circle if and only if every reflection coefficient is.
/// No root finding is needed.
///
/// The coefficients must be scaled so that a0 is unity.
/// The check runs in f64 to give the same verdict in both processing precisions.
fn poles_inside_unit_circle(a: &[f64]) -> bool {
    let mut coeffs = a.to_vec();
    for order in (1..coeffs.len()).rev() {
        let reflection = coeffs[order];
        if reflection.abs() >= 1.0 {
            return false;
        }
        let scale = 1.0 - reflection * reflection;
        let prev = coeffs.clone();
        for (n, coeff) in coeffs.iter_mut().enumerate().take(order).skip(1) {
            *coeff = (prev[n] - reflection * prev[order - n]) / scale;
        }
        coeffs.truncate(order);
    }
    true
}

pub fn validate_config(parameters: &config::DiffEqParameters) -> Res<()> {
    let a = parameters.a();
    let b = parameters.b();
    if a.iter().chain(b.iter()).any(|coeff| !coeff.is_finite()) {
        return Err(config::ConfigError::new("All coefficients must be finite numbers").into());
    }
    if a.is_empty() {
        // Defaults to a single unity coefficient, which gives a stable FIR filter.
        return Ok(());
    }
    if a[0] == 0.0 {
        return Err(config::ConfigError::new("The first 'a' coefficient must not be zero").into());
    }
    let scaled: Vec<f64> = a.iter().map(|coeff| coeff / a[0]).collect();
    if !poles_inside_unit_circle(&scaled) {
        return Err(config::ConfigError::new(
            "Unstable filter, the 'a' coefficients give poles on or outside the unit circle",
        )
        .into());
    }
    Ok(())
}
