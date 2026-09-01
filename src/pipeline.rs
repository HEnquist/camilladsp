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

use crate::ProcessingParameters;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::filters;
use crate::filters::Filter;
use crate::filters::fftconv::ConvCoeffCache;
use crate::mixer;
use crate::processors;
use crate::processors::Processor;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const LOAD_WARN_CONSECUTIVE_CHUNKS: usize = 10;

/// An ordered chain of filters applied to a single channel.
pub struct FilterGroup {
    channel: usize,
    filters: Vec<Box<dyn Filter + Send>>,
}

impl FilterGroup {
    /// Creates a group of filters to process a chunk.
    pub fn from_config(
        channel: usize,
        names: &[String],
        filter_configs: HashMap<String, config::Filter>,
        waveform_length: usize,
        sample_freq: usize,
        processing_params: Arc<ProcessingParameters>,
        cache: &mut ConvCoeffCache,
    ) -> Self {
        debug!("Build filter group from config");
        let mut filters = Vec::<Box<dyn Filter + Send>>::new();
        for name in names {
            let filter_cfg = filter_configs[name].clone();
            trace!("Create filter {name} with config {filter_cfg:?}");
            let filter: Box<dyn Filter + Send> = match filter_cfg {
                config::Filter::Conv { parameters, .. } => {
                    Box::new(filters::fftconv::FftConv::from_config_cached(
                        name,
                        waveform_length,
                        parameters,
                        cache,
                    ))
                }
                config::Filter::Biquad { parameters, .. } => {
                    Box::new(filters::biquad::Biquad::new(
                        name,
                        sample_freq,
                        filters::biquad::BiquadCoefficients::from_config(sample_freq, parameters),
                    ))
                }
                config::Filter::BiquadCombo { parameters, .. } => Box::new(
                    filters::biquadcombo::BiquadCombo::from_config(name, sample_freq, parameters),
                ),
                config::Filter::Delay { parameters, .. } => Box::new(
                    filters::basicfilters::Delay::from_config(name, sample_freq, parameters),
                ),
                config::Filter::Gain { parameters, .. } => {
                    Box::new(filters::basicfilters::Gain::from_config(name, parameters))
                }
                config::Filter::Volume { parameters, .. } => {
                    Box::new(filters::basicfilters::Volume::from_config(
                        name,
                        parameters,
                        waveform_length,
                        sample_freq,
                        processing_params.clone(),
                    ))
                }
                config::Filter::Loudness { parameters, .. } => {
                    Box::new(filters::loudness::Loudness::from_config(
                        name,
                        parameters,
                        sample_freq,
                        processing_params.clone(),
                    ))
                }
                config::Filter::Dither { parameters, .. } => {
                    Box::new(filters::dither::Dither::from_config(name, parameters))
                }
                config::Filter::DiffEq { parameters, .. } => {
                    Box::new(filters::diffeq::DiffEq::from_config(name, parameters))
                }
                config::Filter::Clipper { parameters, .. } => {
                    Box::new(filters::clipper::Clipper::from_config(name, parameters))
                }
                config::Filter::LookaheadLimiter { parameters, .. } => {
                    Box::new(filters::lookahead_limiter::LookaheadLimiter::from_config(
                        name,
                        parameters,
                        sample_freq,
                        waveform_length,
                    ))
                }
            };
            filters.push(filter);
        }
        FilterGroup { channel, filters }
    }

    /// Hot-reload parameters for any filters whose names appear in `changed`.
    pub fn update_parameters(
        &mut self,
        filterconfigs: HashMap<String, config::Filter>,
        changed: &[String],
        cache: &mut ConvCoeffCache,
    ) {
        for filter in &mut self.filters {
            if changed.iter().any(|n| n == filter.name()) {
                filter.update_parameters_cached(filterconfigs[filter.name()].clone(), cache);
            }
        }
    }

    /// Apply all the filters to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) {
        let waveform = &mut input.waveforms[self.channel];
        if waveform.is_empty() {
            return;
        }
        for filter in &mut self.filters {
            filter.process_waveform(waveform);
        }
    }
}

/// Merged filter groups for all channels that can run in parallel via rayon.
pub struct ParallelFilters {
    filters: Vec<Vec<Box<dyn Filter + Send>>>,
    filter_pool: Arc<rayon::ThreadPool>,
}

