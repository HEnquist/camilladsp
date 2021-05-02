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
use loudness;
use mixer;
use std::collections::HashMap;
use std::convert::TryInto;
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::{Arc, RwLock};

use NewValue;
use PrcFmt;
use ProcessingStatus;
use Res;

#[derive(Debug)]
pub struct WavParams {
    sample_format: config::FileFormat,
    sample_rate: usize,
    data_offset: usize,
    data_length: usize,
    channels: usize,
}

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
    new_values[0..values.len()].copy_from_slice(&values);
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
            let scalefactor = PrcFmt::new(2.0).powi(15);
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
            let scalefactor = PrcFmt::new(2.0).powi(31);
            while let Ok(4) = file.read(&mut buffer) {
                buffer[3] = buffer[2];
                buffer[2] = buffer[1];
                buffer[1] = buffer[0];
                buffer[0] = 0;
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
            let scalefactor = PrcFmt::new(2.0).powi(31);
            while let Ok(3) = file.read(&mut buffer[1..4]) {
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
            let scalefactor = PrcFmt::new(2.0).powi(31);
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
        "Read raw data from: '{}', format: {:?}, number of coeffs: {}",
        filename,
        format,
        coefficients.len()
    );
    Ok(coefficients)
}

pub fn find_data_in_wav(filename: &str) -> Res<WavParams> {
    let f = File::open(filename)?;
    let filesize = f.metadata()?.len();
    let mut file = BufReader::new(&f);
    let mut header = [0; 12];
    let _ = file.read(&mut header)?;

    let riff_b = "RIFF".as_bytes();
    let wave_b = "WAVE".as_bytes();
    let data_b = "data".as_bytes();
    let fmt_b = "fmt ".as_bytes();
    let riff_err = header.iter().take(4).zip(riff_b).any(|(a, b)| *a != *b);
    let wave_err = header
        .iter()
        .skip(8)
        .take(4)
        .zip(wave_b)
        .any(|(a, b)| *a != *b);
    if riff_err || wave_err {
        let msg = format!("Invalid wav header in file '{}'", filename);
        return Err(config::ConfigError::new(&msg).into());
    }
    let mut next_chunk_location = 12;
    let mut found_fmt = false;
    let mut found_data = false;
    let mut buffer = [0; 8];

    let mut sample_format = config::FileFormat::S16LE;
    let mut sample_rate = 0;
    let mut channels = 0;
    let mut data_offset = 0;
    let mut data_length = 0;

    while (!found_fmt || !found_data) && next_chunk_location < filesize {
        file.seek(SeekFrom::Start(next_chunk_location))?;
        let _ = file.read(&mut buffer)?;
        let chunk_length = u32::from_le_bytes(buffer[4..8].try_into().unwrap());
        trace!("Analyzing wav chunk of length: {}", chunk_length);
        let is_data = buffer.iter().take(4).zip(data_b).all(|(a, b)| *a == *b);
        let is_fmt = buffer.iter().take(4).zip(fmt_b).all(|(a, b)| *a == *b);
        if is_fmt && (chunk_length == 16 || chunk_length == 18 || chunk_length == 40) {
            found_fmt = true;
            let mut data = [0; 16];
            let _ = file.read(&mut data).unwrap();
            // 1: int, 3: float
            let formatcode = u16::from_le_bytes(data[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(data[2..4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(data[4..8].try_into().unwrap());
            let bytes_per_frame = u16::from_le_bytes(data[12..14].try_into().unwrap());
            let bits = u16::from_le_bytes(data[14..16].try_into().unwrap());
            let bytes_per_sample = bytes_per_frame / channels;
            sample_format = match (formatcode, bits, bytes_per_sample) {
                (1, 16, 2) => config::FileFormat::S16LE,
                (1, 24, 3) => config::FileFormat::S24LE3,
                (1, 24, 4) => config::FileFormat::S24LE,
                (1, 32, 4) => config::FileFormat::S32LE,
                (3, 32, 4) => config::FileFormat::FLOAT32LE,
                (3, 64, 8) => config::FileFormat::FLOAT64LE,
                _ => {
                    let msg = format!("Unsupported wav format of file '{}'", filename);
                    return Err(config::ConfigError::new(&msg).into());
                }
            };
            trace!(
                "Found wav fmt chunk: formatcode: {}, channels: {}, samplerate: {}, bits: {}, bytes_per_frame: {}",
                formatcode, channels, sample_rate, bits, bytes_per_frame
            );
        } else if is_data {
            found_data = true;
            data_offset = next_chunk_location + 8;
            data_length = chunk_length;
            trace!(
                "Found wav data chunk, start: {}, length: {}",
                data_offset,
                data_length
            )
        }
        next_chunk_location += 8 + chunk_length as u64;
    }
    if found_data && found_fmt {
        trace!("Wav file with parameters: format: {:?},  samplerate: {}, channels: {}, data_length: {}, data_offset: {}", sample_format, sample_rate, channels, data_length, data_offset);
        return Ok(WavParams {
            sample_format,
            sample_rate: sample_rate as usize,
            channels: channels as usize,
            data_length: data_length as usize,
            data_offset: data_offset as usize,
        });
    }
    let msg = format!("Unable to parse wav file '{}'", filename);
    Err(config::ConfigError::new(&msg).into())
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
        &params.sample_format,
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
                    config::Filter::Conv { parameters } => Box::new(fftconv::FftConv::from_config(
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
                    config::Filter::Loudness { parameters } => {
                        Box::new(loudness::Loudness::from_config(
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
        config::Filter::Biquad { parameters } => biquad::validate_config(fs, &parameters),
        config::Filter::Delay { parameters } => basicfilters::validate_delay_config(&parameters),
        config::Filter::Gain { parameters } => basicfilters::validate_gain_config(&parameters),
        config::Filter::Dither { parameters } => dither::validate_config(&parameters),
        config::Filter::DiffEq { parameters } => diffeq::validate_config(&parameters),
        config::Filter::Volume { parameters } => basicfilters::validate_volume_config(&parameters),
        config::Filter::Loudness { parameters } => loudness::validate_config(&parameters),
        config::Filter::BiquadCombo { parameters } => biquadcombo::validate_config(fs, &parameters),
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use config::FileFormat;
    use filters::{find_data_in_wav, read_wav};
    use filters::{pad_vector, read_coeff_file};

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {} = {}", left, right, left - right);
        let res = (left - right).abs() < maxdiff;
        println!("Ok: {}", res);
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
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileFormat::FLOAT32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }

    #[test]
    fn read_float64() {
        let loaded = read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileFormat::FLOAT64LE, 24, 8).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }

    #[test]
    fn read_int16() {
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded = read_coeff_file("testdata/int16.raw", &FileFormat::S16LE, 6, 2).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }

    #[test]
    fn read_int24() {
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded = read_coeff_file("testdata/int24.raw", &FileFormat::S24LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }
    #[test]
    fn read_int24_3() {
        let loaded = read_coeff_file("testdata/int243.raw", &FileFormat::S24LE3, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded = read_coeff_file("testdata/int243.raw", &FileFormat::S24LE3, 9, 3).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }
    #[test]
    fn read_int32() {
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded = read_coeff_file("testdata/int32.raw", &FileFormat::S32LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{:?} != {:?}",
            loaded,
            expected
        );
    }
    #[test]
    fn read_text() {
        let loaded = read_coeff_file("testdata/text.txt", &FileFormat::TEXT, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{:?} != {:?}",
            loaded,
            expected
        );
        let loaded = read_coeff_file("testdata/text_header.txt", &FileFormat::TEXT, 4, 1).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{:?} != {:?}",
            loaded,
            expected
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
    pub fn test_analyze_wav() {
        let info = find_data_in_wav("testdata/int32.wav").unwrap();
        println!("{:?}", info);
        assert_eq!(info.sample_format, FileFormat::S32LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
    }

    #[test]
    pub fn test_read_wav() {
        let values = read_wav("testdata/int32.wav", 0).unwrap();
        println!("{:?}", values);
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&values, &expected, 1e-9));
        let bad = read_wav("testdata/int32.wav", 1);
        assert!(bad.is_err());
    }
}
