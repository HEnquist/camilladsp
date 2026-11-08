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

// RACE, recursive ambiophonic crosstalk eliminator

use crate::CamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::config::DelayParameters;
use crate::config::GainParameters;
use crate::config::finite;
use crate::filters::Filter;
use crate::filters::basicfilters::Delay;
use crate::filters::basicfilters::Gain;
use crate::processors::Processor;

/// Parameter validation lives in `camilladsp-schema`.
pub use camilladsp_schema::processors::race::validate_race;

//#[derive(Debug)]
pub struct RACE {
    pub name: String,
    pub channels: usize,
    pub channel_a: usize,
    pub channel_b: usize,
    pub feedback_a: CamillaFloat,
    pub feedback_b: CamillaFloat,
    pub delay_a: Delay,
    pub delay_b: Delay,
    pub gain: Gain,
    pub samplerate: usize,
}

fn delay_config(config: &config::RACEParameters, samplerate: usize) -> DelayParameters {
    // compensate the delay by subtracting one sample period from the delay, clamp at zero
    let sample_period_in_delay_unit = match config.delay_unit() {
        config::DelayUnit::Microseconds => 1000000.0 / samplerate as f64,
        config::DelayUnit::Milliseconds => 1000.0 / samplerate as f64,
        config::DelayUnit::Seconds => 1.0 / samplerate as f64,
        config::DelayUnit::Millimetres => 343.0 * 1000.0 / samplerate as f64,
        config::DelayUnit::Samples => 1.0,
    };
    let compensated_delay = (config.delay.get() - sample_period_in_delay_unit).max(0.0);

    config::DelayParameters {
        // Both operands are finite, so the difference is too.
        delay: finite!(compensated_delay),
        delay_unit: config.delay_unit,
        subsample: config.subsample_delay,
    }
}

fn gain_config(config: &config::RACEParameters) -> GainParameters {
    config::GainParameters {
        // Negating a finite value keeps it finite.
        gain: finite!(-config.attenuation.get()),
        scale: Some(config::GainScale::Decibel),
        inverted: Some(true),
        mute: Some(false),
    }
}

impl RACE {
    /// Creates a RACE processor from a config struct
    pub fn from_config(name: &str, config: config::RACEParameters, samplerate: usize) -> Self {
        let name = name.to_string();
        let channels = config.channels;

        debug!(
            "Creating RACE '{}', channels: {}, channel_a: {}, channel_b: {}, delay: {} {:?}, subsample: {}, attenuation: {}",
            name,
            channels,
            config.channel_a,
            config.channel_b,
            config.delay,
            config.delay_unit(),
            config.subsample_delay(),
            config.attenuation
        );
        let delayconf = delay_config(&config, samplerate);
        let delay_a = Delay::from_config("Delay A", samplerate, delayconf.clone());
        let delay_b = Delay::from_config("Delay B", samplerate, delayconf);

        let gainconfig = gain_config(&config);
        let gain = Gain::from_config("Gain", gainconfig);

        // sort channel numbers
        let channel_a = config.channel_a.min(config.channel_b);
        let channel_b = config.channel_a.max(config.channel_b);

        RACE {
            name,
            channels,
            samplerate,
            channel_a,
            channel_b,
            delay_a,
            delay_b,
            gain,
            feedback_a: 0.0,
            feedback_b: 0.0,
        }
    }
    // Keep block cleanup from preventing coefficient hoisting in the sample loop.
    #[inline(never)]
    fn process_waveforms(
        &mut self,
        channel_a: &mut [CamillaFloat],
        channel_b: &mut [CamillaFloat],
    ) {
        let mut feedback_a = self.feedback_a;
        let mut feedback_b = self.feedback_b;
        for (value_a, value_b) in channel_a.iter_mut().zip(channel_b.iter_mut()) {
            let added_a = *value_a + feedback_b;
            let added_b = *value_b + feedback_a;
            feedback_a = self.delay_a.process_single(added_a);
            feedback_b = self.delay_b.process_single(added_b);
            feedback_a = self.gain.process_single(feedback_a);
            feedback_b = self.gain.process_single(feedback_b);
            *value_a = added_a;
            *value_b = added_b;
        }
        self.feedback_a = feedback_a;
        self.feedback_b = feedback_b;
    }
}

