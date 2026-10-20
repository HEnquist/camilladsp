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

//! Fader levels, advanced once per chunk by the pipeline.
//!
//! [`Faders`](crate::fader::Faders) owns the ramp state of every fader. The pipeline calls
//! [`Faders::prepare_chunk`](crate::fader::Faders::prepare_chunk) before any step runs, and that
//! publishes the gain for the chunk to [`FaderLevels`](crate::fader::FaderLevels). The `Volume`
//! and `Loudness` filters only read from there, so every channel sees the same level for the same
//! chunk, whatever order the channels run in.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::CamillaFloat;
use crate::ProcessingParameters;
use crate::ToCamillaFloat;
use crate::config;
use crate::utils::decibels::db_to_linear;

/// Collecting the fader settings from a config, and checking them, lives in `camilladsp-schema`.
pub use camilladsp_schema::fader::{
    FaderSettings, UNUSED_AUX_FADER, fader_settings, validate_fader_settings,
};

const NUM_FADERS: usize = ProcessingParameters::NUM_FADERS;

/// The level in dB that a mute ramps towards, before the gain is set to zero.
pub const MUTE_LEVEL_DB: f32 = -100.0;

/// The gain a fader applies during one chunk.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FaderGain {
    /// The same linear gain for every sample.
    Constant(CamillaFloat),
    /// A ramp laid out in dB, from `start_db` at the first sample towards
    /// `end_db`, which is reached at the first sample of the next chunk.
    Ramp { start_db: f32, end_db: f32 },
}

#[derive(Debug, Default)]
struct FaderLevel {
    start_db: AtomicU32,
    end_db: AtomicU32,
    gain: AtomicU64,
    ramping: AtomicBool,
}

/// The per chunk fader gains, shared between the pipeline and its filters.
///
/// Only [`Faders`] writes, and only from the pipeline thread before any
/// pipeline step runs. The filters read while the steps run, on the pipeline
/// thread or on the filter pool, which rayon orders after the write. Relaxed
/// atomics are therefore enough, and reading never blocks.
#[derive(Debug, Default)]
pub struct FaderLevels {
    faders: [FaderLevel; NUM_FADERS],
}

impl FaderLevels {
    /// The gain `fader` applies during the current chunk.
    pub fn gain(&self, fader: usize) -> FaderGain {
        let level = &self.faders[fader];
        if level.ramping.load(Ordering::Relaxed) {
            FaderGain::Ramp {
                start_db: f32::from_bits(level.start_db.load(Ordering::Relaxed)),
                end_db: f32::from_bits(level.end_db.load(Ordering::Relaxed)),
            }
        } else {
            FaderGain::Constant(
                f64::from_bits(level.gain.load(Ordering::Relaxed)).to_camilla_float(),
            )
        }
    }

    /// The level in dB of `fader` at the end of the current chunk.
    pub fn level_db(&self, fader: usize) -> f32 {
        f32::from_bits(self.faders[fader].end_db.load(Ordering::Relaxed))
    }

    fn set_constant(&self, fader: usize, level_db: f32, gain: f64) {
        let level = &self.faders[fader];
        level.start_db.store(level_db.to_bits(), Ordering::Relaxed);
        level.end_db.store(level_db.to_bits(), Ordering::Relaxed);
        level.gain.store(gain.to_bits(), Ordering::Relaxed);
        level.ramping.store(false, Ordering::Relaxed);
    }

    fn set_ramp(&self, fader: usize, start_db: f32, end_db: f32) {
        let level = &self.faders[fader];
        level.start_db.store(start_db.to_bits(), Ordering::Relaxed);
        level.end_db.store(end_db.to_bits(), Ordering::Relaxed);
        level.ramping.store(true, Ordering::Relaxed);
    }
}

fn ramp_time_in_chunks(ramp_time_ms: f32, chunksize: usize, samplerate: usize) -> usize {
    (ramp_time_ms / (1000.0 * chunksize as f32 / samplerate as f32)).round() as usize
}

