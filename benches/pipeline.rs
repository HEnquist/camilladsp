extern crate criterion;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
extern crate camillalib;

use camillalib::ProcessingParameters;
use camillalib::audiochunk::AudioChunk;
use camillalib::config;
use camillalib::pipeline::Pipeline;
use std::collections::HashMap;
use std::sync::Arc;

const CHUNK_SIZE: usize = 1024;
const CONV_LENGTHS: [usize; 2] = [32768, 65536];
const PRE_BIQUAD_PARAMS: [(f64, f64); 16] = [
    (120.0, 0.70),
    (220.0, 0.75),
    (350.0, 0.80),
    (500.0, 0.90),
    (700.0, 1.00),
    (900.0, 1.10),
    (1200.0, 0.95),
    (1600.0, 1.05),
    (1800.0, 1.10),
    (2200.0, 0.90),
    (2800.0, 0.95),
    (3200.0, 1.00),
    (3800.0, 0.85),
    (4500.0, 0.80),
    (6200.0, 0.75),
    (8000.0, 0.70),
];
const POST_BIQUAD_PARAMS: [(f64, f64); 16] = [
    (140.0, 0.72),
    (260.0, 0.78),
    (400.0, 0.83),
    (560.0, 0.92),
    (760.0, 1.02),
    (980.0, 1.08),
    (1300.0, 0.98),
    (1700.0, 1.06),
    (2100.0, 1.00),
    (2500.0, 0.94),
    (3000.0, 0.92),
    (3600.0, 0.88),
    (4200.0, 0.84),
    (5200.0, 0.80),
    (6800.0, 0.76),
    (9200.0, 0.72),
];

fn build_biquad_filter(freq: f64, q: f64) -> config::Filter {
    config::Filter::Biquad {
        description: None,
        parameters: config::BiquadParameters::Peaking(config::PeakingWidth::Q {
            freq,
            q,
            gain: 1.5,
        }),
    }
}

fn build_conv_filter(length: usize) -> config::Filter {
    let mut values = Vec::with_capacity(length);
    for idx in 0..length {
        let x = idx as f64 - (length as f64 - 1.0) * 0.5;
        let sinc = if x == 0.0 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        };
        values.push(sinc as camillalib::PrcFmt);
    }
    config::Filter::Conv {
        description: None,
        parameters: config::ConvParameters::Values { values },
    }
}

