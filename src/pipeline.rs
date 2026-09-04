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
                // A combo in a step that is not all biquads cannot join a
                // compiled cascade, so it runs as a cascade of its own.
                config::Filter::BiquadCombo { parameters, .. } => {
                    Box::new(filters::biquadcombo::Combo::new(
                        name,
                        sample_freq,
                        filters::biquadcombo::stages(sample_freq, parameters),
                    ))
                }
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

/// A consecutive run of names in a filter step that are all biquad-based, or
/// all not.
struct FilterRun {
    biquads: bool,
    names: std::ops::Range<usize>,
}

/// Split a step's filter names into consecutive runs by whether they are
/// biquad-based.
///
/// One pass over the names and no cross-channel comparison, because a filter
/// step applies the same list to every channel it reaches, so a name that is a
/// biquad is a biquad on all of them. Comparing every position of every channel
/// instead only becomes necessary after the steps have been merged, where that
/// no longer holds, which is why this runs before the merge.
fn biquad_runs(
    names: &[String],
    filter_configs: &HashMap<String, config::Filter>,
) -> Vec<FilterRun> {
    let mut runs: Vec<FilterRun> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let biquads = matches!(
            filter_configs[name],
            config::Filter::Biquad { .. } | config::Filter::BiquadCombo { .. }
        );
        match runs.last_mut() {
            Some(run) if run.biquads == biquads => run.names.end = index + 1,
            _ => runs.push(FilterRun {
                biquads,
                names: index..index + 1,
            }),
        }
    }
    runs
}

/// A run of biquad-based filters from one config filter step, compiled into
/// one cascade per channel.
///
/// A config filter step applies one `names` list to every channel it reaches,
/// so every channel here has the same shape and the same number of stages. That
/// rectangle is what lets the kernel take several channels at once, and it is
/// why compiling happens per config step rather than after the steps have been
/// merged.
///
/// The cascade runs across the filter boundaries inside the run, so eight
/// one-biquad filters reach the same depth as one eight stage combo.
pub struct BiquadStep {
    /// The chunk channel that `cascades[i]` filters.
    channel_of: Vec<usize>,
    /// One cascade per entry in `channel_of`, all the same length.
    cascades: Vec<Vec<filters::biquad::Biquad>>,
    /// One entry per config filter name, in order, shared by every channel.
    sections: Vec<Section>,
    samplerate: usize,
    /// The cascades that have audio to process this chunk. Held here and
    /// refilled in place so the audio path does not allocate.
    live: Vec<usize>,
}

/// One config filter's worth of stages inside a compiled cascade.
///
/// `len` can be zero, and can change on a reload: a graphic equalizer drops any
/// band that is flat, and `config_diff` only rebuilds the pipeline when a
/// filter changes type, so a slider crossing flat arrives here as a resize.
struct Section {
    name: String,
    len: usize,
}

/// The stages a config filter expands to, or `None` if it is not biquad-based.
fn biquad_stages(
    name: &str,
    conf: &config::Filter,
    samplerate: usize,
) -> Option<Vec<filters::biquad::Biquad>> {
    match conf {
        config::Filter::Biquad { parameters, .. } => {
            let coeffs =
                filters::biquad::BiquadCoefficients::from_config(samplerate, parameters.clone());
            Some(vec![filters::biquad::Biquad::new(name, samplerate, coeffs)])
        }
        config::Filter::BiquadCombo { parameters, .. } => {
            Some(filters::biquadcombo::stages(samplerate, parameters.clone()))
        }
        _ => None,
    }
}

