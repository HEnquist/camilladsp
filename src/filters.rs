// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2025 Henrik Enquist
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

use crate::audiodevice::AudioChunk;
use crate::basicfilters;
use crate::biquad;
use crate::biquadcombo;
use crate::compressor;
use crate::config;
use crate::diffeq;
use crate::dither;
use crate::fftconv;
use crate::limiter;
use crate::loudness;
use crate::mixer;
use crate::noisegate;
use crate::race;
use audioadapter::readwrite::ReadSamples;
use audioadapter::sample::{F32LE, F64LE, I16LE, I24LE, I32LE};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::Res;

use crate::wavtools::find_data_in_wav;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()>;

    fn update_parameters(&mut self, config: config::Filter);

    fn name(&self) -> &str;
}

pub trait Processor {
    // Process a chunk containing several channels.
    fn process_chunk(&mut self, chunk: &mut AudioChunk) -> Res<()>;

    fn update_parameters(&mut self, config: config::Processor);

    fn name(&self) -> &str;
}

pub fn pad_vector(values: &[PrcFmt], length: usize) -> Vec<PrcFmt> {
    let new_len = if values.len() > length {
        values.len()
    } else {
        length
    };
    let mut new_values: Vec<PrcFmt> = vec![0.0; new_len];
    new_values[0..values.len()].copy_from_slice(values);
    new_values
}

