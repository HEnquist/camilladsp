use crate::audiodevice::AudioChunk;
use crate::basicfilters;
use crate::biquad;
use crate::biquadcombo;
use crate::compressor;
use crate::config;
use crate::conversions;
use crate::diffeq;
use crate::dither;
#[cfg(not(feature = "FFTW"))]
use crate::fftconv;
#[cfg(feature = "FFTW")]
use crate::fftconv_fftw as fftconv;
use crate::limiter;
use crate::loudness;
use crate::mixer;
use rawsample::SampleReader;
use std::collections::HashMap;
use std::convert::TryInto;
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Instant;

use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::Res;

/// Windows Guid
/// Used to give sample format in the extended WAVEFORMATEXTENSIBLE wav header
#[derive(Debug, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    fn from_slice(data: &[u8; 16]) -> Guid {
        let data1 = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let data2 = u16::from_le_bytes(data[4..6].try_into().unwrap());
        let data3 = u16::from_le_bytes(data[6..8].try_into().unwrap());
        let data4 = data[8..16].try_into().unwrap();
        Guid {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_FLOAT: Guid = Guid {
    data1: 3,
    data2: 0,
    data3: 16,
    data4: [128, 0, 0, 170, 0, 56, 155, 113],
};

/// KSDATAFORMAT_SUBTYPE_PCM
const SUBTYPE_PCM: Guid = Guid {
    data1: 1,
    data2: 0,
    data3: 16,
    data4: [128, 0, 0, 170, 0, 56, 155, 113],
};

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
            let msg = format!("Could not open coefficient file '{filename}'. Error: {err}");
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
        // All other formats
        _ => {
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let rawformat = conversions::map_file_formats(format);
            let mut nextvalue = vec![0.0; 1];
            let nbr_coeffs = read_bytes_lines / format.bytes_per_sample();
            while let Ok(1) = PrcFmt::read_samples(&mut file, &mut nextvalue, &rawformat) {
                coefficients.push(nextvalue[0]);
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
        let msg = format!("Invalid wav header in file '{filename}'");
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
            let mut data = vec![0; chunk_length as usize];
            let _ = file.read(&mut data).unwrap();
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
                (0xFFFE, _, _) => {
                    // waveformatex
                    if chunk_length != 40 {
                        let msg = format!("Invalid extended header of wav file '{filename}'");
                        return Err(config::ConfigError::new(&msg).into());
                    }
                    let cb_size = u16::from_le_bytes(data[16..18].try_into().unwrap());
                    let valid_bits_per_sample =
                        u16::from_le_bytes(data[18..20].try_into().unwrap());
                    let channel_mask = u32::from_le_bytes(data[20..24].try_into().unwrap());
                    let subformat = &data[24..40];
                    let subformat_guid = Guid::from_slice(subformat.try_into().unwrap());
                    trace!(
                        "Found extended wav fmt chunk: subformatcode: {:?}, cb_size: {}, channel_mask: {}, valid bits per sample: {}",
                        subformat_guid, cb_size, channel_mask, valid_bits_per_sample
                    );
                    match (
                        subformat_guid,
                        bits,
                        bytes_per_sample,
                        valid_bits_per_sample,
                    ) {
                        (SUBTYPE_PCM, 16, 2, 16) => config::FileFormat::S16LE,
                        (SUBTYPE_PCM, 24, 3, 24) => config::FileFormat::S24LE3,
                        (SUBTYPE_PCM, 24, 4, 24) => config::FileFormat::S24LE,
                        (SUBTYPE_PCM, 32, 4, 32) => config::FileFormat::S32LE,
                        (SUBTYPE_FLOAT, 32, 4, 32) => config::FileFormat::FLOAT32LE,
                        (SUBTYPE_FLOAT, 64, 8, 64) => config::FileFormat::FLOAT64LE,
                        (_, _, _, _) => {
                            let msg =
                                format!("Unsupported extended wav format of file '{filename}'");
                            return Err(config::ConfigError::new(&msg).into());
                        }
                    }
                }
                (_, _, _) => {
                    let msg = format!("Unsupported wav format of file '{filename}'");
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
    let msg = format!("Unable to parse wav file '{filename}'");
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
        names: &[String],
        filter_configs: HashMap<String, config::Filter>,
        waveform_length: usize,
        sample_freq: usize,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        debug!("Build filter group from config");
        let mut filters = Vec::<Box<dyn Filter>>::new();
        for name in names {
            let filter_cfg = filter_configs[name].clone();
            trace!("Create filter {} with config {:?}", name, filter_cfg);
            let filter: Box<dyn Filter> =
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

/// A Pipeline is made up of a series of PipelineSteps,
/// each one can be a single Mixer of a group of Filters
pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
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
                        let mixer = mixer::Mixer::from_config(step.name, mixconf);
                        steps.push(PipelineStep::MixerStep(mixer));
                    }
                }
                config::PipelineStep::Filter(step) => {
                    if !step.is_bypassed() {
                        let step_channels = if let Some(channels) = &step.channels {
                            channels.clone()
                        } else {
                            (0..num_channels).collect()
                        };
                        for channel in step_channels {
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
                        let procconf = conf.processors.as_ref().unwrap()[&step.name].clone();
                        let proc = match procconf {
                            config::Processor::Compressor { parameters, .. } => {
                                let comp = compressor::Compressor::from_config(
                                    &step.name,
                                    parameters,
                                    conf.devices.samplerate,
                                    conf.devices.chunksize,
                                );
                                Box::new(comp)
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
            current_volume,
            mute,
            conf.devices.chunksize,
            conf.devices.samplerate,
            processing_params.clone(),
            0,
        );
        let secs_per_chunk = conf.devices.chunksize as f32 / conf.devices.samplerate as f32;
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
                    chunk = mix.process_chunk(&chunk);
                }
                PipelineStep::FilterStep(flt) => {
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
    use crate::filters::{find_data_in_wav, read_wav};
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
    pub fn test_analyze_wav() {
        let info = find_data_in_wav("testdata/int32.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, FileFormat::S32LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
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