impl BiquadStep {
    /// Compiles a run of biquad-based filters into one cascade per channel.
    ///
    /// `names` must be a run that [`biquad_runs`] marked as biquads, which is
    /// what makes `biquad_stages` infallible here, and `channels` must not be
    /// empty. Both are the caller's to hold up.
    fn from_config(
        channels: &[usize],
        names: &[String],
        filter_configs: &HashMap<String, config::Filter>,
        samplerate: usize,
    ) -> Self {
        // Built once and cloned per channel, so the coefficient maths runs once
        // for the step rather than once for every channel of it.
        let mut stages = Vec::new();
        let mut sections = Vec::with_capacity(names.len());
        for name in names {
            let expanded = biquad_stages(name, &filter_configs[name], samplerate)
                .expect("a biquad run holds only biquad-based filters");
            sections.push(Section {
                name: name.clone(),
                len: expanded.len(),
            });
            stages.extend(expanded);
        }
        debug!(
            "Compiled filter step with filters {:?} to {} biquads on each of {} channels",
            names,
            stages.len(),
            channels.len()
        );
        BiquadStep {
            channel_of: channels.to_vec(),
            cascades: vec![stages; channels.len()],
            sections,
            samplerate,
            live: Vec::with_capacity(channels.len()),
        }
    }

    /// Hot-reload parameters for any filters whose names appear in `changed`.
    ///
    /// The sections are walked in order with a running offset, so a section
    /// that resizes carries the new offset to the ones after it. Only the
    /// section that changed is rebuilt, so the biquads around it keep their
    /// state.
    fn update_parameters(
        &mut self,
        filterconfigs: &HashMap<String, config::Filter>,
        changed: &[String],
    ) {
        let Self {
            cascades,
            sections,
            samplerate,
            ..
        } = self;
        let mut start = 0;
        for section in sections.iter_mut() {
            if changed.iter().any(|n| n == &section.name) {
                match &filterconfigs[&section.name] {
                    // Always one stage, and coefficients only, so the filter
                    // state survives exactly as it does for a lone biquad.
                    config::Filter::Biquad { parameters, .. } => {
                        let coeffs = filters::biquad::BiquadCoefficients::from_config(
                            *samplerate,
                            parameters.clone(),
                        );
                        for cascade in cascades.iter_mut() {
                            cascade[start].set_coefficients(coeffs);
                        }
                    }
                    // The stage count can change, so splice. State resets for
                    // this section, which is what rebuilding a combo has always
                    // done.
                    config::Filter::BiquadCombo { parameters, .. } => {
                        let rebuilt = filters::biquadcombo::stages(*samplerate, parameters.clone());
                        for cascade in cascades.iter_mut() {
                            cascade.splice(start..start + section.len, rebuilt.iter().cloned());
                        }
                        section.len = rebuilt.len();
                    }
                    _ => unreachable!("only biquad-based filters are compiled into a cascade"),
                }
            }
            start += section.len;
        }
    }

    /// Apply the cascades to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) {
        // An unused capture channel arrives as an empty waveform, and the
        // kernel walks the channels of a group together. Leaving one in would
        // let it decide the length for the whole group and silently leave the
        // others unfiltered, so it is kept out of the grouping entirely.
        self.live.clear();
        for (position, &channel) in self.channel_of.iter().enumerate() {
            if !input.waveforms[channel].is_empty() {
                self.live.push(position);
            }
        }
        if self.live.is_empty() {
            return;
        }
        filters::biquad::process_cascades(
            &mut self.cascades,
            &mut input.waveforms,
            &self.channel_of,
            &self.live,
        );
    }
}