#[derive(Debug)]
struct FaderRamp {
    ramp_chunks: usize,
    limit: f32,
    /// The target in dB, already limited.
    target_db: f32,
    mute: bool,
    /// The level in dB at the start of the next chunk.
    current_db: f32,
    /// The level in dB at the start of the current ramp.
    ramp_start: f32,
    /// Which chunk of the ramp comes next, counting from 1. Zero when not ramping.
    ramp_step: usize,
}

impl FaderRamp {
    fn settled_level(&self) -> f32 {
        if self.mute {
            MUTE_LEVEL_DB
        } else {
            self.target_db
        }
    }

    fn settled_gain(&self) -> f64 {
        if self.mute {
            0.0
        } else {
            db_to_linear(self.target_db as f64)
        }
    }
}

/// The ramp state of all faders, owned by the pipeline.
pub struct Faders {
    faders: [FaderRamp; NUM_FADERS],
    levels: Arc<FaderLevels>,
    processing_params: Arc<ProcessingParameters>,
    chunksize: usize,
    samplerate: usize,
    /// Value of the shared pause counter at the previous chunk, used to detect
    /// whether audio flow was interrupted since then.
    last_pause_count: u64,
}

impl Faders {
    /// Start every fader at its current shared level, with no ramp running.
    pub fn new(
        settings: [FaderSettings; NUM_FADERS],
        processing_params: Arc<ProcessingParameters>,
        chunksize: usize,
        samplerate: usize,
    ) -> Self {
        let faders = std::array::from_fn(|fader| {
            let limit = settings[fader].limit;
            let target_db = processing_params.current_volume(fader).min(limit);
            let mute = processing_params.is_mute(fader);
            let mut ramp = FaderRamp {
                ramp_chunks: ramp_time_in_chunks(
                    settings[fader].ramp_time_ms,
                    chunksize,
                    samplerate,
                ),
                limit,
                target_db,
                mute,
                current_db: 0.0,
                ramp_start: 0.0,
                ramp_step: 0,
            };
            ramp.current_db = ramp.settled_level();
            ramp
        });
        // Start in sync with the shared counter, so the first chunk is not
        // mistaken for a resume after a pause.
        let last_pause_count = processing_params.pause_count();
        let faders = Faders {
            faders,
            levels: Arc::new(FaderLevels::default()),
            processing_params,
            chunksize,
            samplerate,
            last_pause_count,
        };
        // Publish the starting levels, so filters built before the first chunk
        // read valid values.
        for (idx, fader) in faders.faders.iter().enumerate() {
            faders
                .levels
                .set_constant(idx, fader.current_db, fader.settled_gain());
        }
        faders
    }

    pub fn from_config(
        conf: &config::Configuration,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        Self::new(
            fader_settings(conf),
            processing_params,
            conf.devices.chunksize(),
            conf.devices.samplerate(),
        )
    }

    /// The levels for the filters to read.
    pub fn levels(&self) -> Arc<FaderLevels> {
        self.levels.clone()
    }

    /// Apply changed ramp times and limits.
    pub fn update_parameters(&mut self, conf: &config::Configuration) {
        let settings = fader_settings(conf);
        for (fader, settings) in self.faders.iter_mut().zip(settings) {
            let ramp_chunks =
                ramp_time_in_chunks(settings.ramp_time_ms, self.chunksize, self.samplerate);
            if ramp_chunks != fader.ramp_chunks && fader.ramp_step > 0 {
                // Carry on from where the ramp is now, at the new speed.
                if ramp_chunks > 0 {
                    fader.ramp_start = fader.current_db;
                    fader.ramp_step = 1;
                } else {
                    fader.current_db = fader.settled_level();
                    fader.ramp_step = 0;
                }
            }
            fader.ramp_chunks = ramp_chunks;
            fader.limit = settings.limit;
            // The next chunk sees the target above the new limit, and ramps
            // down to it from here.
            if fader.current_db > fader.limit {
                fader.current_db = fader.limit;
            }
        }
    }