impl ParallelFilters {
    /// Hot-reload parameters for any filters whose names appear in `changed`.
    pub fn update_parameters(
        &mut self,
        filterconfigs: HashMap<String, config::Filter>,
        changed: &[String],
        cache: &mut ConvCoeffCache,
    ) {
        for channel_filters in &mut self.filters {
            for filter in channel_filters {
                if changed.iter().any(|n| n == filter.name()) {
                    filter.update_parameters_cached(filterconfigs[filter.name()].clone(), cache);
                }
            }
        }
    }

    /// Apply all the filters to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) {
        self.filter_pool.install(|| {
            self.filters
                .par_iter_mut()
                .zip(input.waveforms.par_iter_mut())
                .filter(|(f, w)| !f.is_empty() && !w.is_empty())
                .for_each(|(f, w)| {
                    for filt in f {
                        filt.process_waveform(w);
                    }
                });
        });
    }
}

/// A Pipeline is made up of a series of PipelineSteps,
/// each one can be a single Mixer or a group of Filters
pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
    ParallelFiltersStep(ParallelFilters),
    ProcessorStep(Box<dyn Processor>),
}

/// The complete processing pipeline: an ordered list of mixer, filter, and processor steps
/// with a master volume applied before the first step.
pub struct Pipeline {
    steps: Vec<PipelineStep>,
    volume: filters::basicfilters::Volume,
    secs_per_chunk: f32,
    processing_params: Arc<ProcessingParameters>,
    overloaded_chunks: usize,
}