/// A Pipeline is made up of a series of PipelineSteps,
/// each one can be a single Mixer or a group of Filters
pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
    ParallelFiltersStep(ParallelFilters),
    BiquadStep(BiquadStep),
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
                        let channels: Vec<usize> = channels_iter.collect();
                        // A step can name an empty list of channels, which
                        // makes it a step that does nothing.
                        if channels.is_empty() {
                            continue;
                        }
                        let filter_configs = conf.filters.as_ref().unwrap();
                        // Runs of biquad-based filters compile into one cascade
                        // per channel, so the canon reaches across the
                        // filter boundaries inside a run. Everything else keeps
                        // the per-channel chains. A run is decided by the names
                        // alone, since the step applies the same list to every
                        // channel it reaches.
                        //
                        // Splitting a step this way runs all of one run's
                        // channels before any of the next run's, where the
                        // whole step used to run channel by channel. Each
                        // channel still sees its filters in the configured
                        // order; only filters that share state across channels
                        // can tell, which `parallelize_filters` describes.
                        for run in biquad_runs(&step.names, filter_configs) {
                            let names = &step.names[run.names.clone()];
                            if run.biquads {
                                steps.push(PipelineStep::BiquadStep(BiquadStep::from_config(
                                    &channels,
                                    names,
                                    filter_configs,
                                    conf.devices.samplerate,
                                )));
                                continue;
                            }
                            for &channel in &channels {
                                let fltgrp = FilterGroup::from_config(
                                    channel,
                                    names,
                                    filter_configs.clone(),
                                    conf.devices.chunksize,
                                    conf.devices.samplerate,
                                    processing_params.clone(),
                                    &mut coeff_cache,
                                );
                                steps.push(PipelineStep::FilterStep(fltgrp));
                            }
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
                PipelineStep::BiquadStep(flt) => {
                    flt.update_parameters(conf.filters.as_ref().unwrap(), filters);
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
                PipelineStep::BiquadStep(flt) => {
                    flt.process_chunk(&mut chunk);
                }
                PipelineStep::ProcessorStep(comp) => {
                    comp.process_chunk(&mut chunk);
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
/// This is not only the pool's doing. A compiled biquad run splits a step for
/// every channel at once, so a biquad between a `Volume` and a `Loudness` puts
/// the same cross-channel interleaving in the default single threaded path.
/// The pool widens the reordering, it does not introduce it.
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
            PipelineStep::ProcessorStep(_) | PipelineStep::BiquadStep(_) => {
                if parfilt.is_some() {
                    debug!("Append parallel filter step to pipeline");
                    new_steps.push(PipelineStep::ParallelFiltersStep(parfilt.take().unwrap()));
                }
                debug!("Append step to pipeline without merging it");
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
    use super::{Pipeline, PipelineStep};
    use crate::CamillaFloat;
    use crate::ProcessingParameters;
    use crate::audiochunk::AudioChunk;
    use crate::filters::Filter;
    use std::sync::Arc;

    /// Filters shared by the tests below. Distinct coefficients, so a stage
    /// that runs out of order or twice cannot go unnoticed, and a graphic
    /// equalizer in the middle whose stage count depends on its gains.
    const FILTERS: &str = "
filters:
  bq_a:
    type: Biquad
    parameters:
      type: Peaking
      freq: 120
      q: 1.1
      gain: 3.0
  bq_b:
    type: Biquad
    parameters:
      type: Lowshelf
      freq: 400
      q: 0.7
      gain: -2.5
  combo:
    type: BiquadCombo
    parameters:
      type: ButterworthHighpass
      freq: 80
      order: 4
  geq:
    type: BiquadCombo
    parameters:
      type: GraphicEqualizer
      gains: [1.0, 0.0, -2.0, 0.0, 3.0]
  bq_c:
    type: Biquad
    parameters:
      type: Highshelf
      freq: 5000
      q: 0.8
      gain: 1.5
";

    fn config_with(
        pipeline: &str,
        channels: usize,
        chunksize: usize,
    ) -> crate::config::Configuration {
        let text = format!(
            "
devices:
  samplerate: 44100
  chunksize: {chunksize}
  capture:
    type: SignalGenerator
    channels: {channels}
    signal:
      type: Sine
      freq: 1000
      level: 0.0
  playback:
    type: Stdout
    channels: {channels}
    format: S16_LE
{FILTERS}
{pipeline}
"
        );
        yaml_serde::from_str(&text).unwrap()
    }

    fn test_signal(n: usize, seed: usize) -> Vec<CamillaFloat> {
        (0..n)
            .map(|i| (0.017 * ((7 * i + 13 * seed) as CamillaFloat)).sin() * 0.5)
            .collect()
    }

    /// The stages a named filter expands to, built straight from the config so
    /// the reference does not go through any of the code under test.
    fn reference_stages(
        conf: &crate::config::Configuration,
        name: &str,
    ) -> Vec<crate::filters::biquad::Biquad> {
        let fs = conf.devices.samplerate;
        super::biquad_stages(name, &conf.filters.as_ref().unwrap()[name], fs).unwrap()
    }

    fn assert_bit_equal(left: &[Vec<CamillaFloat>], right: &[Vec<CamillaFloat>], what: &str) {
        assert_eq!(left.len(), right.len(), "{what}: channels");
        for (ch, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(l.len(), r.len(), "{what}: length of channel {ch}");
            for (i, (a, b)) in l.iter().zip(r.iter()).enumerate() {
                assert_eq!(a.to_bits(), b.to_bits(), "{what}: channel {ch} sample {i}");
            }
        }
    }

    /// A compiled step has to produce exactly what the same filters produce run
    /// one at a time. This is the property that makes compiling them safe, and
    /// the reference here is genuinely sequential: every stage over the whole
    /// waveform, through `Biquad::process_waveform`.
    ///
    /// The master volume sits ahead of the steps, but at 0 dB with no ramp it
    /// multiplies by exactly 1.0, so it does not disturb the comparison.
    #[test]
    fn compiled_step_matches_sequential() {
        const CHANNELS: usize = 2;
        const CHUNK: usize = 256;
        const NAMES: [&str; 5] = ["bq_a", "combo", "bq_b", "geq", "bq_c"];
        let conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, combo, bq_b, geq, bq_c]
",
            CHANNELS,
            CHUNK,
        );

        let params = Arc::new(ProcessingParameters::default());
        let mut pipeline = Pipeline::from_config(conf.clone(), params, None);
        assert!(
            matches!(pipeline.steps[0], PipelineStep::BiquadStep(_)),
            "an all-biquad step should compile"
        );

        let waveforms: Vec<Vec<CamillaFloat>> =
            (0..CHANNELS).map(|c| test_signal(CHUNK, c)).collect();
        let chunk = AudioChunk::new(waveforms.clone(), 1.0, -1.0, CHUNK, CHUNK);
        let out = pipeline.process_chunk(chunk);

        let mut want = waveforms;
        for waveform in want.iter_mut() {
            for name in NAMES {
                for stage in reference_stages(&conf, name).iter_mut() {
                    stage.process_waveform(waveform);
                }
            }
        }
        assert_bit_equal(&out.waveforms, &want, "compiled against sequential");
    }

    /// A channel the capture step does not use arrives as an empty waveform.
    /// The kernel walks the channels of a group together, so an empty one left
    /// in would decide the length for the whole group and silently leave the
    /// others unfiltered.
    #[test]
    fn an_unused_channel_does_not_silence_the_others() {
        const CHANNELS: usize = 4;
        const CHUNK: usize = 128;
        let conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, bq_b, bq_c]
",
            CHANNELS,
            CHUNK,
        );

        let params = Arc::new(ProcessingParameters::default());
        let mut pipeline = Pipeline::from_config(conf.clone(), params, None);

        // Only 0 and 2 carry audio, as they would ahead of a mixer that reads
        // just those two of four capture channels.
        let live = [0usize, 2];
        let waveforms: Vec<Vec<CamillaFloat>> = (0..CHANNELS)
            .map(|c| {
                if live.contains(&c) {
                    test_signal(CHUNK, c)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let chunk = AudioChunk::new(waveforms.clone(), 1.0, -1.0, CHUNK, CHUNK);
        let out = pipeline.process_chunk(chunk);

        let mut want = waveforms;
        for waveform in want.iter_mut() {
            for name in ["bq_a", "bq_b", "bq_c"] {
                for stage in reference_stages(&conf, name).iter_mut() {
                    stage.process_waveform(waveform);
                }
            }
        }
        assert_bit_equal(&out.waveforms, &want, "with two unused channels");
        for &c in &live {
            assert!(
                out.waveforms[c].iter().any(|v| *v != 0.0),
                "channel {c} came out silent"
            );
        }
    }

    /// A graphic equalizer resizes whenever a band crosses flat, and
    /// `config_diff` sends that through as an in-place parameter update rather
    /// than a rebuild. The section has to be spliced without disturbing the
    /// biquads on either side of it, which keep their filter state.
    #[test]
    fn a_resizing_combo_leaves_its_neighbours_alone() {
        const CHUNK: usize = 128;
        let conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, geq, bq_c]
",
            2,
            CHUNK,
        );
        let params = Arc::new(ProcessingParameters::default());
        let mut pipeline = Pipeline::from_config(conf.clone(), params, None);

        // Run a chunk so every stage has state worth preserving.
        let waveforms: Vec<Vec<CamillaFloat>> = (0..2).map(|c| test_signal(CHUNK, c)).collect();
        pipeline.process_chunk(AudioChunk::new(waveforms, 1.0, -1.0, CHUNK, CHUNK));

        let PipelineStep::BiquadStep(step) = &pipeline.steps[0] else {
            panic!("expected a compiled step");
        };
        // Three active bands out of five, plus one biquad either side.
        assert_eq!(
            step.sections.iter().map(|s| s.len).collect::<Vec<_>>(),
            vec![1, 3, 1]
        );
        let before: Vec<_> = step.cascades[0]
            .iter()
            .map(|b| (b.s1.to_bits(), b.s2.to_bits()))
            .collect();
        let first_before = before[0];
        let last_before = *before.last().unwrap();

        // Two more bands come off flat, so the section grows from three to five.
        let mut newconf = conf.clone();
        newconf.filters.as_mut().unwrap().insert(
            "geq".to_string(),
            yaml_serde::from_str(
                "
type: BiquadCombo
parameters:
  type: GraphicEqualizer
  gains: [1.0, 2.0, -2.0, -1.0, 3.0]
",
            )
            .unwrap(),
        );
        pipeline.update_parameters(newconf, &["geq".to_string()], &[], &[]);

        let PipelineStep::BiquadStep(step) = &pipeline.steps[0] else {
            panic!("expected a compiled step");
        };
        assert_eq!(
            step.sections.iter().map(|s| s.len).collect::<Vec<_>>(),
            vec![1, 5, 1],
            "the section should have grown"
        );
        assert_eq!(step.cascades[0].len(), 7, "and the cascade with it");
        let after: Vec<_> = step.cascades[0]
            .iter()
            .map(|b| (b.s1.to_bits(), b.s2.to_bits()))
            .collect();
        assert_eq!(
            after[0], first_before,
            "the biquad before it lost its state"
        );
        assert_eq!(
            *after.last().unwrap(),
            last_before,
            "the biquad after it lost its state"
        );

        // The spliced cascade also has to still process correctly. Take the
        // stages as they now stand, run the next chunk through the pipeline,
        // and check it against those same stages run one at a time from the
        // same state.
        let mut reference = step.cascades.clone();
        let next: Vec<Vec<CamillaFloat>> = (0..2).map(|c| test_signal(CHUNK, c + 7)).collect();
        let out = pipeline.process_chunk(AudioChunk::new(next.clone(), 1.0, -1.0, CHUNK, CHUNK));
        let mut want = next;
        for (cascade, waveform) in reference.iter_mut().zip(want.iter_mut()) {
            for stage in cascade.iter_mut() {
                stage.process_waveform(waveform);
            }
        }
        assert_bit_equal(&out.waveforms, &want, "processing after the resize");
        let peak = out
            .waveforms
            .iter()
            .flat_map(|w| w.iter())
            .fold(0.0 as CamillaFloat, |m, v| m.max(v.abs()));
        assert!(
            peak > 1e-6,
            "silence after the resize, the test proves nothing"
        );
    }

    /// A step that mixes biquads with anything else splits into runs: the
    /// biquad runs compile, the rest keeps the per-channel chains. Each channel
    /// still sees its filters in the order the configuration listed them, which
    /// is what this checks by running the same filters by hand.
    #[test]
    fn a_mixed_step_splits_into_runs() {
        const CHANNELS: usize = 2;
        const CHUNK: usize = 128;
        let mut conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, combo, gain, bq_b, bq_c]
",
            CHANNELS,
            CHUNK,
        );
        conf.filters.as_mut().unwrap().insert(
            "gain".to_string(),
            yaml_serde::from_str(
                "
type: Gain
parameters:
  gain: -6.0
",
            )
            .unwrap(),
        );

        let params = Arc::new(ProcessingParameters::default());
        let mut pipeline = Pipeline::from_config(conf.clone(), params, None);

        // Two biquads either side of the gain compile; the gain does not.
        let shape: Vec<&str> = pipeline
            .steps
            .iter()
            .map(|s| match s {
                PipelineStep::BiquadStep(_) => "biquad",
                PipelineStep::FilterStep(_) => "chain",
                _ => "other",
            })
            .collect();
        assert_eq!(
            shape,
            vec!["biquad", "chain", "chain", "biquad"],
            "expected a compiled run, the gain on each channel, then another run"
        );

        let waveforms: Vec<Vec<CamillaFloat>> =
            (0..CHANNELS).map(|c| test_signal(CHUNK, c)).collect();
        let chunk = AudioChunk::new(waveforms.clone(), 1.0, -1.0, CHUNK, CHUNK);
        let out = pipeline.process_chunk(chunk);

        let gain_conf = conf.filters.as_ref().unwrap()["gain"].clone();
        let crate::config::Filter::Gain { parameters, .. } = gain_conf else {
            unreachable!()
        };
        let mut want = waveforms;
        for waveform in want.iter_mut() {
            for stage in reference_stages(&conf, "bq_a").iter_mut() {
                stage.process_waveform(waveform);
            }
            for stage in reference_stages(&conf, "combo").iter_mut() {
                stage.process_waveform(waveform);
            }
            crate::filters::basicfilters::Gain::from_config("gain", parameters.clone())
                .process_waveform(waveform);
            for stage in reference_stages(&conf, "bq_b").iter_mut() {
                stage.process_waveform(waveform);
            }
            for stage in reference_stages(&conf, "bq_c").iter_mut() {
                stage.process_waveform(waveform);
            }
        }
        assert_bit_equal(&out.waveforms, &want, "mixed step split into runs");
    }

    /// Runs one chunk through a pipeline built from `conf`, with or without a
    /// thread pool, and hands back the waveforms.
    fn process_once(
        conf: crate::config::Configuration,
        multithreaded: bool,
        channels: usize,
    ) -> Vec<Vec<CamillaFloat>> {
        let chunksize = conf.devices.chunksize;
        let pool = crate::processing::build_processing_threadpool(
            multithreaded,
            conf.devices.worker_threads(),
            chunksize,
            conf.devices.samplerate,
        );
        let params = Arc::new(ProcessingParameters::default());
        let mut pipeline = Pipeline::from_config(conf, params, pool);
        let waveforms: Vec<Vec<CamillaFloat>> =
            (0..channels).map(|c| test_signal(chunksize, c)).collect();
        let chunk = AudioChunk::new(waveforms, 1.0, -1.0, chunksize, chunksize);
        pipeline.process_chunk(chunk).waveforms
    }

    /// A step can name the channels it applies to, so different channels end up
    /// with different chains. That is the crossover idiom the README documents,
    /// and it means the pipeline as a whole is ragged even though each
    /// individual step is not.
    #[test]
    fn channel_selectors_give_each_channel_its_own_chain() {
        const CHANNELS: usize = 4;
        const CHUNK: usize = 128;
        let conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, combo]
  - type: Filter
    channels: [0, 1]
    names: [bq_b, geq]
  - type: Filter
    channels: [2, 3]
    names: [bq_c]
