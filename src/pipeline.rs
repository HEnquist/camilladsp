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
use crate::ProcessingParameters;
use crate::Res;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::filters;
use crate::filters::Filter;
use crate::filters::biquad::MAX_INTERLEAVE;
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
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        if !input.waveforms[self.channel].is_empty() {
            for filter in &mut self.filters {
                filter.process_waveform(&mut input.waveforms[self.channel])?;
            }
        }
        Ok(())
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
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        self.filter_pool.install(|| {
            self.filters
                .par_iter_mut()
                .zip(input.waveforms.par_iter_mut())
                .filter(|(f, w)| !f.is_empty() && !w.is_empty())
                .for_each(|(f, w)| {
                    for filt in f {
                        let _ = filt.process_waveform(w);
                    }
                });
        });
        Ok(())
    }
}

/// Filter chains for every channel of a step, indexed by channel.
type ChannelChains = Vec<Vec<Box<dyn Filter + Send>>>;

/// Filter groups for channels whose chains are entirely biquad-based, run with
/// several channels interleaved.
///
/// A biquad stalls on its own feedback path, so processing channels one at a
/// time leaves the core idle. Channels are batched into groups of at most
/// [`MAX_INTERLEAVE`] and the groups run in sequence on the calling thread.
///
/// This is the single-threaded path. Interleaving already extracts the
/// parallelism the biquads had to offer, so putting a thread pool on top of it
/// only costs dispatch: a four channel biquad pipeline measured 92 us here
/// against 138 us on the pool. Configurations that enable multithreading keep
/// the per-channel [`ParallelFilters`] path instead, which is what heavy FIR
/// convolution wants.
///
/// Built only when every channel has an equal-length, fully biquad-based
/// chain; anything else falls back to one [`FilterGroup`] per channel.
pub struct InterleavedFilters {
    filters: ChannelChains,
}

impl InterleavedFilters {
    /// Returns `Some` if every channel chain is the same length and made up
    /// entirely of filters that expose a biquad cascade.
    ///
    /// The all-biquad requirement is deliberate. Grouping channels trades
    /// per-channel rayon parallelism for interleaving, which is a win for
    /// biquads but a loss for an expensive filter like a convolution, since
    /// four of those would then run serially in one task instead of in
    /// parallel. Mixed chains therefore stay on [`ParallelFilters`].
    fn try_new(mut filters: ChannelChains) -> Result<Self, ChannelChains> {
        let uniform_depth = filters
            .iter()
            .map(|chain| chain.len())
            .collect::<std::collections::HashSet<_>>()
            .len()
            <= 1;
        let all_biquads = filters
            .iter_mut()
            .flat_map(|chain| chain.iter_mut())
            .all(|f| f.biquad_cascade().is_some());
        let worth_it = filters.len() > 1 && filters.first().is_some_and(|c| !c.is_empty());
        if uniform_depth && all_biquads && worth_it {
            Ok(InterleavedFilters { filters })
        } else {
            Err(filters)
        }
    }

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
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        for (chains, waves) in self
            .filters
            .chunks_mut(MAX_INTERLEAVE)
            .zip(input.waveforms.chunks_mut(MAX_INTERLEAVE))
        {
            process_interleaved_group(chains, waves);
        }
        Ok(())
    }
}

/// Runs one group of channels, advancing every chain a position at a time so
/// the filters at each position can be batched.
///
/// All chains here are the same length, checked when the step was built.
fn process_interleaved_group(
    chains: &mut [Vec<Box<dyn Filter + Send>>],
    waves: &mut [Vec<CamillaFloat>],
) {
    let depth = chains.first().map(|c| c.len()).unwrap_or(0);
    for pos in 0..depth {
        // Fixed-size destructuring keeps the audio path allocation-free.
        match (&mut *chains, &mut *waves) {
            ([c0, c1, c2, c3], [w0, w1, w2, w3]) => {
                let mut fs: [&mut (dyn Filter + Send); 4] =
                    [&mut *c0[pos], &mut *c1[pos], &mut *c2[pos], &mut *c3[pos]];
                let mut ws: [&mut [CamillaFloat]; 4] =
                    [w0.as_mut(), w1.as_mut(), w2.as_mut(), w3.as_mut()];
                let _ = filters::process_channels_interleaved(fs.as_mut_slice(), ws.as_mut_slice());
            }
            ([c0, c1, c2], [w0, w1, w2]) => {
                let mut fs: [&mut (dyn Filter + Send); 3] =
                    [&mut *c0[pos], &mut *c1[pos], &mut *c2[pos]];
                let mut ws: [&mut [CamillaFloat]; 3] = [w0.as_mut(), w1.as_mut(), w2.as_mut()];
                let _ = filters::process_channels_interleaved(fs.as_mut_slice(), ws.as_mut_slice());
            }
            ([c0, c1], [w0, w1]) => {
                let mut fs: [&mut (dyn Filter + Send); 2] = [&mut *c0[pos], &mut *c1[pos]];
                let mut ws: [&mut [CamillaFloat]; 2] = [w0.as_mut(), w1.as_mut()];
                let _ = filters::process_channels_interleaved(fs.as_mut_slice(), ws.as_mut_slice());
            }
            ([c0], [w0]) => {
                let _ = c0[pos].process_waveform(w0);
            }
            _ => {}
        }
    }
}

