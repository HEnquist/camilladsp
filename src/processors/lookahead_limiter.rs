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
use crate::Res;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::filters::Filter;
use crate::filters::basicfilters::Delay;
use crate::filters::lookahead_limiter::{LookaheadGain, limiter_parameters, validate_times};
use crate::processors::Processor;

/// Multichannel lookahead limiter.
///
/// Peaks are detected on the monitored channels, and the resulting gain envelope is
/// applied to the processed channels. Looking ahead means the output is delayed by
/// the attack time. By default all channels are delayed, also the ones that are not
/// processed, so that the channels stay time aligned.
pub struct LookaheadLimiter {
    pub name: String,
    pub channels: usize,
    pub monitor_channels: Vec<usize>,
    pub process_channels: Vec<usize>,
    pub delay_processed_only: bool,
    pub samplerate: usize,
    gain: LookaheadGain,
    /// Lookahead delay for each channel.
    delays: Vec<Delay>,
    /// Peak detection signal of the current chunk.
    scratch: Vec<PrcFmt>,
}

/// Expand an empty channel list to all channels.
fn all_channels_if_empty(mut channels: Vec<usize>, nbr_channels: usize) -> Vec<usize> {
    if channels.is_empty() {
        for n in 0..nbr_channels {
            channels.push(n);
        }
    }
    channels
}

/// Build one lookahead delay per channel.
fn make_delays(nbr_channels: usize, attack_samples: usize, samplerate: usize) -> Vec<Delay> {
    (0..nbr_channels)
        .map(|ch| {
            Delay::new(
                &format!("Lookahead delay {ch}"),
                samplerate,
                attack_samples as PrcFmt,
                false,
            )
        })
        .collect()
}

impl LookaheadLimiter {
    /// Creates a LookaheadLimiter processor from a config struct
    pub fn from_config(
        name: &str,
        config: config::LookaheadLimiterProcessorParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let name = name.to_string();
        let channels = config.channels;
        let monitor_channels = all_channels_if_empty(config.monitor_channels(), channels);
        let process_channels = all_channels_if_empty(config.process_channels(), channels);
        let (limit, attack_samples, release_coeff) = limiter_parameters(
            config.limit,
            config.attack,
            config.attack_unit,
            config.release,
            config.release_unit,
            samplerate,
        );

        debug!(
            "Creating lookahead limiter '{}', channels: {}, monitor_channels: {:?}, process_channels: {:?}, delay_processed_only: {}, limit dB: {}, linear: {}, attack/lookahead: {} samples, release coefficient: {}",
            name,
            channels,
            monitor_channels,
            process_channels,
            config.delay_processed_only(),
            config.limit,
            limit,
            attack_samples,
            release_coeff
        );

        LookaheadLimiter {
            name,
            channels,
            monitor_channels,
            process_channels,
            delay_processed_only: config.delay_processed_only(),
            samplerate,
            gain: LookaheadGain::new(limit, attack_samples, release_coeff, samplerate, chunksize),
            delays: make_delays(channels, attack_samples, samplerate),
            scratch: vec![0.0; chunksize],
        }
    }

    /// Find the largest amplitude of all monitored channels, store result in self.scratch
    fn detect_peaks(&mut self, input: &AudioChunk) {
        let ch = self.monitor_channels[0];
        for (peak, val) in self.scratch.iter_mut().zip(input.waveforms[ch].iter()) {
            *peak = val.abs();
        }
        for ch in self.monitor_channels.iter().skip(1) {
            for (peak, val) in self.scratch.iter_mut().zip(input.waveforms[*ch].iter()) {
                *peak = peak.max(val.abs());
            }
        }
    }

    fn apply_gain(gain: &[PrcFmt], input: &mut [PrcFmt]) {
        for (val, gain) in input.iter_mut().zip(gain.iter()) {
            *val *= gain;
        }
    }
}

