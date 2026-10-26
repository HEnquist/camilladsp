use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::time::Duration;

use camilladsp::CamillaFloat;
use camilladsp::ProcessingParameters;
use camilladsp::audiochunk::AudioChunk;
use camilladsp::config;
use camilladsp::config::FiniteF64;
use camilladsp::filters::fftconv::ConvCoeffCache;
use camilladsp::pipeline::Pipeline;
use std::collections::HashMap;
use std::hint::black_box;
use std::num::NonZeroUsize;
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
            freq: config::FiniteF64::expect_finite(freq),
            q: config::FiniteF64::expect_finite(q),
            gain: config::FiniteF64::expect_finite(1.5),
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
        values.push(FiniteF64::expect_finite(sinc));
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
        channels: config::MixerChannels {
            r#in: NonZeroUsize::new(4).unwrap(),
            out: NonZeroUsize::new(2).unwrap(),
        },
        mapping: vec![
            config::MixerMapping {
                dest: 0,
                sources: vec![
                    config::MixerSource {
                        channel: 0,
                        gain: Some(FiniteF64::expect_finite(0.0)),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                    config::MixerSource {
                        channel: 2,
                        gain: Some(FiniteF64::expect_finite(-6.0)),
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
                        gain: Some(FiniteF64::expect_finite(0.0)),
                        inverted: Some(false),
                        mute: Some(false),
                        scale: Some(config::GainScale::Decibel),
                    },
                    config::MixerSource {
                        channel: 3,
                        gain: Some(FiniteF64::expect_finite(-6.0)),
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
            samplerate: NonZeroUsize::new(48000).unwrap(),
            chunksize: NonZeroUsize::new(chunksize).unwrap(),
            queuelimit: None,
            silence_threshold: None,
            silence_timeout_s: None,
            capture: config::CaptureDevice::Stdin(config::CaptureDeviceStdin {
                channels: NonZeroUsize::new(4).unwrap(),
                format: config::BinarySampleFormat::F32_LE,
                extra_samples: None,
                skip_bytes: None,
                read_bytes: None,
                labels: None,
            }),
            playback: config::PlaybackDevice::Stdout {
                channels: NonZeroUsize::new(2).unwrap(),
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
    let filter_pool = camilladsp::processing::build_processing_threadpool(
        multithreaded,
        conf.devices.worker_threads(),
        conf.devices.chunksize(),
        conf.devices.samplerate(),
    );
    Pipeline::from_config(
        conf,
        processing_params,
        filter_pool,
        &mut ConvCoeffCache::new(),
    )
}

/// A large multi-way active system: many channels, a handful of biquads on
/// each. The opposite shape to the parametric equaliser the bench above
/// measures, and the one the cascade axis alone can do almost nothing with,
/// since a three stage canon is only three recurrences wide.
///
/// It is here because measuring only the deep and narrow shape sizes the
/// kernel around the wrong axis.
const WIDE_CHANNELS: usize = 16;
const WIDE_BIQUAD_PARAMS: [(f64, f64); 3] = [(90.0, 0.71), (1800.0, 1.05), (7500.0, 0.75)];

fn build_wide_pipeline(chunksize: usize, multithreaded: bool) -> Pipeline {
    let mut filters = HashMap::new();
    let mut names = Vec::with_capacity(WIDE_BIQUAD_PARAMS.len());
    for (index, (freq, q)) in WIDE_BIQUAD_PARAMS.iter().enumerate() {
        let name = format!("wide_bq_{}", index + 1);
        filters.insert(name.clone(), build_biquad_filter(*freq, *q));
        names.push(name);
    }

    let conf = config::Configuration {
        title: None,
        description: None,
        devices: config::Devices {
            samplerate: NonZeroUsize::new(48000).unwrap(),
            chunksize: NonZeroUsize::new(chunksize).unwrap(),
            queuelimit: None,
            silence_threshold: None,
            silence_timeout_s: None,
            capture: config::CaptureDevice::Stdin(config::CaptureDeviceStdin {
                channels: NonZeroUsize::new(WIDE_CHANNELS).unwrap(),
                format: config::BinarySampleFormat::F32_LE,
                extra_samples: None,
                skip_bytes: None,
                read_bytes: None,
                labels: None,
            }),
            playback: config::PlaybackDevice::Stdout {
                channels: NonZeroUsize::new(WIDE_CHANNELS).unwrap(),
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
        mixers: None,
        filters: Some(filters),
        processors: None,
        pipeline: Some(vec![config::PipelineStep::Filter(
            config::PipelineStepFilter {
                channels: None,
                names,
                description: None,
                bypassed: Some(false),
            },
        )]),
    };

    let processing_params = Arc::new(ProcessingParameters::new(&[0.0_f32; 5], &[false; 5]));
    let filter_pool = camilladsp::processing::build_processing_threadpool(
        multithreaded,
        conf.devices.worker_threads(),
        conf.devices.chunksize(),
        conf.devices.samplerate(),
    );
    Pipeline::from_config(
        conf,
        processing_params,
        filter_pool,
        &mut ConvCoeffCache::new(),
    )
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
    for (name, multithreaded) in [("wide_single", false), ("wide_multi", true)] {
        let mut pipeline = build_wide_pipeline(CHUNK_SIZE, multithreaded);
        group.bench_with_input(BenchmarkId::new("variant", name), &name, |b, _| {
            b.iter_batched(
                || make_chunk(WIDE_CHANNELS, CHUNK_SIZE),
                |chunk| {
                    let _out = pipeline.process_chunk(chunk);
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn bench_processing_paths(c: &mut Criterion) {
    use serde_json::json;

    let filters = [
        ("gain", "Gain", json!({"gain": -6.0})),
        ("delay", "Delay", json!({"delay": 10.0, "delay_unit": "ms"})),
        (
            "delay_short",
            "Delay",
            json!({"delay": 1.0, "delay_unit": "samples"}),
        ),
        (
            "delay_long",
            "Delay",
            json!({"delay": 100.0, "delay_unit": "ms"}),
        ),
        (
            "fractional_delay",
            "Delay",
            json!({"delay": 10.01, "delay_unit": "ms", "subsample": true}),
        ),
        ("volume", "Volume", json!({"fader": "Aux1"})),
        ("volume_ramp", "Volume", json!({"fader": "Aux1"})),
        (
            "loudness",
            "Loudness",
            json!({"reference_level": 0.0, "fader": "Aux1"}),
        ),
        ("hard_clip", "Clipper", json!({"clip_limit": -6.0})),
        (
            "soft_clip",
            "Clipper",
            json!({"clip_limit": -6.0, "soft_clip": true}),
        ),
        (
            "dither",
            "Dither",
            json!({"type": "Flat", "bits": 16, "amplitude": 2.0}),
        ),
        (
            "dither_shaped",
            "Dither",
            json!({"type": "Shibata48", "bits": 16}),
        ),
        ("diffeq", "DiffEq", json!({"a": [1.0, -0.5], "b": [0.5]})),
        (
            "combo",
            "BiquadCombo",
            json!({"type": "LinkwitzRileyLowpass", "freq": 2000.0, "order": 8}),
        ),
        (
            "limiter",
            "LookaheadLimiter",
            json!({"limit": -6.0, "attack": 5.0, "attack_unit": "ms", "release": 100.0, "release_unit": "ms"}),
        ),
    ];
    let processors = [
        (
            "compressor",
            "Compressor",
            json!({"threshold": -20.0, "factor": 4.0, "attack": 5.0, "attack_unit": "ms", "release": 100.0, "release_unit": "ms"}),
        ),
        (
            "noisegate",
            "NoiseGate",
            json!({"threshold": -40.0, "attenuation": 60.0, "attack": 5.0, "attack_unit": "ms", "release": 100.0, "release_unit": "ms"}),
        ),
        (
            "linked_limiter",
            "LookaheadLimiter",
            json!({"limit": -6.0, "attack": 5.0, "attack_unit": "ms", "release": 100.0, "release_unit": "ms"}),
        ),
        (
            "race",
            "RACE",
            json!({"channel_a": 0, "channel_b": 1, "delay": 100.0, "delay_unit": "us", "attenuation": 6.0}),
        ),
    ];
    let mut group = c.benchmark_group("processing_paths");
    for (channels, frames) in [(2, 64), (2, 1024), (8, 1024)] {
        for (kind, variants) in [
            ("Filter", filters.as_slice()),
            ("Processor", processors.as_slice()),
        ] {
            for (name, typ, parameters) in variants {
                let mut parameters = parameters.clone();
                let mut conf = json!({
                    "devices": {
                        "samplerate": 48000, "chunksize": frames,
                        "capture": {"type": "Stdin", "channels": channels, "format": "F32_LE"},
                        "playback": {"type": "Stdout", "channels": channels, "format": "F32_LE"}
                    }
                });
                if kind == "Filter" {
                    conf["filters"] = json!({"test": {"type": typ, "parameters": parameters}});
                    conf["pipeline"] = json!([{"type": kind, "names": ["test"]}]);
                } else {
                    parameters["channels"] = json!(channels);
                    conf["processors"] = json!({"test": {"type": typ, "parameters": parameters}});
                    conf["pipeline"] = json!([{"type": kind, "name": "test"}]);
                }
                let params = Arc::new(ProcessingParameters::new(
                    &[0.0, -20.0, 0.0, 0.0, 0.0],
                    &[false; 5],
                ));
                let mut pipeline = Pipeline::from_config(
                    serde_json::from_value(conf).unwrap(),
                    params.clone(),
                    None,
                    &mut ConvCoeffCache::new(),
                );
                let source = make_chunk(channels, frames);
                let mut chunk = Some(make_chunk(channels, frames));
                let mut ramp_up = false;
                group.bench_function(
                    BenchmarkId::new(*name, format!("{channels}ch_{frames}")),
                    |b| {
                        b.iter(|| {
                            if *name == "volume_ramp" {
                                params.set_target_volume(1, if ramp_up { -20.0 } else { -21.0 });
                                ramp_up = !ramp_up;
                            }
                            let mut input = chunk.take().unwrap();
                            for (wave, src) in input.waveforms.iter_mut().zip(&source.waveforms) {
                                wave.copy_from_slice(src);
                            }
                            chunk = Some(pipeline.process_chunk(input));
                            black_box(&chunk);
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

fn bench_metering(c: &mut Criterion) {
    use camilladsp::spectrum::{AudioRingBuffer, RING_BUFFER_CAPACITY, SpectrumComputer};

    let mut group = c.benchmark_group("metering");
    for (channels, frames) in [(2, 64), (2, 1024), (16, 1024), (16, 4096)] {
        let chunk = make_chunk(channels, frames);
        let mut stats = chunk.stats();
        let mut ring = AudioRingBuffer::new();
        for _ in 0..RING_BUFFER_CAPACITY / frames + 1 {
            ring.push_chunk(&chunk);
        }
        let shape = format!("{channels}ch_{frames}");
        group.bench_function(BenchmarkId::new("rms_peak", &shape), |b| {
            b.iter(|| {
                chunk.update_stats(&mut stats);
                black_box(&stats);
            })
        });
        group.bench_function(BenchmarkId::new("push", &shape), |b| {
            b.iter(|| ring.push_chunk(black_box(&chunk)))
        });
        for fft_len in [4096, RING_BUFFER_CAPACITY] {
            for channel in [Some(0), None] {
                let mut buf = Vec::with_capacity(fft_len);
                let name = if channel.is_some() {
                    "read_channel"
                } else {
                    "read_average"
                };
                group.bench_function(BenchmarkId::new(name, format!("{shape}_{fft_len}")), |b| {
                    b.iter(|| {
                        assert!(ring.read_latest_into(fft_len, channel, &mut buf));
                        black_box(&buf);
                    })
                });
            }
        }
    }
    for fft_len in [4096, 65536, RING_BUFFER_CAPACITY] {
        let mut ring = AudioRingBuffer::new();
        ring.push_chunk(&make_chunk(2, fft_len));
        let mut computer = SpectrumComputer::new(fft_len, 256, 20.0, 20000.0, 48000);
        group.bench_function(BenchmarkId::new("spectrum", fft_len), |b| {
            b.iter(|| {
                assert!(computer.fill_from_buffer(&ring, None));
                black_box(computer.compute_from_windowed().unwrap());
            })
        });
    }
    group.finish();
}

fn bench_resampling(c: &mut Criterion) {
    use camilladsp::utils::resampling::new_resampler;
    use camilladsp::utils::stash::{container_from_stash, recycle_chunk, vec_from_stash};

    let mut variants = vec![
        ("sync", config::Resampler::Synchronous),
        ("slip", config::Resampler::Slip),
    ];
    for (name, profile) in [
        ("sinc_veryfast", config::AsyncSincProfile::VeryFast),
        ("sinc_fast", config::AsyncSincProfile::Fast),
        ("sinc_balanced", config::AsyncSincProfile::Balanced),
        ("sinc_accurate", config::AsyncSincProfile::Accurate),
    ] {
        variants.push((
            name,
            config::Resampler::AsyncSinc(config::AsyncSincParameters::Profile { profile }),
        ));
    }
    for (name, interpolation) in [
        ("poly_linear", config::AsyncPolyInterpolation::Linear),
        ("poly_cubic", config::AsyncPolyInterpolation::Cubic),
        ("poly_quintic", config::AsyncPolyInterpolation::Quintic),
        ("poly_septic", config::AsyncPolyInterpolation::Septic),
    ] {
        variants.push((name, config::Resampler::AsyncPoly { interpolation }));
    }
    let mut group = c.benchmark_group("resampling");
    for channels in [2, 8] {
        for (name, conf) in &variants {
            let rate_in = if *name == "slip" { 48000 } else { 44100 };
            let mut resampler = new_resampler(
                &Some(*conf),
                channels,
                48000,
                rate_in,
                CHUNK_SIZE,
                Arc::new(ProcessingParameters::default()),
            )
            .unwrap();
            let source = make_chunk(channels, resampler.resampler.input_frames_max());
            group.bench_function(BenchmarkId::new(*name, channels), |b| {
                b.iter(|| {
                    let frames = resampler.resampler.input_frames_next();
                    let mut waves = container_from_stash(channels);
                    for src in &source.waveforms {
                        let mut wave = vec_from_stash(frames);
                        wave.copy_from_slice(&src[..frames]);
                        waves.push(wave);
                    }
                    let mut chunk = AudioChunk::new(waves, 1.0, -1.0, frames, frames);
                    resampler.resample_chunk(&mut chunk, CHUNK_SIZE, channels);
                    black_box(&chunk);
                    recycle_chunk(chunk);
                })
            });
        }
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
    targets = bench_complete_pipeline, bench_processing_paths, bench_metering, bench_resampling
}
criterion_main!(benches);