/// A Pipeline is made up of a series of PipelineSteps,
/// each one can be a single Mixer or a group of Filters
pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
    ParallelFiltersStep(ParallelFilters),
    InterleavedFiltersStep(InterleavedFilters),
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
        // Runs with or without a pool: interleaving needs no threads.
        steps = parallelize_filters(
            &mut steps,
            conf.devices.capture.channels(),
            filter_pool.as_ref(),
        );
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
                PipelineStep::InterleavedFiltersStep(flt) => {
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
                    flt.process_chunk(&mut chunk).unwrap();
                }
                PipelineStep::ParallelFiltersStep(flt) => {
                    flt.process_chunk(&mut chunk).unwrap();
                }
                PipelineStep::InterleavedFiltersStep(flt) => {
                    flt.process_chunk(&mut chunk).unwrap();
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

// Loop through the pipeline to merge individual filter steps,
// in order use rayon to apply them in parallel.
/// Emit an accumulated filter step in the form the configuration asked for.
///
/// Multithreading is an explicit choice, not a hint, so when a pool is
/// configured every filter step goes to [`ParallelFilters`] exactly as before.
///
/// Without a pool the chains are split by position into runs that every
/// channel can process interleaved and runs that they cannot, so a
/// configuration mixing biquads with convolution still gets interleaved
/// biquads. The runs stay in position order, so each channel still sees its
/// filters in the order the configuration listed them.
fn finish_filter_step(
    filters: ChannelChains,
    pool: Option<&Arc<rayon::ThreadPool>>,
) -> Vec<PipelineStep> {
    if let Some(pool) = pool {
        return vec![PipelineStep::ParallelFiltersStep(ParallelFilters {
            filters,
            filter_pool: pool.clone(),
        })];
    }
    let mut steps = Vec::new();
    for (batchable, chains) in split_into_runs(filters) {
        if batchable {
            match InterleavedFilters::try_new(chains) {
                Ok(interleaved) => {
                    debug!("Adding interleaved biquad step");
                    steps.push(PipelineStep::InterleavedFiltersStep(interleaved));
                    continue;
                }
                Err(chains) => steps.extend(per_channel_steps(chains)),
            }
        } else {
            steps.extend(per_channel_steps(chains));
        }
    }
    steps
}

/// One [`FilterGroup`] per channel that has anything to do.
fn per_channel_steps(chains: ChannelChains) -> Vec<PipelineStep> {
    chains
        .into_iter()
        .enumerate()
        .filter(|(_, chain)| !chain.is_empty())
        .map(|(channel, filters)| PipelineStep::FilterStep(FilterGroup { channel, filters }))
        .collect()
}

/// Split per-channel chains into consecutive runs of positions, each flagged
/// with whether every channel can be processed interleaved there.
///
/// A position is batchable when every channel has a filter there and all of
/// them are biquad-based. Anything else, including a position some channel
/// does not reach, becomes a non-batchable run handled per channel.
fn split_into_runs(mut filters: ChannelChains) -> Vec<(bool, ChannelChains)> {
    let depth = filters.iter().map(|chain| chain.len()).max().unwrap_or(0);
    if depth == 0 {
        return Vec::new();
    }
    let nbr_channels = filters.len();
    let batchable: Vec<bool> = (0..depth)
        .map(|pos| {
            nbr_channels > 1
                && filters.iter_mut().all(|chain| {
                    chain
                        .get_mut(pos)
                        .is_some_and(|f| f.biquad_cascade().is_some())
                })
        })
        .collect();

    // Consecutive positions with the same verdict share a step.
    let mut runs: Vec<(bool, usize)> = Vec::new();
    for flag in batchable {
        match runs.last_mut() {
            Some((last, count)) if *last == flag => *count += 1,
            _ => runs.push((flag, 1)),
        }
    }

    let mut remaining: Vec<_> = filters.into_iter().map(|chain| chain.into_iter()).collect();
    runs.into_iter()
        .map(|(flag, count)| {
            let chains = remaining
                .iter_mut()
                .map(|it| it.by_ref().take(count).collect())
                .collect();
            (flag, chains)
        })
        .collect()
}

fn parallelize_filters(
    steps: &mut Vec<PipelineStep>,
    nbr_channels: usize,
    pool: Option<&Arc<rayon::ThreadPool>>,
) -> Vec<PipelineStep> {
    debug!("Merging filter steps to enable parallel and interleaved processing");
    let mut new_steps: Vec<PipelineStep> = Vec::new();
    let mut parfilt: Option<ChannelChains> = None;
    let mut active_channels = nbr_channels;
    for step in steps.drain(..) {
        match step {
            PipelineStep::MixerStep(ref mix) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.extend(finish_filter_step(parfilt.take().unwrap(), pool));
                }
                active_channels = mix.channels_out;
                debug!("Append mixer step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::ProcessorStep(_) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.extend(finish_filter_step(parfilt.take().unwrap(), pool));
                }
                debug!("Append processor step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::ParallelFiltersStep(_) | PipelineStep::InterleavedFiltersStep(_) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.extend(finish_filter_step(parfilt.take().unwrap(), pool));
                }
                debug!("Append existing parallel filter step to pipeline");
                new_steps.push(step);
            }
            PipelineStep::FilterStep(mut flt) => {
                if parfilt.is_none() {
                    debug!("Start new merged filter step");
                    let mut filters = Vec::with_capacity(active_channels);
                    for _ in 0..active_channels {
                        filters.push(Vec::new());
                    }
                    parfilt = Some(filters);
                }
                if let Some(ref mut f) = parfilt {
                    debug!(
                        "Adding {} filters to channel {} of merged filter step",
                        flt.filters.len(),
                        flt.channel
                    );
                    f[flt.channel].append(&mut flt.filters);
                }
            }
        }
    }
    if parfilt.is_some() {
        debug!("Append parallel filter step to pipeline");
        new_steps.extend(finish_filter_step(parfilt.take().unwrap(), pool));
    }
    new_steps
}