impl Pipeline {
    /// Create a new pipeline from a configuration structure.
    ///
    /// `filter_pool` is the thread pool to use for parallel filter processing.
    /// `None` means single-threaded processing.
    pub fn from_config(
        conf: config::Configuration,
        processing_params: Arc<ProcessingParameters>,
        filter_pool: Option<Arc<rayon::ThreadPool>>,
    ) -> Self {
        debug!("Build new pipeline");
        trace!("Pipeline config {:?}", conf.pipeline);
        let mut steps = Vec::<PipelineStep>::new();
        // One cache for the whole build, so channels sharing a convolution
        // filter share its transformed coefficients.
        let mut coeff_cache = ConvCoeffCache::new();
        let mut num_channels = conf.devices.capture.channels();
        for step in conf.pipeline.unwrap_or_default() {
            match step {
                config::PipelineStep::Mixer(step) => {
                    if !step.is_bypassed() {
                        let mixconf = conf.mixers.as_ref().unwrap()[&step.name].clone();
                        num_channels = mixconf.channels.out;
                        debug!(
                            "Add Mixer step with mixer {}, pipeline becomes {} channels wide",
                            step.name, mixconf.channels.out
                        );
                        let mixer = mixer::Mixer::from_config(step.name, mixconf);
                        steps.push(PipelineStep::MixerStep(mixer));
                    }
                }
                config::PipelineStep::Filter(step) => {
                    if !step.is_bypassed() {
                        let channels_iter: Box<dyn Iterator<Item = usize>> = if let Some(channels) =
                            &step.channels
                        {
                            debug!(
                                "Add Filter step with filters {:?} to channels {:?}",
                                step.names, channels
                            );
                            Box::new(channels.iter().copied()) as Box<dyn Iterator<Item = usize>>
                        } else {
                            debug!(
                                "Add Filter step with filters {:?} to all {} channels",
                                step.names, num_channels
                            );
                            Box::new(0..num_channels) as Box<dyn Iterator<Item = usize>>
                        };
                        for channel in channels_iter {
                            let fltgrp = FilterGroup::from_config(
                                channel,
                                &step.names,
                                conf.filters.as_ref().unwrap().clone(),
                                conf.devices.chunksize,
                                conf.devices.samplerate,
                                processing_params.clone(),
                                &mut coeff_cache,
                            );
                            steps.push(PipelineStep::FilterStep(fltgrp));
                        }
                    }
                }
                config::PipelineStep::Processor(step) => {
                    if !step.is_bypassed() {
                        debug!("Add Processor step with processor {}", step.name);
                        let procconf = conf.processors.as_ref().unwrap()[&step.name].clone();
                        let proc = match procconf {
                            config::Processor::Compressor { parameters, .. } => {
                                let comp = processors::compressor::Compressor::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                    conf.devices.chunksize,
                                );
                                Box::new(comp) as Box<dyn Processor>
                            }
                            config::Processor::NoiseGate { parameters, .. } => {
                                let gate = processors::noisegate::NoiseGate::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                    conf.devices.chunksize,
                                );
                                Box::new(gate) as Box<dyn Processor>
                            }
                            config::Processor::LookaheadLimiter { parameters, .. } => {
                                let limiter =
                                    processors::lookahead_limiter::LookaheadLimiter::from_config(
                                        &step.name,
                                        parameters,
                                        conf.devices.samplerate,
                                        conf.devices.chunksize,
                                    );
                                Box::new(limiter) as Box<dyn Processor>
                            }
                            config::Processor::RACE { parameters, .. } => {
                                let race = processors::race::RACE::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                );
                                Box::new(race) as Box<dyn Processor>
                            }
                        };
                        steps.push(PipelineStep::ProcessorStep(proc));
                    }
                }
            }
        }
        let current_volume = processing_params.current_volume(0);
        let mute = processing_params.is_mute(0);
        let volume = filters::basicfilters::Volume::new(
            "default",
            conf.devices.volume_ramp_time_ms(),
            conf.devices.volume_limit(),
            current_volume,
            mute,
            conf.devices.chunksize,
            conf.devices.samplerate,
            processing_params.clone(),
            0,
        );
        let secs_per_chunk = conf.devices.chunksize as f32 / conf.devices.samplerate as f32;
        // When a rayon pool is available, merge the per-channel filter
        // steps into parallel steps that run on it. With no pool the
        // filters run sequentially.
        if let Some(pool) = &filter_pool {
            steps = parallelize_filters(&mut steps, conf.devices.capture.channels(), pool);
        }
        Pipeline {
            steps,
            volume,
            secs_per_chunk,
            processing_params,
            overloaded_chunks: 0,
        }
    }

    /// Hot-reload changed filters, mixers, and processors without rebuilding the pipeline.
    pub fn update_parameters(
        &mut self,
        conf: config::Configuration,
        filters: &[String],
        mixers: &[String],
        processors: &[String],
    ) {
        debug!("Updating parameters");
        // One cache for the whole pass, so a convolution filter used on
        // several channels is read and transformed once. It is dropped at the
        // end of the pass, before any later config can reuse a name.
        let mut coeff_cache = ConvCoeffCache::new();
        for mut step in &mut self.steps {
            match &mut step {
                PipelineStep::MixerStep(mix) => {
                    if mixers.iter().any(|n| n == &mix.name) {
                        mix.update_parameters(conf.mixers.as_ref().unwrap()[&mix.name].clone());
                    }
                }
                PipelineStep::FilterStep(flt) => {
                    flt.update_parameters(
                        conf.filters.as_ref().unwrap().clone(),
                        filters,
                        &mut coeff_cache,
                    );
                }
                PipelineStep::ParallelFiltersStep(flt) => {
                    flt.update_parameters(
                        conf.filters.as_ref().unwrap().clone(),
                        filters,
                        &mut coeff_cache,
                    );
                }
                PipelineStep::ProcessorStep(proc) => {
                    if processors.iter().any(|n| n == proc.name()) {
                        proc.update_parameters(
                            conf.processors.as_ref().unwrap()[proc.name()].clone(),
                        );
                    }
                }
            }
        }
    }

    /// Process an AudioChunk by calling either a MixerStep or a FilterStep
    pub fn process_chunk(&mut self, mut chunk: AudioChunk) -> AudioChunk {
        let start = Instant::now();
        self.volume.process_chunk(&mut chunk);
        for mut step in &mut self.steps {
            match &mut step {
                PipelineStep::MixerStep(mix) => {
                    chunk = mix.process_chunk(chunk);
                }
                PipelineStep::FilterStep(flt) => {
                    flt.process_chunk(&mut chunk);
                }
                PipelineStep::ParallelFiltersStep(flt) => {
                    flt.process_chunk(&mut chunk);
                }
                PipelineStep::ProcessorStep(comp) => {
                    comp.process_chunk(&mut chunk).unwrap();
                }
            }
        }
        let secs_elapsed = start.elapsed().as_secs_f32();
        let load = 100.0 * secs_elapsed / self.secs_per_chunk;
        self.processing_params.set_processing_load(load);
        trace!("Processing load: {load}%");
        if load > 100.0 {
            self.overloaded_chunks += 1;
            if self.overloaded_chunks == LOAD_WARN_CONSECUTIVE_CHUNKS {
                warn!(
                    "Processing load has been above 100% for {} consecutive chunks (current: {load}%)",
                    LOAD_WARN_CONSECUTIVE_CHUNKS
                );
            }
        } else {
            self.overloaded_chunks = 0;
        }
        chunk
    }
}

