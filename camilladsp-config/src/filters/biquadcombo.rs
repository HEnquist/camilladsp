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

/// Validate a BiquadCombo convolution config.
pub fn validate_config(samplerate: usize, conf: &config::BiquadComboParameters) -> Res<()> {
    let maxfreq = samplerate as f64 / 2.0;
    match conf {
        config::BiquadComboParameters::LinkwitzRileyHighpass { freq, order }
        | config::BiquadComboParameters::LinkwitzRileyLowpass { freq, order } => {
            if *freq <= 0.0 {
                return Err(config::ConfigError::new("Frequency must be > 0").into());
            } else if *freq >= maxfreq {
                return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
            }
            if (*order % 2 > 0) || (*order == 0) {
                return Err(
                    config::ConfigError::new("LR order must be an even non-zero number").into(),
                );
            }
            Ok(())
        }
        config::BiquadComboParameters::ButterworthHighpass { freq, order }
        | config::BiquadComboParameters::ButterworthLowpass { freq, order } => {
            if *freq <= 0.0 {
                return Err(config::ConfigError::new("Frequency must be > 0").into());
            } else if *freq >= maxfreq {
                return Err(config::ConfigError::new("Frequency must be < samplerate/2").into());
            }
            if *order == 0 {
                return Err(
                    config::ConfigError::new("Butterworth order must be larger than zero").into(),
                );
            }
            Ok(())
        }
        config::BiquadComboParameters::Tilt { gain } => {
            if *gain <= -100.0 {
                return Err(config::ConfigError::new("Gain must be > -100").into());
            } else if *gain >= 100.0 {
                return Err(config::ConfigError::new("Gain must be < 100").into());
            }
            Ok(())
        }
        config::BiquadComboParameters::NPointPeq { bands } => {
            if bands.len() < 2 {
                return Err(config::ConfigError::new(
                    "At least two bands are needed, for the low and high shelves",
                )
                .into());
            }
            for params in npeq_sections(bands).iter() {
                biquad::validate_config(samplerate, params)?;
            }
            // The first band becomes the low shelf and the last the high shelf,
            // so the bands have to be listed with rising frequency.
            for pair in bands.windows(2) {
                if pair[1].freq < pair[0].freq {
                    return Err(config::ConfigError::new(
                        "Band frequencies must not decrease along the list",
                    )
                    .into());
                }
            }
            Ok(())
        }
        config::BiquadComboParameters::GraphicEqualizer(params) => {
            if params.freq_min() <= 0.0 || params.freq_max() <= 0.0 {
                return Err(config::ConfigError::new("Min and max requencies must be > 0").into());
            } else if params.freq_min() >= maxfreq as f32 || params.freq_max() >= maxfreq as f32 {
                return Err(config::ConfigError::new(
                    "Min and max frequencies must be < samplerate/2",
                )
                .into());
            }
            if params.freq_min() >= params.freq_max() {
                return Err(config::ConfigError::new(
                    "Min frequency must be lower than max frequency",
                )
                .into());
            }
            for gain in params.gains.iter() {
                if *gain > 40.0 || *gain < -40.0 {
                    return Err(config::ConfigError::new(
                        "Equalizer gains must be withing +- 40 dB",
                    )
                    .into());
                }
            }
            Ok(())
        }
    }
}