fn build_pipeline(chunksize: usize, multithreaded: bool, with_conv: bool) -> Pipeline {
    let mut filters = HashMap::new();
    let extra_filters = if with_conv { CONV_LENGTHS.len() } else { 0 };
    let mut pre_filter_names = Vec::with_capacity(PRE_BIQUAD_PARAMS.len() + extra_filters);
    let mut post_filter_names = Vec::with_capacity(POST_BIQUAD_PARAMS.len() + extra_filters);
    for (index, (freq, q)) in PRE_BIQUAD_PARAMS.iter().enumerate() {
        let name = format!("pre_bq_{}", index + 1);
        filters.insert(name.clone(), build_biquad_filter(*freq, *q));
        pre_filter_names.push(name);
    }
    for (index, (freq, q)) in POST_BIQUAD_PARAMS.iter().enumerate() {
        let name = format!("post_bq_{}", index + 1);
        filters.insert(name.clone(), build_biquad_filter(*freq, *q));
        post_filter_names.push(name);
    }

    if with_conv {
        filters.insert("pre_conv_1".to_string(), build_conv_filter(CONV_LENGTHS[0]));
        filters.insert("pre_conv_2".to_string(), build_conv_filter(CONV_LENGTHS[1]));
        filters.insert("post_conv_1".to_string(), build_conv_filter(CONV_LENGTHS[0]));
        filters.insert("post_conv_2".to_string(), build_conv_filter(CONV_LENGTHS[1]));
        pre_filter_names.push("pre_conv_1".to_string());
        pre_filter_names.push("pre_conv_2".to_string());
        post_filter_names.push("post_conv_1".to_string());
        post_filter_names.push("post_conv_2".to_string());
    }

    let mixer = config::Mixer {
        description: None,
        channels: config::MixerChannels { r#in: 4, out: 2 },
        mapping: vec![
            config::MixerMapping {
                dest: 0,
                sources: vec![
                    config::MixerSource {
                        channel: 0,
                        gain: Some(0.0),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                    config::MixerSource {
                        channel: 2,
                        gain: Some(-6.0),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                ],
                mute: Some(false),
            },
            config::MixerMapping {
                dest: 1,
                sources: vec![
                    config::MixerSource {
                        channel: 1,
                        gain: Some(0.0),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                    config::MixerSource {
                        channel: 3,
                        gain: Some(-6.0),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                ],
                mute: Some(false),
            },
        ],
        labels: None,
    };

    let mut mixers = HashMap::new();
    mixers.insert("mix_4_to_2".to_string(), mixer);

    let conf = config::Configuration {
        title: None,
        description: None,
        devices: config::Devices {
            samplerate: 48000,
            chunksize,
            queuelimit: None,
            silence_threshold: None,
            silence_timeout: None,
            capture: config::CaptureDevice::Stdin(config::CaptureDeviceStdin {
                channels: 4,
                format: config::BinarySampleFormat::F32_LE,
                extra_samples: None,
                skip_bytes: None,
                read_bytes: None,
                labels: None,
            }),
            playback: config::PlaybackDevice::Stdout {
                channels: 2,
                format: config::BinarySampleFormat::F32_LE,
                wav_header: None,
            },
            enable_rate_adjust: None,
            target_level: None,
            adjust_period: None,
            resampler: None,
            capture_samplerate: None,
            stop_on_rate_change: None,
            rate_measure_interval: None,
            volume_ramp_time: None,
            volume_limit: None,
            multithreaded: Some(multithreaded),
            worker_threads: None,
        },
        mixers: Some(mixers),
        filters: Some(filters),
        processors: None,
        pipeline: Some(vec![
            config::PipelineStep::Filter(config::PipelineStepFilter {
                channels: None,
                names: pre_filter_names,
                description: None,
                bypassed: Some(false),
            }),
            config::PipelineStep::Mixer(config::PipelineStepMixer {
                name: "mix_4_to_2".to_string(),
                description: None,
                bypassed: Some(false),
            }),
            config::PipelineStep::Filter(config::PipelineStepFilter {
                channels: None,
                names: post_filter_names,
                description: None,
                bypassed: Some(false),
            }),
        ]),
    };

    let processing_params = Arc::new(ProcessingParameters::new(&[0.0; 5], &[false; 5]));
    Pipeline::from_config(conf, processing_params)
}

fn make_chunk(channels: usize, frames: usize) -> AudioChunk {
    let mut waveforms = Vec::with_capacity(channels);
    for channel in 0..channels {
        let mut waveform = Vec::with_capacity(frames);
        for frame in 0..frames {
            let phase = (frame as f64 + channel as f64 * 13.0) * 0.013;
            waveform.push(phase.sin() as camillalib::PrcFmt);
        }
        waveforms.push(waveform);
    }
    AudioChunk::new(waveforms, 0.0, 0.0, frames, frames)
}

fn bench_complete_pipeline(c: &mut Criterion) {
    let variants = [
        ("biquad_single", false, false),
        ("biquad_multi", true, false),
        ("biquad_conv_single", false, true),
        ("biquad_conv_multi", true, true),
    ];

    let mut group = c.benchmark_group("complete_pipeline_chunk");
    for (name, multithreaded, with_conv) in variants {
        let mut pipeline = build_pipeline(CHUNK_SIZE, multithreaded, with_conv);
        group.bench_with_input(BenchmarkId::new("variant", name), &name, |b, _| {
            b.iter_batched(
                || make_chunk(4, CHUNK_SIZE),
                |chunk| {
                    let _out = pipeline.process_chunk(chunk);
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, bench_complete_pipeline);
criterion_main!(benches);
