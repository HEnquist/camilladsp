use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::time::Duration;

use camillalib::CamillaFloat;
use camillalib::ProcessingParameters;
use camillalib::audiochunk::AudioChunk;
use camillalib::config;
use camillalib::pipeline::Pipeline;
use std::collections::HashMap;
use std::sync::Arc;

const CHUNK_SIZE: usize = 1024;
const CONV_LENGTHS: [usize; 2] = [32768, 65536];
/// A million taps, so the convolutions dominate the pipeline completely. This
/// is the case the thread pool exists for.
const LONG_CONV_LENGTHS: [usize; 1] = [1048576];

/// How much FIR filtering a pipeline variant carries.
#[derive(Clone, Copy, PartialEq)]
enum Conv {
    None,
    Normal,
    Long,
}

impl Conv {
    fn lengths(self) -> &'static [usize] {
        match self {
            Conv::None => &[],
            Conv::Normal => &CONV_LENGTHS,
            Conv::Long => &LONG_CONV_LENGTHS,
        }
    }
}
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
    let pi = std::f64::consts::PI;
    for idx in 0..length {
        let x = idx as f64 - (length as f64 - 1.0) * 0.5;
        let sinc = if x == 0.0 {
            1.0
        } else {
            (pi * x).sin() / (pi * x)
        };
        values.push(sinc);
    }
    config::Filter::Conv {
        description: None,
        parameters: config::ConvParameters::Values { values },
    }
}

fn build_pipeline(chunksize: usize, multithreaded: bool, conv: Conv) -> Pipeline {
    let mut filters = HashMap::new();
    let conv_lengths = conv.lengths();
    let extra_filters = conv_lengths.len();
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

    for (index, length) in conv_lengths.iter().enumerate() {
        let pre = format!("pre_conv_{}", index + 1);
        let post = format!("post_conv_{}", index + 1);
        filters.insert(pre.clone(), build_conv_filter(*length));
        filters.insert(post.clone(), build_conv_filter(*length));
        pre_filter_names.push(pre);
        post_filter_names.push(post);
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
            silence_timeout_s: None,
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
            adjust_interval_s: None,
            resampler: None,
            capture_samplerate: None,
            stop_on_rate_change: None,
            rate_measure_interval_s: None,
            volume_ramp_time_ms: None,
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

    let processing_params = Arc::new(ProcessingParameters::new(&[0.0_f32; 5], &[false; 5]));
    let filter_pool = camillalib::processing::build_processing_threadpool(
        multithreaded,
        conf.devices.worker_threads(),
        conf.devices.chunksize,
        conf.devices.samplerate,
    );
    Pipeline::from_config(conf, processing_params, filter_pool)
}

fn make_chunk(channels: usize, frames: usize) -> AudioChunk {
    let mut waveforms = Vec::with_capacity(channels);
    for channel in 0..channels {
        let mut waveform = Vec::with_capacity(frames);
        for frame in 0..frames {
            let phase = (frame as CamillaFloat + channel as CamillaFloat * 13.0) * 0.013;
            waveform.push(phase.sin());
        }
        waveforms.push(waveform);
    }
    AudioChunk::new(
        waveforms,
        0.0 as CamillaFloat,
        0.0 as CamillaFloat,
        frames,
        frames,
    )
}

fn bench_complete_pipeline(c: &mut Criterion) {
    let variants = [
        ("biquad_single", false, Conv::None),
        ("biquad_multi", true, Conv::None),
        ("biquad_conv_single", false, Conv::Normal),
        ("biquad_conv_multi", true, Conv::Normal),
        // Convolution-dominated: this is where the thread pool should pay off.
        ("long_fir_single", false, Conv::Long),
        ("long_fir_multi", true, Conv::Long),
    ];

    let mut group = c.benchmark_group("complete_pipeline_chunk");
    for (name, multithreaded, conv) in variants {
        // The million-tap variants take milliseconds per chunk, so fewer
        // samples keeps the bench to a sensible runtime.
        let mut pipeline = build_pipeline(CHUNK_SIZE, multithreaded, conv);
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

/// One chunk through this pipeline takes between 90 us and 6 ms, so criterion's
/// default 3 s warm-up and 5 s measurement spend nearly all of their time
/// repeating a reading that stabilised immediately. Criterion budgets by wall
/// time, not by iteration count, so the defaults cost the same 8 s per variant
/// whatever the variant costs.
///
/// `sample_size` stays high because it is nearly free: criterion budgets by
/// wall time, so with fast iterations it just packs more of them into each
/// sample. It only costs time once one sample per iteration would overrun the
/// budget, which at 6 ms and 50 samples it does not.
///
/// Micro-benchmarks measuring individual kernels should keep the defaults;
/// at nanosecond scale the long budgets are doing real work.
fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50)
        .without_plots()
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_complete_pipeline
}
criterion_main!(benches);