#[cfg(test)]
mod tests {
    use super::{Pipeline, split_into_runs};
    use crate::CamillaFloat;
    use crate::ProcessingParameters;
    use crate::Res;
    use crate::audiochunk::AudioChunk;
    use crate::config;
    use crate::filters::Filter;
    use crate::filters::biquad::{Biquad, BiquadCoefficients};
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
    /// Stand-in for a filter that cannot be batched, such as a convolution.
    struct NotABiquad;

    impl Filter for NotABiquad {
        fn process_waveform(&mut self, _waveform: &mut [CamillaFloat]) -> Res<()> {
            Ok(())
        }
        fn update_parameters(&mut self, _config: config::Filter) {}
        fn name(&self) -> &str {
            "not_a_biquad"
        }
    }

    /// `b` for a biquad, `c` for something that cannot be interleaved.
    fn chains_from(shapes: &[&str]) -> super::ChannelChains {
        shapes
            .iter()
            .map(|shape| {
                shape
                    .chars()
                    .map(|ch| match ch {
                        'b' => Box::new(Biquad::new(
                            "b",
                            44100,
                            BiquadCoefficients::new(-0.1, 0.005, 0.2, 0.4, 0.2),
                        )) as Box<dyn Filter + Send>,
                        _ => Box::new(NotABiquad) as Box<dyn Filter + Send>,
                    })
                    .collect()
            })
            .collect()
    }

    fn run_shape(shapes: &[&str]) -> Vec<(bool, usize)> {
        split_into_runs(chains_from(shapes))
            .into_iter()
            .map(|(flag, chains)| (flag, chains[0].len()))
            .collect()
    }

