use audiodevice::AudioChunk;
use basicfilters;
use biquad;
use config;
use dither;
use fftconv;
use mixer;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Read};

use PrcFmt;
use Res;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()>;

    fn update_parameters(&mut self, config: config::Filter) -> ();

    fn name(&self) -> String;
}

pub fn read_coeff_file(filename: &str, format: &config::FileFormat) -> Res<Vec<PrcFmt>> {
    let mut coefficients = Vec::<PrcFmt>::new();
    let f = File::open(filename)?;
    let mut file = BufReader::new(&f);
    match format {
        config::FileFormat::TEXT => {
            for line in file.lines() {
                let l = line?;
                coefficients.push(l.trim().parse()?);
            }
        }
        config::FileFormat::FLOAT32LE => {
            let mut buffer = [0; 4];
            while let Ok(4) = file.read(&mut buffer) {
                let value = f32::from_le_bytes(buffer) as PrcFmt;
                coefficients.push(value);
            }
        }
        config::FileFormat::FLOAT64LE => {
            let mut buffer = [0; 8];
            while let Ok(4) = file.read(&mut buffer) {
                let value = f64::from_le_bytes(buffer) as PrcFmt;
                coefficients.push(value);
            }
        }
        config::FileFormat::S16LE => {
            let mut buffer = [0; 2];
            let scalefactor = (2.0 as PrcFmt).powi(15);
            while let Ok(4) = file.read(&mut buffer) {
                let mut value = i16::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
            }
        }
        config::FileFormat::S24LE => {
            let mut buffer = [0; 4];
            let scalefactor = (2.0 as PrcFmt).powi(23);
            while let Ok(4) = file.read(&mut buffer) {
                let mut value = i32::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
            }
        }
        config::FileFormat::S32LE => {
            let mut buffer = [0; 4];
            let scalefactor = (2.0 as PrcFmt).powi(31);
            while let Ok(4) = file.read(&mut buffer) {
                let mut value = i32::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
            }
        }
    }
    debug!(
        "Read file: {}, number of coeffs: {}",
        filename,
        coefficients.len()
    );
    Ok(coefficients)
}

pub struct FilterGroup {
    channel: usize,
    filters: Vec<Box<dyn Filter>>,
}

impl FilterGroup {
    /// Creates a group of filters to process a chunk.
    pub fn from_config(
        channel: usize,
        names: Vec<String>,
        filter_configs: HashMap<String, config::Filter>,
        waveform_length: usize,
        sample_freq: usize,
    ) -> Self {
        debug!("Build from config");
        let mut filters = Vec::<Box<dyn Filter>>::new();
        for name in names {
            let filter_cfg = filter_configs[&name].clone();
            let filter: Box<dyn Filter> =
                match filter_cfg {
                    config::Filter::Conv { parameters } => Box::new(fftconv::FFTConv::from_config(
                        name,
                        waveform_length,
                        parameters,
                    )),
                    config::Filter::Biquad { parameters } => Box::new(biquad::Biquad::new(
                        name,
                        sample_freq,
                        biquad::BiquadCoefficients::from_config(sample_freq, parameters),
                    )),
                    config::Filter::Delay { parameters } => Box::new(
                        basicfilters::Delay::from_config(name, sample_freq, parameters),
                    ),
                    config::Filter::Gain { parameters } => {
                        Box::new(basicfilters::Gain::from_config(name, parameters))
                    }
                    config::Filter::Dither { parameters } => {
                        Box::new(dither::Dither::from_config(name, parameters))
                    } //_ => panic!("unknown type")
                };
            filters.push(filter);
        }
        FilterGroup { channel, filters }
    }

    pub fn update_parameters(
        &mut self,
        filterconfigs: HashMap<String, config::Filter>,
        changed: Vec<String>,
    ) {
        for filter in &mut self.filters {
            if changed.iter().any(|n| n == &filter.name()) {
                filter.update_parameters(filterconfigs[&filter.name()].clone());
            }
        }
    }