",
            CHANNELS,
            CHUNK,
        );
        let got = process_once(conf.clone(), false, CHANNELS);

        // Every channel takes the first step; only its own pair takes the rest.
        let per_channel: [&[&str]; CHANNELS] = [
            &["bq_a", "combo", "bq_b", "geq"],
            &["bq_a", "combo", "bq_b", "geq"],
            &["bq_a", "combo", "bq_c"],
            &["bq_a", "combo", "bq_c"],
        ];
        let mut want: Vec<Vec<CamillaFloat>> =
            (0..CHANNELS).map(|c| test_signal(CHUNK, c)).collect();
        for (channel, names) in per_channel.iter().enumerate() {
            for name in *names {
                for stage in reference_stages(&conf, name).iter_mut() {
                    stage.process_waveform(&mut want[channel]);
                }
            }
        }
        assert_bit_equal(&got, &want, "channel selectors");

        // Guards against a vacuous pass: the chains really do differ, and the
        // pipeline really did produce audio.
        assert!(
            got[0] != got[2],
            "channels 0 and 2 took different chains but came out identical"
        );
        let peak = got
            .iter()
            .flat_map(|w| w.iter())
            .fold(0.0 as CamillaFloat, |m, v| m.max(v.abs()));
        assert!(
            peak > 1e-6,
            "pipeline produced silence, the test proves nothing"
        );
    }

    /// Compiled cascades stay on the calling thread, because running several
    /// biquads at a time already keeps the processor busy. Everything else
    /// still goes to the pool when one is configured, which is what heavy
    /// convolution wants.
    #[test]
    fn the_pool_takes_everything_except_the_cascades() {
        let mut conf = config_with(
            "
pipeline:
  - type: Filter
    names: [bq_a, bq_b]
  - type: Filter
    names: [conv]
",
            2,
            64,
        );
        conf.filters.as_mut().unwrap().insert(
            "conv".to_string(),
            yaml_serde::from_str(
                "
type: Conv
parameters:
  type: Values
  values: [0.5, 0.25, 0.125]
",
            )
            .unwrap(),
        );
        let pool = crate::processing::build_processing_threadpool(true, 2, 64, 44100);
        assert!(pool.is_some(), "the test needs a pool to be built");
        let params = Arc::new(ProcessingParameters::default());
        let pipeline = Pipeline::from_config(conf, params, pool);

        let shape: Vec<&str> = pipeline
            .steps
            .iter()
            .map(|s| match s {
                PipelineStep::BiquadStep(_) => "cascade",
                PipelineStep::ParallelFiltersStep(_) => "pool",
                PipelineStep::FilterStep(_) => "chain",
                _ => "other",
            })
            .collect();
        assert_eq!(
            shape,
            vec!["cascade", "pool"],
            "biquads should stay off the pool and the convolution should go on it"
        );
    }

    /// Running on the pool must not change the audio. The merging that happens
    /// when a pool is configured reorders filters across channels, so this
    /// pins that it stays unobservable for a pipeline with no shared state
    /// between channels.
    #[test]
    fn the_pool_does_not_change_the_audio() {
        const CHANNELS: usize = 4;
        let pipeline = "
pipeline:
  - type: Filter
    names: [bq_a, combo]
  - type: Filter
    channels: [0, 1]
    names: [bq_b]
  - type: Filter
    names: [geq, bq_c]
";
        let single = process_once(config_with(pipeline, CHANNELS, 128), false, CHANNELS);
        let pooled = process_once(config_with(pipeline, CHANNELS, 128), true, CHANNELS);
        assert_bit_equal(&pooled, &single, "pooled against single threaded");
    }

    /// A cascade too shallow to fill the width budget on its own has to spend
    /// what is left on channels, which is the whole reason the kernel carries
    /// a channel axis at all.
    #[test]
    fn a_shallow_cascade_spends_the_rest_of_the_budget_on_channels() {
        use crate::filters::biquad::{MAX_CHANNELS, MAX_DEPTH, choose_split};
        // Deep enough to fill the budget alone: one channel at a time.
        assert_eq!(choose_split(8, 32), (1, MAX_DEPTH));
        assert_eq!(choose_split(2, 16), (1, MAX_DEPTH));
        // Too shallow: the channel axis takes up the slack.
        assert_eq!(choose_split(8, 2), (MAX_CHANNELS, 2));
        assert_eq!(choose_split(8, 1), (MAX_CHANNELS, 1));
        // And it cannot ask for more channels than the step actually has.
        assert_eq!(choose_split(2, 1), (2, 1));
        assert_eq!(choose_split(1, 1), (1, 1));
    }

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