impl Processor for LookaheadLimiter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Apply a LookaheadLimiter to an AudioChunk, modifying it in-place.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        self.detect_peaks(input);
        self.gain.process_detection(&self.scratch);
        // Unless disabled, delay the unprocessed channels too, to keep all channels time aligned.
        for (ch, delay) in self.delays.iter_mut().enumerate() {
            if !self.delay_processed_only || self.process_channels.contains(&ch) {
                delay.process_waveform(&mut input.waveforms[ch])?;
            }
        }
        for ch in self.process_channels.iter() {
            Self::apply_gain(self.gain.envelope(), &mut input.waveforms[*ch]);
        }
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Processor) {
        if let config::Processor::LookaheadLimiter {
            parameters: config, ..
        } = config
        {
            let channels = config.channels;
            let samplerate = self.samplerate;
            let (limit, attack_samples, release_coeff) = limiter_parameters(
                config.limit,
                config.attack,
                config.attack_unit,
                config.release,
                config.release_unit,
                samplerate,
            );

            // Rebuilding the delays clears them, so only do it when the length changed.
            if attack_samples != self.gain.attack_samples || channels != self.channels {
                self.delays = make_delays(channels, attack_samples, samplerate);
            }
            self.channels = channels;
            self.monitor_channels = all_channels_if_empty(config.monitor_channels(), channels);
            self.process_channels = all_channels_if_empty(config.process_channels(), channels);
            self.delay_processed_only = config.delay_processed_only();
            self.gain
                .set_parameters(limit, attack_samples, release_coeff);

            debug!(
                "Updated lookahead limiter '{}', monitor_channels: {:?}, process_channels: {:?}, delay_processed_only: {}, limit dB: {}, linear: {}, attack/lookahead: {} samples, release coefficient: {}",
                self.name,
                self.monitor_channels,
                self.process_channels,
                self.delay_processed_only,
                config.limit,
                limit,
                attack_samples,
                release_coeff
            );
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the lookahead limiter config, to give a helpful message intead of a panic.
pub fn validate_lookahead_limiter(
    config: &config::LookaheadLimiterProcessorParameters,
    samplerate: usize,
) -> Res<()> {
    let channels = config.channels;
    validate_times(
        config.attack,
        config.attack_unit,
        config.release,
        samplerate,
    )?;
    for ch in config.monitor_channels().iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid monitor channel: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    for ch in config.process_channels().iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid channel to process: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TimeUnit;

    fn params(
        monitor_channels: Option<Vec<usize>>,
        process_channels: Option<Vec<usize>>,
        attack: PrcFmt,
        release: PrcFmt,
    ) -> config::LookaheadLimiterProcessorParameters {
        config::LookaheadLimiterProcessorParameters {
            channels: 2,
            monitor_channels,
            process_channels,
            limit: 0.0,
            attack,
            attack_unit: TimeUnit::Samples,
            release,
            release_unit: TimeUnit::Samples,
            delay_processed_only: None,
        }
    }

    fn chunk(waveforms: Vec<Vec<PrcFmt>>) -> AudioChunk {
        let frames = waveforms[0].len();
        AudioChunk::new(waveforms, 1.0, -1.0, frames, frames)
    }

    fn assert_close(left: &[PrcFmt], right: &[PrcFmt], epsilon: PrcFmt) {
        assert_eq!(left.len(), right.len());
        for (i, (&l, &r)) in left.iter().zip(right.iter()).enumerate() {
            assert!(
                (l - r).abs() <= epsilon,
                "Mismatch at index {i}: left={l}, right={r}\nleft:  {left:?}\nright: {right:?}"
            );
        }
    }

    /// With a single channel the processor must match the filter exactly.
    #[test]
    fn test_matches_filter() {
        let samplerate = 48000;
        let waveform: Vec<PrcFmt> = vec![
            1.0, 1.0, 1.0, 1.0, 1.0, 2.0, -2.0, 1.0, 1.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
            1.0, 1.0,
        ];
        let chunksize = waveform.len();

        let mut processor = LookaheadLimiter::from_config(
            "test",
            config::LookaheadLimiterProcessorParameters {
                channels: 1,
                monitor_channels: None,
                process_channels: None,
                limit: 0.0,
                attack: 4.0,
                attack_unit: TimeUnit::Samples,
                release: 1.0 / std::f64::consts::LN_2 as PrcFmt,
                release_unit: TimeUnit::Samples,
                delay_processed_only: None,
            },
            samplerate,
            chunksize,
        );
        let mut filter = crate::filters::lookahead_limiter::LookaheadLimiter::from_config(
            "test",
            config::LookaheadLimiterParameters {
                limit: 0.0,
                attack: 4.0,
                attack_unit: TimeUnit::Samples,
                release: 1.0 / std::f64::consts::LN_2 as PrcFmt,
                release_unit: TimeUnit::Samples,
            },
            samplerate,
            chunksize,
        );

        let mut processor_chunk = chunk(vec![waveform.clone()]);
        let mut filter_waveform = waveform;

        processor.process_chunk(&mut processor_chunk).unwrap();
        filter.process_waveform(&mut filter_waveform).unwrap();

        assert_close(&processor_chunk.waveforms[0], &filter_waveform, 1e-12);
    }

    /// The peak of the loudest monitored channel controls the gain of all processed channels.
    #[test]
    fn test_peak_detection_uses_loudest_channel() {
        let mut limiter =
            LookaheadLimiter::from_config("test", params(None, None, 0.0, 0.0), 48000, 4);
        let mut input = chunk(vec![vec![0.25, 0.25, 0.25, 0.25], vec![0.5, 1.0, 2.0, 4.0]]);
        limiter.process_chunk(&mut input).unwrap();

        // Channel 1 is limited to 1.0, channel 0 gets the same gain reduction.
        assert_close(&input.waveforms[1], &[0.5, 1.0, 1.0, 1.0], 1e-12);
        assert_close(&input.waveforms[0], &[0.25, 0.25, 0.125, 0.0625], 1e-12);
    }

    /// An unmonitored channel does not affect the gain, but is still limited.
    #[test]
    fn test_monitor_subset() {
        let mut limiter =
            LookaheadLimiter::from_config("test", params(Some(vec![0]), None, 0.0, 0.0), 48000, 2);
        let mut input = chunk(vec![vec![1.0, 2.0], vec![4.0, 1.0]]);
        limiter.process_chunk(&mut input).unwrap();

        assert_close(&input.waveforms[0], &[1.0, 1.0], 1e-12);
        assert_close(&input.waveforms[1], &[4.0, 0.5], 1e-12);
    }

    /// Channels that are not processed pass through unchanged, but delayed.
    #[test]
    fn test_unprocessed_channel_is_delayed_only() {
        let mut limiter = LookaheadLimiter::from_config(
            "test",
            params(Some(vec![0]), Some(vec![0]), 2.0, 0.0),
            48000,
            4,
        );
        let mut input = chunk(vec![vec![0.0, 0.0, 2.0, 0.0], vec![1.0, 2.0, 3.0, 4.0]]);
        limiter.process_chunk(&mut input).unwrap();

        // Channel 1 is only delayed by the two samples of lookahead.
        assert_close(&input.waveforms[1], &[0.0, 0.0, 1.0, 2.0], 1e-12);
        // Channel 0 is delayed by the same amount, and the peak is ramped down to the limit.
        assert_close(&input.waveforms[0], &[0.0, 0.0, 0.0, 0.0], 1e-12);

        let mut input = chunk(vec![vec![0.0, 0.0, 0.0, 0.0], vec![5.0, 6.0, 7.0, 8.0]]);
        limiter.process_chunk(&mut input).unwrap();
        assert_close(&input.waveforms[1], &[3.0, 4.0, 5.0, 6.0], 1e-12);
        assert_close(&input.waveforms[0], &[1.0, 0.0, 0.0, 0.0], 1e-12);
    }

    /// With `delay_processed_only`, channels that are not processed are left untouched.
    #[test]
    fn test_delay_processed_only() {
        let mut config = params(Some(vec![0]), Some(vec![0]), 2.0, 0.0);
        config.delay_processed_only = Some(true);
        let mut limiter = LookaheadLimiter::from_config("test", config, 48000, 4);

        let mut input = chunk(vec![vec![0.0, 0.0, 2.0, 0.0], vec![1.0, 2.0, 3.0, 4.0]]);
        limiter.process_chunk(&mut input).unwrap();

        // Channel 1 passes through without any delay.
        assert_close(&input.waveforms[1], &[1.0, 2.0, 3.0, 4.0], 1e-12);
        // Channel 0 is still delayed by the two samples of lookahead.
        assert_close(&input.waveforms[0], &[0.0, 0.0, 0.0, 0.0], 1e-12);
    }

    #[test]
    fn test_validate() {
        assert!(validate_lookahead_limiter(&params(None, None, 4.0, 4.0), 48000).is_ok());
        assert!(
            validate_lookahead_limiter(&params(Some(vec![0, 2]), None, 4.0, 4.0), 48000).is_err()
        );
        assert!(validate_lookahead_limiter(&params(None, Some(vec![5]), 4.0, 4.0), 48000).is_err());
        assert!(validate_lookahead_limiter(&params(None, None, 48001.0, 4.0), 48000).is_err());
        assert!(validate_lookahead_limiter(&params(None, None, -1.0, 4.0), 48000).is_err());
    }
}