impl Processor for RACE {
    fn name(&self) -> &str {
        &self.name
    }

    /// Apply a RACE processor to an AudioChunk, modifying it in-place.
    fn process_chunk(&mut self, input: &mut AudioChunk) {
        let (first, second) = input.waveforms.split_at_mut(self.channel_b);
        let channel_a = &mut first[self.channel_a];
        let channel_b = &mut second[0];
        if channel_a.is_empty() || channel_b.is_empty() {
            return;
        }
        let frames = channel_a.len().min(channel_b.len());
        self.process_waveforms(channel_a, channel_b);
        if self.feedback_a.is_subnormal() {
            self.feedback_a = 0.0;
        }
        if self.feedback_b.is_subnormal() {
            self.feedback_b = 0.0;
        }
        self.delay_a.flush_subnormals(frames);
        self.delay_b.flush_subnormals(frames);
    }

    fn update_parameters(&mut self, config: config::Processor) {
        if let config::Processor::RACE {
            parameters: config, ..
        } = config
        {
            self.channels = config.channels;

            let delayparams = delay_config(&config, self.samplerate);
            let delayconf = config::Filter::Delay {
                parameters: delayparams,
                description: None,
            };
            self.delay_a.update_parameters(delayconf.clone());
            self.delay_b.update_parameters(delayconf);

            let gainparams = gain_config(&config);
            let gainconfig = config::Filter::Gain {
                description: None,
                parameters: gainparams,
            };
            self.gain.update_parameters(gainconfig);

            // sort channel numbers
            self.channel_a = config.channel_a.min(config.channel_b);
            self.channel_b = config.channel_a.max(config.channel_b);

            debug!(
                "Updating RACE '{}', channels: {}, channel_a: {}, channel_b: {}, delay: {} {:?}, subsample: {}, attenuation: {}",
                self.name,
                self.channels,
                config.channel_a,
                config.channel_b,
                config.delay,
                config.delay_unit(),
                config.subsample_delay(),
                config.attenuation
            );
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RACE;
    use crate::audiochunk::AudioChunk;
    use crate::config::{DelayUnit, RACEParameters, finite};
    use crate::processors::Processor;

    #[test]
    fn impulse_decay_reaches_silence() {
        for (frames, delay, subsample) in [
            (128, 5.0, false),
            (128, 4.8, true),
            (16, 65.0, false),
            (16, 65.8, true),
        ] {
            let mut race = RACE::from_config(
                "race",
                RACEParameters {
                    channels: 4,
                    channel_a: 3,
                    channel_b: 1,
                    delay: finite!(delay),
                    delay_unit: DelayUnit::Samples,
                    subsample_delay: Some(subsample),
                    attenuation: finite!(3.0),
                },
                48000,
            );
            let mut chunk = AudioChunk::new(vec![vec![0.0; frames]; 4], 0.5, -0.25, frames, frames);
            chunk.waveforms[1][0] = 0.5;
            chunk.waveforms[3][0] = -0.25;
            race.process_chunk(&mut chunk);
            assert_eq!(chunk.waveforms[1][0], 0.5);
            assert_eq!(chunk.waveforms[3][0], -0.25);
            for _ in 0..(4096 * delay.ceil() as usize).div_ceil(frames) {
                chunk.waveforms[1].fill(0.0);
                chunk.waveforms[3].fill(0.0);
                race.process_chunk(&mut chunk);
            }
            assert!(
                chunk
                    .waveforms
                    .iter()
                    .flatten()
                    .all(|&sample| sample == 0.0)
            );
            assert_eq!(race.feedback_a, 0.0);
            assert_eq!(race.feedback_b, 0.0);
        }
    }
}