/// Merge adjacent per-channel filter steps into one chain per channel, so the
/// channels can be run in parallel on the thread pool. Mixers and processors
/// work on all channels at once and break a run.
///
/// This changes the order the filters run in. The configuration gives them
/// step by step, and channel by channel within each step. A merged run instead
/// has each channel running its whole chain, and the channels run at the same
/// time as each other.
///
/// One pairing can tell, and is accepted rather than fixed. `Volume` writes its
/// fader's current level while processing, and `Loudness` reads that level to
/// size its compensation. Put them on the same fader but on different channels
/// and the `Loudness` can read the level one chunk late while the volume is
/// moving. Both on the same channel is not affected, the chain there keeping
/// the order the configuration gave.
///
/// The cost is one chunk of lag on a compensation curve that moves over
/// hundreds of milliseconds, which is not audible. Before giving up the
/// parallelism to preserve the order exactly, check that there is a listener
/// who can hear the difference.
fn parallelize_filters(
    steps: &mut Vec<PipelineStep>,
    nbr_channels: usize,
    pool: &Arc<rayon::ThreadPool>,
) -> Vec<PipelineStep> {
    debug!("Merging filter steps to enable parallel processing");
    let mut new_steps: Vec<PipelineStep> = Vec::new();
    let mut parfilt = None;
    let mut active_channels = nbr_channels;
    for step in steps.drain(..) {
        match step {
            PipelineStep::MixerStep(ref mix) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.push(PipelineStep::ParallelFiltersStep(parfilt.take().unwrap()));
                }
                active_channels = mix.channels_out;
                debug!("Append mixer step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::ProcessorStep(_) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.push(PipelineStep::ParallelFiltersStep(parfilt.take().unwrap()));
                }
                debug!("Append processor step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::ParallelFiltersStep(_) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.push(PipelineStep::ParallelFiltersStep(parfilt.take().unwrap()));
                }
                debug!("Append existing parallel filter step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::FilterStep(mut flt) => {
                if parfilt.is_none() {
                    debug!("Start new parallel filter step");
                    let mut filters = Vec::with_capacity(active_channels);
                    for _ in 0..active_channels {
                        filters.push(Vec::new());
                    }
                    parfilt = Some(ParallelFilters {
                        filters,
                        filter_pool: pool.clone(),
                    });
                }
                if let Some(ref mut f) = parfilt {
                    debug!(
                        "Adding {} filters to channel {} of parallel filter step",
                        flt.filters.len(),
                        flt.channel
                    );
                    f.filters[flt.channel].append(&mut flt.filters);
                }
            }
        }
    }
    if parfilt.is_some() {
        debug!("Append parallel filter step to pipeline");
        new_steps.push(PipelineStep::ParallelFiltersStep(parfilt.take().unwrap()));
    }
    new_steps
}

#[cfg(test)]
mod tests {
    use super::Pipeline;
    use crate::CamillaFloat;
    use crate::ProcessingParameters;
    use crate::audiochunk::AudioChunk;
    use std::sync::Arc;

    // Regression test: volume set before config load must be applied to the very first chunk.
    // Set volume to -100 dB, then load a config,
    // and verify no full-scale burst comes out of the first processed chunk.
    #[test]
    fn volume_preset_before_pipeline_build() {
        const CONFIG: &str = "
devices:
  samplerate: 44100
  chunksize: 1024
  capture:
    type: SignalGenerator
    channels: 2
    signal:
      type: Sine
      freq: 1000
      level: 0.0
  playback:
    type: Stdout
    channels: 2
    format: S16_LE
";
        let conf: crate::config::Configuration = yaml_serde::from_str(CONFIG).unwrap();
        let chunksize = conf.devices.chunksize;
        let channels = conf.devices.capture.channels();

        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, -100.0);
        params.sync_volumes_to_target();

        let mut pipeline = Pipeline::from_config(conf, params, None);

        let waveforms = vec![vec![1.0 as CamillaFloat; chunksize]; channels];
        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, chunksize, chunksize);
        let out = pipeline.process_chunk(chunk);

        let max_val = out
            .waveforms
            .iter()
            .flat_map(|w| w.iter())
            .map(|x| x.abs())
            .fold(0.0_f64 as CamillaFloat, CamillaFloat::max);
        assert!(
            max_val < 1e-3,
            "First chunk after preset volume should be near-silent, got max={max_val}"
        );
    }
}