    #[test]
    fn splits_mixed_chains_into_runs() {
        // A convolution in the middle must not disqualify the biquads around it.
        assert_eq!(
            run_shape(&["bcbb", "bcbb"]),
            vec![(true, 1), (false, 1), (true, 2)]
        );
        assert_eq!(run_shape(&["bbbb", "bbbb"]), vec![(true, 4)]);
        assert_eq!(run_shape(&["cccc", "cccc"]), vec![(false, 4)]);
        assert_eq!(
            run_shape(&["cbbc", "cbbc"]),
            vec![(false, 1), (true, 2), (false, 1)]
        );
    }

    /// A position only some channels reach cannot be batched.
    #[test]
    fn ragged_chains_are_not_batched_past_the_shortest() {
        assert_eq!(run_shape(&["bbb", "b"]), vec![(true, 1), (false, 2)]);
    }

    /// One channel has nothing to interleave with.
    #[test]
    fn single_channel_is_never_batched() {
        assert_eq!(run_shape(&["bbb"]), vec![(false, 3)]);
    }

    /// Channels differing in filter type at the same position cannot be batched.
    #[test]
    fn mismatched_types_at_a_position_are_not_batched() {
        assert_eq!(run_shape(&["bb", "bc"]), vec![(true, 1), (false, 1)]);
    }
    /// Four channels, biquads with a non-biquad filter in the middle so the
    /// chains have to be split into runs, plus a trailing biquad cascade.
    const SPLIT_CONFIG: &str = "
devices:
  samplerate: 44100
  chunksize: 256
  capture:
    type: SignalGenerator
    channels: 4
    signal:
      type: Sine
      freq: 1000
      level: 0.0
  playback:
    type: Stdout
    channels: 4
    format: S16_LE
filters:
  hp:
    type: Biquad
    parameters:
      type: Highpass
      freq: 120
      q: 0.7
  peak:
    type: Biquad
    parameters:
      type: Peaking
      freq: 1000
      q: 1.5
      gain: 4.0
  att:
    type: Gain
    parameters:
      gain: -3.0
  lr4:
    type: BiquadCombo
    parameters:
      type: LinkwitzRileyLowpass
      freq: 2000
      order: 4
pipeline:
  - type: Filter
    channels: [0, 1, 2, 3]
    names: [hp, peak, att, lr4, hp]
";

    fn process_once(
        conf: config::Configuration,
        pool: Option<Arc<rayon::ThreadPool>>,
    ) -> Vec<Vec<CamillaFloat>> {
        let chunksize = conf.devices.chunksize;
        let channels = conf.devices.capture.channels();
        let params = Arc::new(ProcessingParameters::default());
        params.set_target_volume(0, 0.0);
        params.sync_volumes_to_target();
        let mut pipeline = Pipeline::from_config(conf, params, pool);

        // Distinct signal per channel, so a mix-up between channels shows up.
        let waveforms = (0..channels)
            .map(|ch| {
                (0..chunksize)
                    .map(|n| {
                        let t = n as CamillaFloat / 44100.0;
                        let f = 200.0 * (1.0 + ch as CamillaFloat);
                        0.5 * (2.0 * std::f64::consts::PI as CamillaFloat * f * t).sin()
                    })
                    .collect()
            })
            .collect();
        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, chunksize, chunksize);
        pipeline.process_chunk(chunk).waveforms
    }

    /// End to end check that interleaving changes nothing.
    ///
    /// Without a pool the biquads run interleaved and the chains get split
    /// around the Gain; with a pool they run one channel at a time. Each
    /// channel sees the same operations in the same order either way, so the
    /// results must match exactly.
    #[test]
    fn interleaved_pipeline_matches_parallel_pipeline() {
        let conf: config::Configuration = yaml_serde::from_str(SPLIT_CONFIG).unwrap();
        let interleaved = process_once(conf, None);

        let conf: config::Configuration = yaml_serde::from_str(SPLIT_CONFIG).unwrap();
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(2)
                .build()
                .unwrap(),
        );
        let parallel = process_once(conf, Some(pool));

        assert_eq!(interleaved.len(), parallel.len());
        for (ch, (a, b)) in interleaved.iter().zip(parallel.iter()).enumerate() {
            assert_eq!(a, b, "channel {ch} differs between the two pipeline paths");
        }
        // Guard against both paths silently doing nothing.
        let peak = interleaved
            .iter()
            .flat_map(|w| w.iter())
            .fold(0.0 as CamillaFloat, |m, v| m.max(v.abs()));
        assert!(
            peak > 1e-6,
            "pipeline produced silence, test proves nothing"
        );
    }
}
