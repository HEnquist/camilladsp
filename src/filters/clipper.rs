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

use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;
use crate::config;
use crate::filters::Filter;
use crate::utils::decibels::db_to_linear;

const CUBEFACTOR: CamillaFloat = 1.0 / 6.75; // = 1 / (2 * 1.5^3)

#[derive(Clone, Debug)]
pub struct Clipper {
    pub name: String,
    pub soft_clip: bool,
    pub clip_limit: CamillaFloat,
}

impl Clipper {
    /// Creates a Clipper from a config struct
    pub fn from_config(name: &str, config: config::ClipperParameters) -> Self {
        let clip_limit = db_to_linear(config.clip_limit).to_camilla_float();

        debug!(
            "Creating clipper '{}', soft_clip: {}, clip_limit dB: {}, linear: {}",
            name,
            config.soft_clip(),
            config.clip_limit,
            clip_limit
        );

        Clipper {
            name: name.to_string(),
            soft_clip: config.soft_clip(),
            clip_limit,
        }
    }

    fn apply_soft_clip(&self, input: &mut [CamillaFloat]) {
        for val in input.iter_mut() {
            let mut scaled = *val / self.clip_limit;
            scaled = scaled.clamp(-1.5, 1.5);
            scaled -= CUBEFACTOR * scaled.powi(3);
            *val = scaled * self.clip_limit;
        }
    }

    fn apply_hard_clip(&self, input: &mut [CamillaFloat]) {
        for val in input.iter_mut() {
            *val = val.clamp(-self.clip_limit, self.clip_limit);
        }
    }

    pub fn apply_clip(&self, input: &mut [CamillaFloat]) {
        if self.soft_clip {
            self.apply_soft_clip(input);
        } else {
            self.apply_hard_clip(input);
        }
    }
}

impl Filter for Clipper {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) -> Res<()> {
        self.apply_clip(waveform);
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Filter) {
        if let config::Filter::Clipper {
            parameters: config, ..
        } = config
        {
            let clip_limit = db_to_linear(config.clip_limit).to_camilla_float();

            self.soft_clip = config.soft_clip();
            self.clip_limit = clip_limit;
            debug!(
                "Updated clipper '{}', soft_clip: {}, clip_limit dB: {}, linear: {}",
                self.name,
                config.soft_clip(),
                config.clip_limit,
                clip_limit
            );
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the clipper config, always return ok to allow any config.
pub fn validate_config(_config: &config::ClipperParameters) -> Res<()> {
    Ok(())
}