    /// Apply all the filters to an AudioChunk.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        for filter in &mut self.filters {
            filter.process_waveform(&mut input.waveforms[self.channel])?;
        }
        Ok(())
    }
}

/// A Pipeline is made up of a series of PipelineSteps,
/// each one can be a single Mixer of a group of Filters
pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
}

pub struct Pipeline {
    steps: Vec<PipelineStep>,
}

impl Pipeline {
    /// Create a new pipeline from a configuration structure.
    pub fn from_config(conf: config::Configuration) -> Self {
        debug!("Build new pipeline");
        let mut steps = Vec::<PipelineStep>::new();
        for step in conf.pipeline {
            match step {
                config::PipelineStep::Mixer { name } => {
                    let mixconf = conf.mixers[&name].clone();
                    let mixer = mixer::Mixer::from_config(name, mixconf);
                    steps.push(PipelineStep::MixerStep(mixer));
                }
                config::PipelineStep::Filter { channel, names } => {
                    let fltgrp = FilterGroup::from_config(
                        channel,
                        names,
                        conf.filters.clone(),
                        conf.devices.buffersize,
                        conf.devices.samplerate,
                    );
                    steps.push(PipelineStep::FilterStep(fltgrp));
                }
            }
        }
        Pipeline { steps }
    }

    pub fn update_parameters(
        &mut self,
        conf: config::Configuration,
        filters: Vec<String>,
        mixers: Vec<String>,
    ) {
        debug!("Updating parameters");
        for mut step in &mut self.steps {
            match &mut step {
                PipelineStep::MixerStep(mix) => {
                    if mixers.iter().any(|n| n == &mix.name) {
                        mix.update_parameters(conf.mixers[&mix.name].clone());
                    }
                }
                PipelineStep::FilterStep(flt) => {
                    flt.update_parameters(conf.filters.clone(), filters.clone());
                }
            }
        }
    }

    /// Process an AudioChunk by calling either a MixerStep or a FilterStep
    pub fn process_chunk(&mut self, mut chunk: AudioChunk) -> AudioChunk {
        for mut step in &mut self.steps {
            match &mut step {
                PipelineStep::MixerStep(mix) => {
                    chunk = mix.process_chunk(&chunk);
                }
                PipelineStep::FilterStep(flt) => {
                    flt.process_chunk(&mut chunk).unwrap();
                }
            }
        }
        chunk
    }
}

/// Validate the filter config, to give a helpful message intead of a panic.
pub fn validate_filter(fs: usize, filter_config: &config::Filter) -> Res<()> {
    match filter_config {
        config::Filter::Conv { parameters } => fftconv::validate_config(&parameters),
        config::Filter::Biquad { parameters } => {
            let coeffs = biquad::BiquadCoefficients::from_config(fs, parameters.clone());
            if !coeffs.is_stable() {
                return Err(Box::new(config::ConfigError::new(
                    "Unstable filter specified",
                )));
            }
            Ok(())
        }
        config::Filter::Delay { parameters } => {
            if parameters.delay < 0.0 {
                return Err(Box::new(config::ConfigError::new(
                    "Negative delay specified",
                )));
            }
            Ok(())
        }
        config::Filter::Gain { .. } => Ok(()),
        config::Filter::Dither { .. } => Ok(()), //_ => panic!("unknown type")
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use config::FileFormat;
    use filters::read_coeff_file;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn read_float32() {
        let loaded = read_coeff_file("testdata/float32.raw", &FileFormat::FLOAT32LE).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(loaded, expected, 1e-15));
    }

    #[test]
    fn read_float64() {
        let loaded = read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(loaded, expected, 1e-15));
    }

    #[test]
    fn read_int16() {
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(loaded, expected, 1e-15));
    }

    #[test]
    fn read_int24() {
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(loaded, expected, 1e-6));
    }
    #[test]
    fn read_int32() {
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(loaded, expected, 1e-9));
    }
}
