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
use crate::filters::biquad;

/// Expand the bands of an NPointPeq into biquad parameters.
/// The first band becomes a low shelf and the last a high shelf,
/// with the ones in between as peaking filters.
pub fn npeq_sections(bands: &[config::PeqBand]) -> Vec<config::BiquadParameters> {
    let last = bands.len().saturating_sub(1);
    bands
        .iter()
        .enumerate()
        .map(|(n, band)| {
            let config::PeqBand { freq, q, gain } = *band;
            if n == 0 {
                config::BiquadParameters::Lowshelf(config::ShelfSteepness::Q { freq, q, gain })
            } else if n == last {
                config::BiquadParameters::Highshelf(config::ShelfSteepness::Q { freq, q, gain })
            } else {
                config::BiquadParameters::Peaking(config::PeakingWidth::Q { freq, q, gain })
            }
        })
        .collect()
}

/// Check a crossover frequency against zero and the Nyquist limit.
fn check_freq(issues: &mut Issues, freq: f64, maxfreq: f64) {
    if freq <= 0.0 {
        issues.invalid(issue_path!["freq"], "Frequency must be > 0");
    } else if freq >= maxfreq {
        issues.invalid(issue_path!["freq"], "Frequency must be < samplerate/2");
    }
}

/// Validate a BiquadCombo config. Issue paths are relative to the parameters.
pub fn validate_config(
    samplerate: usize,
    conf: &config::BiquadComboParameters,
) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let maxfreq = samplerate as f64 / 2.0;
    match conf {
        config::BiquadComboParameters::LinkwitzRileyHighpass { freq, order }
        | config::BiquadComboParameters::LinkwitzRileyLowpass { freq, order } => {
            check_freq(&mut issues, freq.get(), maxfreq);
            if (*order % 2 > 0) || (*order == 0) {
                issues.invalid(
                    issue_path!["order"],
                    "LR order must be an even non-zero number",
                );
            }
        }
        config::BiquadComboParameters::ButterworthHighpass { freq, order }
        | config::BiquadComboParameters::ButterworthLowpass { freq, order } => {
            check_freq(&mut issues, freq.get(), maxfreq);
            if *order == 0 {
                issues.invalid(
                    issue_path!["order"],
                    "Butterworth order must be larger than zero",
                );
            }
        }
        config::BiquadComboParameters::Tilt { gain } => {
            if *gain <= -100.0 {
                issues.invalid(issue_path!["gain"], "Gain must be > -100");
            } else if *gain >= 100.0 {
                issues.invalid(issue_path!["gain"], "Gain must be < 100");
            }
        }
        config::BiquadComboParameters::NPointPeq { bands } => {
            if bands.len() < 2 {
                issues.invalid(
                    issue_path!["bands"],
                    "At least two bands are needed, for the low and high shelves",
                );
            }
            // Each band becomes one biquad, whose parameters have the same names
            // as those of the band.
            for (n, params) in npeq_sections(bands).iter().enumerate() {
                issues.nest_result(
                    issue_path!["bands", n],
                    biquad::validate_config(samplerate, params),
                );
            }
            // The first band becomes the low shelf and the last the high shelf,
            // so the bands have to be listed with rising frequency.
            for (n, pair) in bands.windows(2).enumerate() {
                if pair[1].freq < pair[0].freq {
                    issues.invalid(
                        issue_path!["bands", n + 1, "freq"],
                        "Band frequencies must not decrease along the list",
                    );
                }
            }
        }
        config::BiquadComboParameters::GraphicEqualizer(params) => {
            for (field, freq) in [
                ("freq_min", params.freq_min()),
                ("freq_max", params.freq_max()),
            ] {
                if freq <= 0.0 {
                    issues.invalid(issue_path![field], "Min and max requencies must be > 0");
                } else if freq >= maxfreq as f32 {
                    issues.invalid(
                        issue_path![field],
                        "Min and max frequencies must be < samplerate/2",
                    );
                }
            }
            if params.freq_min() >= params.freq_max() {
                issues.invalid(
                    issue_path!["freq_max"],
                    "Min frequency must be lower than max frequency",
                );
            }
            for (n, gain) in params.gains.iter().enumerate() {
                if *gain > 40.0 || *gain < -40.0 {
                    issues.invalid(
                        issue_path!["gains", n],
                        "Equalizer gains must be withing +- 40 dB",
                    );
                }
            }
        }
    }
    issues.into_result(())
}