    /// Advance every fader by one chunk and publish the gains for it. Call
    /// once per chunk, before any filter processes it.
    pub fn prepare_chunk(&mut self) {
        // Did audio flow stop between the previous chunk and this one? If so,
        // any volume change seen now was made while paused, and ramping it
        // would fade in from a level that is no longer relevant.
        let pause_count = self.processing_params.pause_count();
        let resumed_after_pause = pause_count != self.last_pause_count;
        self.last_pause_count = pause_count;

        for (idx, fader) in self.faders.iter_mut().enumerate() {
            let shared_mute = self.processing_params.is_mute(idx);
            let target_db = self.processing_params.target_volume(idx).min(fader.limit);

            if (target_db - fader.target_db).abs() > 0.01 || fader.mute != shared_mute {
                fader.target_db = target_db;
                fader.mute = shared_mute;
                if fader.ramp_chunks > 0 && !resumed_after_pause {
                    trace!(
                        "fader {idx}: starting ramp {} -> {}, mute: {}",
                        fader.current_db, target_db, shared_mute
                    );
                    fader.ramp_start = fader.current_db;
                    fader.ramp_step = 1;
                } else {
                    trace!(
                        "fader {idx}: switch without ramp {} -> {}, mute: {}",
                        fader.current_db, target_db, shared_mute
                    );
                    fader.current_db = fader.settled_level();
                    fader.ramp_step = 0;
                }
            }

            if fader.ramp_step == 0 {
                self.levels
                    .set_constant(idx, fader.current_db, fader.settled_gain());
            } else {
                // The ramp is laid out in dB, at the f32 precision the levels
                // are kept in.
                let range = (fader.settled_level() - fader.ramp_start) / fader.ramp_chunks as f32;
                let start_db = fader.ramp_start + range * (fader.ramp_step - 1) as f32;
                // The last step lands exactly on the settled level, so that the
                // constant gain that follows continues without a step.
                let end_db = if fader.ramp_step == fader.ramp_chunks {
                    fader.settled_level()
                } else {
                    fader.ramp_start + range * fader.ramp_step as f32
                };
                trace!("fader {idx}: ramp step {}", fader.ramp_step);
                self.levels.set_ramp(idx, start_db, end_db);
                fader.current_db = end_db;
                fader.ramp_step += 1;
                if fader.ramp_step > fader.ramp_chunks {
                    fader.ramp_step = 0;
                }
            }
            self.processing_params
                .set_current_volume(idx, fader.current_db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::Filter;
    use crate::filters::basicfilters::Volume;

    const CHUNKSIZE: usize = 64;
    const SAMPLERATE: usize = 44100;

    fn faders(params: &Arc<ProcessingParameters>, ramp_chunks: f32) -> Faders {
        let ramp_time_ms = 1000.0 * CHUNKSIZE as f32 / SAMPLERATE as f32 * ramp_chunks;
        let settings = [FaderSettings {
            ramp_time_ms,
            limit: 50.0,
        }; NUM_FADERS];
        Faders::new(settings, params.clone(), CHUNKSIZE, SAMPLERATE)
    }

    /// A change landing while the channels of a chunk are being processed must
    /// not reach only some of them. Every channel applies the gain published at
    /// the start of the chunk.
    #[test]
    fn all_channels_see_the_same_chunk() {
        let params = Arc::new(ProcessingParameters::default());
        let mut faders = faders(&params, 2.0);
        let mut left = Volume::new("left", 1, CHUNKSIZE, faders.levels());
        let mut right = Volume::new("right", 1, CHUNKSIZE, faders.levels());

        faders.prepare_chunk();
        let mut left_wf = vec![1.0 as CamillaFloat; CHUNKSIZE];
        left.process_waveform(&mut left_wf);
        params.set_target_volume(1, -20.0);
        let mut right_wf = vec![1.0 as CamillaFloat; CHUNKSIZE];
        right.process_waveform(&mut right_wf);
        assert_eq!(left_wf, right_wf);

        // The next chunk starts the ramp, on both channels alike.
        faders.prepare_chunk();
        let mut left_wf = vec![1.0 as CamillaFloat; CHUNKSIZE];
        left.process_waveform(&mut left_wf);
        let mut right_wf = vec![1.0 as CamillaFloat; CHUNKSIZE];
        right.process_waveform(&mut right_wf);
        assert_eq!(left_wf, right_wf);
        assert!(left_wf[0] > left_wf[CHUNKSIZE - 1]);
    }

    /// A ramp ends exactly on the target, and the level follows it chunk by chunk.
    #[test]
    fn ramp_levels_step_to_the_target() {
        let params = Arc::new(ProcessingParameters::default());
        let mut faders = faders(&params, 2.0);
        let levels = faders.levels();

        params.set_target_volume(0, -20.0);
        faders.prepare_chunk();
        assert_eq!(
            levels.gain(0),
            FaderGain::Ramp {
                start_db: 0.0,
                end_db: -10.0
            }
        );
        faders.prepare_chunk();
        assert_eq!(
            levels.gain(0),
            FaderGain::Ramp {
                start_db: -10.0,
                end_db: -20.0
            }
        );
        assert_eq!(params.current_volume(0), -20.0);
        faders.prepare_chunk();
        assert!(matches!(levels.gain(0), FaderGain::Constant(_)));
        assert_eq!(levels.level_db(0), -20.0);
    }

    /// A mute applied without a ramp reports the mute level, the same as a
    /// ramped mute ends at.
    #[test]
    fn unramped_mute_reports_mute_level() {
        let params = Arc::new(ProcessingParameters::default());
        let mut faders = faders(&params, 0.0);
        let levels = faders.levels();

        params.set_mute(2, true);
        faders.prepare_chunk();
        assert_eq!(levels.level_db(2), MUTE_LEVEL_DB);
        assert_eq!(levels.gain(2), FaderGain::Constant(0.0));
    }

    /// A fader starting above its limit starts at the limit, without ramping down.
    #[test]
    fn starts_at_the_limit() {
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 20.0);
        params.sync_volumes_to_target();
        let settings = [FaderSettings {
            ramp_time_ms: 100.0,
            limit: 10.0,
        }; NUM_FADERS];
        let mut faders = Faders::new(settings, params.clone(), CHUNKSIZE, SAMPLERATE);
        let levels = faders.levels();
        assert_eq!(levels.level_db(0), 10.0);
        faders.prepare_chunk();
        assert!(matches!(levels.gain(0), FaderGain::Constant(_)));
        assert_eq!(levels.level_db(0), 10.0);
    }

    fn config(filters: &str, names: &str) -> config::Configuration {
        let yaml = format!(
            "
devices:
  samplerate: 44100
  chunksize: 1024
  capture: {{type: Stdin, channels: 2, format: S16_LE}}
  playback: {{type: Stdout, channels: 2, format: S16_LE}}
filters:
{filters}
pipeline:
  - type: Filter
    names: [{names}]
"
        );
        yaml_serde::from_str(&yaml).unwrap()
    }

    #[test]
    fn aux_settings_come_from_the_volume_filter() {
        let conf = config(
            "  v:\n    type: Volume\n    parameters: {fader: Aux2, ramp_time_ms: 100, limit: -3}",
            "v",
        );
        let settings = fader_settings(&conf);
        assert_eq!(
            settings[2],
            FaderSettings {
                ramp_time_ms: 100.0,
                limit: -3.0
            }
        );
        assert_eq!(settings[1], UNUSED_AUX_FADER);
        assert!(validate_fader_settings(&conf).is_ok());
    }

    #[test]
    fn volume_filters_on_one_fader_must_agree() {
        let same = config(
            "  a:\n    type: Volume\n    parameters: {fader: Aux1, limit: 0}\n  \
            b:\n    type: Volume\n    parameters: {fader: Aux1, limit: 0}",
            "a, b",
        );
        assert!(validate_fader_settings(&same).is_ok());

        let different = config(
            "  a:\n    type: Volume\n    parameters: {fader: Aux1, limit: 0}\n  \
            b:\n    type: Volume\n    parameters: {fader: Aux1, limit: -6}",
            "a, b",
        );
        assert!(validate_fader_settings(&different).is_err());
        // The first one wins where the config is used without validation.
        assert_eq!(fader_settings(&different)[1].limit, 0.0);
    }
}