pub fn read_coeff_file(
    filename: &str,
    format: &config::FileFormat,
    read_bytes_lines: usize,
    skip_bytes_lines: usize,
) -> Res<Vec<PrcFmt>> {
    let mut coefficients = Vec::<PrcFmt>::new();
    let f = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("Could not open coefficient file '{filename}'. Reason: {err}");
            return Err(config::ConfigError::new(&msg).into());
        }
    };
    let mut file = BufReader::new(&f);
    let read_bytes_lines = if read_bytes_lines > 0 {
        read_bytes_lines
    } else {
        usize::MAX
    };

    match format {
        // Handle TEXT separately
        config::FileFormat::TEXT => {
            for (nbr, line) in file
                .lines()
                .skip(skip_bytes_lines)
                .take(read_bytes_lines)
                .enumerate()
            {
                match line {
                    Err(err) => {
                        let msg = format!(
                            "Can't read line {} of file '{}'. Reason: {}",
                            nbr + 1 + skip_bytes_lines,
                            filename,
                            err
                        );
                        return Err(config::ConfigError::new(&msg).into());
                    }
                    Ok(l) => match l.trim().parse() {
                        Ok(val) => coefficients.push(val),
                        Err(err) => {
                            let msg = format!(
                                "Can't parse value on line {} of file '{}'. Reason: {}",
                                nbr + 1 + skip_bytes_lines,
                                filename,
                                err
                            );
                            return Err(config::ConfigError::new(&msg).into());
                        }
                    },
                }
            }
        }
        // All other formats
        _ => {
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / format.bytes_per_sample();
            let limit = if nbr_coeffs > 0 {
                Some(nbr_coeffs)
            } else {
                None
            };

            match *format {
                config::FileFormat::S16LE => {
                    file.read_converted_to_limit_or_end::<I16LE, PrcFmt>(&mut coefficients, limit)?;
                }
                config::FileFormat::S24LE3 => {
                    file.read_converted_to_limit_or_end::<I24LE<3>, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::FileFormat::S24LE => {
                    file.read_converted_to_limit_or_end::<I24LE<4>, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::FileFormat::S32LE => {
                    file.read_converted_to_limit_or_end::<I32LE, PrcFmt>(&mut coefficients, limit)?;
                }
                config::FileFormat::FLOAT32LE => {
                    file.read_converted_to_limit_or_end::<F32LE, PrcFmt>(&mut coefficients, limit)?;
                }
                config::FileFormat::FLOAT64LE => {
                    file.read_converted_to_limit_or_end::<F64LE, PrcFmt>(&mut coefficients, limit)?;
                }
                config::FileFormat::TEXT => unreachable!(),
            }
            debug!("Read {} coeffs from file", coefficients.len());
        }
    }
    debug!(
        "Read raw data from: '{}', format: {:?}, number of coeffs: {}",
        filename,
        format,
        coefficients.len()
    );
    Ok(coefficients)
}

pub fn read_wav(filename: &str, channel: usize) -> Res<Vec<PrcFmt>> {
    let params = find_data_in_wav(filename)?;
    if channel >= params.channels {
        let msg = format!(
            "Cant read channel {} of file '{}' which contains {} channels.",
            channel, filename, params.channels
        );
        return Err(config::ConfigError::new(&msg).into());
    }

    let alldata = read_coeff_file(
        filename,
        &config::FileFormat::from_sample_format(&params.sample_format),
        params.data_length,
        params.data_offset,
    )?;

    let data = alldata
        .iter()
        .skip(channel)
        .step_by(params.channels)
        .copied()
        .collect::<Vec<PrcFmt>>();
    debug!(
        "Read wav file '{}', format: {:?}, channel: {} of {}, samplerate: {}, length: {}",
        filename,
        params.sample_format,
        channel,
        params.channels,
        params.sample_rate,
        data.len()
    );
    Ok(data)
}

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
    ) -> Self {
        debug!("Build filter group from config");
        let mut filters = Vec::<Box<dyn Filter + Send>>::new();
        for name in names {
            let filter_cfg = filter_configs[name].clone();
            trace!("Create filter {} with config {:?}", name, filter_cfg);
            let filter: Box<dyn Filter + Send> =
                match filter_cfg {
                    config::Filter::Conv { parameters, .. } => Box::new(
                        fftconv::FftConv::from_config(name, waveform_length, parameters),
                    ),
                    config::Filter::Biquad { parameters, .. } => Box::new(biquad::Biquad::new(
                        name,
                        sample_freq,
                        biquad::BiquadCoefficients::from_config(sample_freq, parameters),
                    )),
                    config::Filter::BiquadCombo { parameters, .. } => Box::new(
                        biquadcombo::BiquadCombo::from_config(name, sample_freq, parameters),
                    ),
                    config::Filter::Delay { parameters, .. } => Box::new(
                        basicfilters::Delay::from_config(name, sample_freq, parameters),
                    ),
                    config::Filter::Gain { parameters, .. } => {
                        Box::new(basicfilters::Gain::from_config(name, parameters))
                    }
                    config::Filter::Volume { parameters, .. } => {
                        Box::new(basicfilters::Volume::from_config(
                            name,
                            parameters,
                            waveform_length,
                            sample_freq,
                            processing_params.clone(),
                        ))
                    }
                    config::Filter::Loudness { parameters, .. } => {
                        Box::new(loudness::Loudness::from_config(
                            name,
                            parameters,
                            sample_freq,
                            processing_params.clone(),
                        ))
                    }
                    config::Filter::Dither { parameters, .. } => {
                        Box::new(dither::Dither::from_config(name, parameters))
                    }
                    config::Filter::DiffEq { parameters, .. } => {
                        Box::new(diffeq::DiffEq::from_config(name, parameters))
                    }
                    config::Filter::Limiter { parameters, .. } => {
                        Box::new(limiter::Limiter::from_config(name, parameters))
                    }
                };
            filters.push(filter);
        }
        FilterGroup { channel, filters }
    }

    pub fn update_parameters(
        &mut self,
        filterconfigs: HashMap<String, config::Filter>,
        changed: &[String],
    ) {
        for filter in &mut self.filters {
            if changed.iter().any(|n| n == filter.name()) {
                filter.update_parameters(filterconfigs[filter.name()].clone());
            }
        }
    }

    /// Apply all the filters to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        if !input.waveforms[self.channel].is_empty() {
            // Zeroes all sse registers on x86_64 architecturesto work around
            // rustc bug https://github.com/rust-lang/rust/issues/116359
            #[cfg(all(target_arch = "x86_64", feature = "avoid-rustc-issue-116359"))]
            unsafe {
                use std::arch::asm;
                asm!(
                    "xorpd xmm0, xmm0",
                    "xorpd xmm1, xmm1",
                    "xorpd xmm2, xmm2",
                    "xorpd xmm3, xmm3",
                    "xorpd xmm4, xmm4",
                    "xorpd xmm5, xmm5",
                    "xorpd xmm6, xmm6",
                    "xorpd xmm7, xmm7",
                    "xorpd xmm8, xmm8",
                    "xorpd xmm9, xmm9",
                    "xorpd xmm10, xmm10",
                    "xorpd xmm11, xmm11",
                    "xorpd xmm12, xmm12",
                    "xorpd xmm13, xmm13",
                    "xorpd xmm14, xmm14",
                    "xorpd xmm15, xmm15"
                )
            }
            for filter in &mut self.filters {
                filter.process_waveform(&mut input.waveforms[self.channel])?;
            }
        }
        Ok(())
    }
}

