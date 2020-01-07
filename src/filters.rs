use std::io::BufReader;
use std::io::BufRead;
use std::fs::File;
//use std::path::Path;
use std::error;
use std::collections::HashMap;
use config;
use audiodevice::AudioChunk;
use fftconv;
use biquad;
use mixer;

pub type Res<T> = Result<T, Box<dyn error::Error>>;

// Traits etc for filters
// Sample format
//type SmpFmt = i16;
type PrcFmt = f64;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()>;
}

pub fn read_coeff_file(filename: &str) -> Res<Vec<PrcFmt>> {
    let mut coefficients = Vec::<PrcFmt>::new();
    let f = File::open(filename).unwrap();
    let file = BufReader::new(&f);
    for line in file.lines() {
        let l = line.unwrap();
        coefficients.push(l.parse().unwrap());
        
    }
    println!("{:?}", coefficients); 
    Ok(coefficients)
}

fn get_coefficients(coeffs: &config::FilterCoefficients) -> Vec<PrcFmt> {
    match coeffs {
        config::FilterCoefficients::Values{values} => values.clone(),
        config::FilterCoefficients::File{values} => read_coeff_file(&values).unwrap(),
    }
}


pub struct FilterGroup {
    channel: usize,
    filters: Vec<Box<dyn Filter>>,
}

impl FilterGroup {
    /// Creates a group of filters to process a chunk
    pub fn from_config(channel: usize, names: Vec<String>, filter_configs: HashMap<String, config::Filter>, waveform_length: usize) -> Self {
        let mut filters = Vec::<Box<dyn Filter>>::new();
        for name in names {
            let filter_cfg = &filter_configs[&name];
            let filter: Box<dyn Filter> = match filter_cfg.r#type {
                config::FilterType::Conv => {
                    let coeffs = get_coefficients(&filter_cfg.coefficients);
                    Box::new(fftconv::FFTConv::new(waveform_length, &coeffs))
                },
                config::FilterType::Biquad => {
                    let coeffs = get_coefficients(&filter_cfg.coefficients);
                    Box::new(biquad::Biquad::new(biquad::BiquadCoefficients::from_vec(&coeffs)))
                },
                _ => panic!("unknown type")
            };
            filters.push(filter);
        }
        FilterGroup {
            channel: channel,
            filters: filters,
        }

    }

    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        for filter in &mut self.filters {
            filter.process_waveform(&mut input.waveforms[self.channel])?;
        }
        Ok(())
    }
}

pub enum PipelineStep {
    MixerStep(mixer::Mixer),
    FilterStep(FilterGroup),
}
pub struct Pipeline {
    steps: Vec<PipelineStep>,
}

impl Pipeline {
    pub fn from_config(conf: config::Configuration) -> Self {
        let mut steps = Vec::<PipelineStep>::new();
        for step in conf.pipeline {
            match step {
                config::PipelineStep::Mixer{name} => {
                    let mixconf = conf.mixers[&name].clone();
                    let mixer = mixer::Mixer::from_config(mixconf);
                    steps.push(PipelineStep::MixerStep(mixer));
                }
                config::PipelineStep::Filter{channel, names} => {
                    let fltgrp = FilterGroup::from_config(channel, names, conf.filters.clone(), conf.devices.buffersize);
                    steps.push(PipelineStep::FilterStep(fltgrp));
                }
            }
        }
        Pipeline {
            steps: steps,
        }
    }

//pub enum PipelineStep {
    //Mixer { name: String },
    //Filter { channel: usize, names: Vec<String>}

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