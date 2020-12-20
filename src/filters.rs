use audiodevice::AudioChunk;
use basicfilters;
use biquad;
use biquadcombo;
use config;
use diffeq;
use dither;
#[cfg(not(feature = "FFTW"))]
use fftconv;
#[cfg(feature = "FFTW")]
use fftconv_fftw as fftconv;
use mixer;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::{Arc, RwLock};

use PrcFmt;
use ProcessingStatus;
use Res;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()>;

    fn update_parameters(&mut self, config: config::Filter);

    fn name(&self) -> String;
}

pub fn pad_vector(values: &[PrcFmt], length: usize) -> Vec<PrcFmt> {
    let new_len = if values.len() > length {
        values.len()
    } else {
        length
    };
    let mut new_values: Vec<PrcFmt> = vec![0.0; new_len];
    new_values[0..values.len()].copy_from_slice(&values[..]);
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
            let msg = format!(
                "Could not open coefficient file '{}'. Error: {}",
                filename, err
            );
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
                            "Can't read line {} of file '{}'. Error: {}",
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
                                "Can't parse value on line {} of file '{}'. Error: {}",
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
        config::FileFormat::FLOAT32LE => {
            let mut buffer = [0; 4];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 4;
            while let Ok(4) = file.read(&mut buffer) {
                let value = f32::from_le_bytes(buffer) as PrcFmt;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
            }
        }
        config::FileFormat::FLOAT64LE => {
            let mut buffer = [0; 8];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 8;
            while let Ok(8) = file.read(&mut buffer) {
                let value = f64::from_le_bytes(buffer) as PrcFmt;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
            }
        }
        config::FileFormat::S16LE => {
            let mut buffer = [0; 2];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 2;
            let scalefactor = (2.0 as PrcFmt).powi(15);
            while let Ok(2) = file.read(&mut buffer) {
                let mut value = i16::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
            }
        }
        config::FileFormat::S24LE => {
            let mut buffer = [0; 4];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 4;
            let scalefactor = (2.0 as PrcFmt).powi(23);
            while let Ok(4) = file.read(&mut buffer) {
                let mut value = i32::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
            }
        }
        config::FileFormat::S24LE3 => {
            let mut buffer = [0; 4];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 3;
            let scalefactor = (2.0 as PrcFmt).powi(23);
            while let Ok(3) = file.read(&mut buffer[0..3]) {
                let mut value = i32::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
            }
        }
        config::FileFormat::S32LE => {
            let mut buffer = [0; 4];
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / 4;
            let scalefactor = (2.0 as PrcFmt).powi(31);
            while let Ok(4) = file.read(&mut buffer) {
                let mut value = i32::from_le_bytes(buffer) as PrcFmt;
                value /= scalefactor;
                coefficients.push(value);
                if coefficients.len() >= nbr_coeffs {
                    break;
                }
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
        processing_status: Arc<RwLock<ProcessingStatus>>,
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
                    config::Filter::BiquadCombo { parameters } => Box::new(
                        biquadcombo::BiquadCombo::from_config(name, sample_freq, parameters),
                    ),
                    config::Filter::Delay { parameters } => Box::new(
                        basicfilters::Delay::from_config(name, sample_freq, parameters),
                    ),
                    config::Filter::Gain { parameters } => {
                        Box::new(basicfilters::Gain::from_config(name, parameters))
                    }
                    config::Filter::Volume { parameters } => {
                        Box::new(basicfilters::Volume::from_config(
                            name,
                            parameters,
                            waveform_length,
                            sample_freq,
                            processing_status.clone(),
                        ))
                    }
                    config::Filter::Dither { parameters } => {
                        Box::new(dither::Dither::from_config(name, parameters))
                    }
                    config::Filter::DiffEq { parameters } => {
                        Box::new(diffeq::DiffEq::from_config(name, parameters))
                    }
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
        if !input.waveforms[self.channel].is_empty() {
            for filter in &mut self.filters {
                filter.process_waveform(&mut input.waveforms[self.channel])?;
            }
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
    pub fn from_config(
        conf: config::Configuration,
        processing_status: Arc<RwLock<ProcessingStatus>>,
    ) -> Self {
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
                        conf.devices.chunksize,
                        conf.devices.samplerate,
                        processing_status.clone(),
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
                return Err(config::ConfigError::new("Unstable filter specified").into());
            }
            Ok(())
        }
        config::Filter::Delay { parameters } => {
            if parameters.delay < 0.0 {
                return Err(config::ConfigError::new("Negative delay specified").into());
            }
            Ok(())
        }
        config::Filter::Gain { .. } => Ok(()),
        config::Filter::Dither { .. } => Ok(()),
        config::Filter::DiffEq { .. } => Ok(()),
        config::Filter::Volume { .. } => Ok(()),
        config::Filter::BiquadCombo { parameters } => biquadcombo::validate_config(&parameters),
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use config::FileFormat;
    use filters::{pad_vector, read_coeff_file};

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {}", left, right);
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: &[PrcFmt], right: &[PrcFmt], maxdiff: PrcFmt) -> bool {
        if left.len() != right.len() {
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
        assert!(compare_waveforms(&loaded, &expected, 1e-15));
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileFormat::FLOAT32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-15));
    }

    #[test]
    fn read_float64() {
        let loaded = read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&loaded, &expected, 1e-15));
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 24, 8).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-15));
    }

    #[test]
    fn read_int16() {
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&loaded, &expected, 1e-4));
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 6, 2).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-4));
    }

    #[test]
    fn read_int24() {
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&loaded, &expected, 1e-6));
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-6));
    }
    #[test]
    fn read_int32() {
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&loaded, &expected, 1e-9));
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-9));
    }
    #[test]
    fn read_text() {
        let loaded = read_coeff_file("testdata/text.txt", &FileFormat::TEXT, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&loaded, &expected, 1e-9));
        let loaded = read_coeff_file("testdata/text_header.txt", &FileFormat::TEXT, 4, 1).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5];
        assert!(compare_waveforms(&loaded, &expected, 1e-9));
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
}