pub struct ParallelFilters {
    filters: Vec<Vec<Box<dyn Filter + Send>>>,
}

impl ParallelFilters {
    pub fn update_parameters(
        &mut self,
        filterconfigs: HashMap<String, config::Filter>,
        changed: &[String],
    ) {
        for channel_filters in &mut self.filters {
            for filter in channel_filters {
                if changed.iter().any(|n| n == filter.name()) {
                    filter.update_parameters(filterconfigs[filter.name()].clone());
                }
            }
        }
    }

    /// Apply all the filters to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        self.filters
            .par_iter_mut()
            .zip(input.waveforms.par_iter_mut())
            .filter(|(f, w)| !f.is_empty() && !w.is_empty())
            .for_each(|(f, w)| {
                for filt in f {
                    let _ = filt.process_waveform(w);
                }
            });
        Ok(())
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

pub struct Pipeline {
    steps: Vec<PipelineStep>,
    volume: basicfilters::Volume,
    secs_per_chunk: f32,
    processing_params: Arc<ProcessingParameters>,
}

impl Pipeline {
    /// Create a new pipeline from a configuration structure.
    pub fn from_config(
        conf: config::Configuration,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        debug!("Build new pipeline");
        trace!("Pipeline config {:?}", conf.pipeline);
        let mut steps = Vec::<PipelineStep>::new();
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
                                let comp = compressor::Compressor::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                    conf.devices.chunksize,
                                );
                                Box::new(comp) as Box<dyn Processor>
                            }
                            config::Processor::NoiseGate { parameters, .. } => {
                                let gate = noisegate::NoiseGate::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                    conf.devices.chunksize,
                                );
                                Box::new(gate) as Box<dyn Processor>
                            }
                            config::Processor::RACE { parameters, .. } => {
                                let race = race::RACE::from_config(
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
        let volume = basicfilters::Volume::new(
            "default",
            conf.devices.ramp_time(),
            conf.devices.volume_limit(),
            current_volume,
            mute,
            conf.devices.chunksize,
            conf.devices.samplerate,
            processing_params.clone(),
            0,
        );
        let secs_per_chunk = conf.devices.chunksize as f32 / conf.devices.samplerate as f32;
        if conf.devices.multithreaded() {
            steps = parallelize_filters(&mut steps, conf.devices.capture.channels());
        }
        Pipeline {
            steps,
            volume,
            secs_per_chunk,
            processing_params,
        }
    }

    pub fn update_parameters(
        &mut self,
        conf: config::Configuration,
        filters: &[String],
        mixers: &[String],
        processors: &[String],
    ) {
        debug!("Updating parameters");
        for mut step in &mut self.steps {
            match &mut step {
                PipelineStep::MixerStep(mix) => {
                    if mixers.iter().any(|n| n == &mix.name) {
                        mix.update_parameters(conf.mixers.as_ref().unwrap()[&mix.name].clone());
                    }
                }
                PipelineStep::FilterStep(flt) => {
                    flt.update_parameters(conf.filters.as_ref().unwrap().clone(), filters);
                }
                PipelineStep::ParallelFiltersStep(flt) => {
                    flt.update_parameters(conf.filters.as_ref().unwrap().clone(), filters);
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
                PipelineStep::ProcessorStep(comp) => {
                    comp.process_chunk(&mut chunk).unwrap();
                }
            }
        }
        let secs_elapsed = start.elapsed().as_secs_f32();
        let load = 100.0 * secs_elapsed / self.secs_per_chunk;
        self.processing_params.set_processing_load(load);
        trace!("Processing load: {load}%");
        chunk
    }
}

// Loop trough the pipeline to merge individual filter steps,
// in order use rayon to apply them in parallel.
fn parallelize_filters(steps: &mut Vec<PipelineStep>, nbr_channels: usize) -> Vec<PipelineStep> {
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
                    parfilt = Some(ParallelFilters { filters });
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

/// Validate the filter config, to give a helpful message intead of a panic.
pub fn validate_filter(fs: usize, filter_config: &config::Filter) -> Res<()> {
    match filter_config {
        config::Filter::Conv { parameters, .. } => fftconv::validate_config(parameters),
        config::Filter::Biquad { parameters, .. } => biquad::validate_config(fs, parameters),
        config::Filter::Delay { parameters, .. } => basicfilters::validate_delay_config(parameters),
        config::Filter::Gain { parameters, .. } => basicfilters::validate_gain_config(parameters),
        config::Filter::Dither { parameters, .. } => dither::validate_config(parameters),
        config::Filter::DiffEq { parameters, .. } => diffeq::validate_config(parameters),
        config::Filter::Volume { parameters, .. } => {
            basicfilters::validate_volume_config(parameters)
        }
        config::Filter::Loudness { parameters, .. } => loudness::validate_config(parameters),
        config::Filter::BiquadCombo { parameters, .. } => {
            biquadcombo::validate_config(fs, parameters)
        }
        config::Filter::Limiter { parameters, .. } => limiter::validate_config(parameters),
    }
}

#[cfg(test)]
mod tests {
    use crate::config::FileFormat;
    use crate::filters::read_wav;
    use crate::filters::{pad_vector, read_coeff_file};
    use crate::PrcFmt;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {} = {}", left, right, left - right);
        let res = (left - right).abs() < maxdiff;
        println!("Ok: {res}");
        res
    }

    fn compare_waveforms(left: &[PrcFmt], right: &[PrcFmt], maxdiff: PrcFmt) -> bool {
        if left.len() != right.len() {
            println!("wrong length");
            return false;
        }
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn read_float32() {
        let loaded = read_coeff_file("testdata/float32.raw", &FileFormat::FLOAT32LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileFormat::FLOAT32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_float64() {
        let loaded = read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 24, 8).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int16() {
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 6, 2).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int24() {
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int24_3() {
        let loaded = read_coeff_file("testdata/int243.raw", &FileFormat::S24LE3, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded = read_coeff_file("testdata/int243.raw", &FileFormat::S24LE3, 9, 3).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int32() {
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_text() {
        let loaded = read_coeff_file("testdata/text.txt", &FileFormat::TEXT, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded = read_coeff_file("testdata/text_header.txt", &FileFormat::TEXT, 4, 1).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn test_padding() {
        let values: Vec<PrcFmt> = vec![1.0, 0.5];
        let values_padded: Vec<PrcFmt> = vec![1.0, 0.5, 0.0, 0.0, 0.0];
        let values_0 = pad_vector(&values, 0);
        assert!(compare_waveforms(&values, &values_0, 1e-15));
        let values_5 = pad_vector(&values, 5);
        assert!(compare_waveforms(&values_padded, &values_5, 1e-15));
    }

    #[test]
    pub fn test_read_wav() {
        let values = read_wav("testdata/int32.wav", 0).unwrap();
        println!("{values:?}");
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&values, &expected, 1e-9));
        let bad = read_wav("testdata/int32.wav", 1);
        assert!(bad.is_err());
    }
}
