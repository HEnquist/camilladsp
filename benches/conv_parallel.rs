//! How well does one convolution filter scale across channels?
//!
//! Every channel of a filter step runs the same impulse response, and the
//! transformed coefficients are read-only, so the channels share one copy of
//! them. This measures what that is worth: `shared` builds the channels
//! through one [`ConvCoeffCache`] the way the pipeline does, `separate` gives
//! each channel its own copy, which is what happens without the cache.
//!
//! The difference is memory traffic. Coefficients are read in full for every
//! chunk, so with a copy per channel the working set grows with the channel
//! count. Both processing paths pay for that, so both are measured: `par`
//! spreads the channels over a thread pool the way `multithreaded` does,
//! `serial` walks them in order the way the default path does. In parallel the
//! copies compete for bandwidth at the same instant; in serial they evict each
//! other from cache in turn.
//!
//! The pool comes from `build_processing_threadpool`, so its workers get the
//! same real-time priority they get in production. Note that the calling thread
//! is deliberately left alone: promoting it makes a back-to-back benchmark far
//! slower, because the scheduler throttles a real-time thread that overruns the
//! duty cycle it declared.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rayon::prelude::*;
use std::hint::black_box;

use camillalib::CamillaFloat;
use camillalib::config;
use camillalib::filters::Filter;
use camillalib::filters::fftconv::{ConvCoeffCache, FftConv};
use camillalib::processing::build_processing_threadpool;

const CHUNK: usize = 1024;
const CHANNEL_COUNTS: [usize; 3] = [1, 2, 4];
/// Short enough to stay in cache, long enough to leave it well behind.
const FILTER_LENGTHS: [usize; 3] = [16384, 262144, 1048576];

fn conv_config(length: usize) -> config::ConvParameters {
    let scale = 1.0 / length as f64;
    config::ConvParameters::Values {
        values: (0..length)
            .map(|n| (n as f64 * 0.001).sin() * scale)
            .collect(),
    }
}

/// One filter per channel, all sharing the transformed coefficients.
fn shared_filters(channels: usize, length: usize) -> Vec<FftConv> {
    let mut cache = ConvCoeffCache::new();
    (0..channels)
        .map(|_| FftConv::from_config_cached("conv", CHUNK, conv_config(length), &mut cache))
        .collect()
}

/// One filter per channel, each with its own copy of the coefficients.
fn separate_filters(channels: usize, length: usize) -> Vec<FftConv> {
    (0..channels)
        .map(|_| FftConv::from_config("conv", CHUNK, conv_config(length)))
        .collect()
}

fn waveforms(channels: usize) -> Vec<Vec<CamillaFloat>> {
    (0..channels)
        .map(|c| {
            (0..CHUNK)
                .map(|n| ((n + c * 13) as CamillaFloat * 0.013).sin())
                .collect()
        })
        .collect()
}

/// The filters work in place, so restore the input before each iteration.
/// Without this every iteration convolves the previous output again and the
/// signal runs away until the transform rejects it.
fn refill(waves: &mut [Vec<CamillaFloat>], source: &[Vec<CamillaFloat>]) {
    for (wave, src) in waves.iter_mut().zip(source) {
        wave.copy_from_slice(src);
    }
}

fn bench_conv_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("conv_parallel");
    group.sample_size(20);

    for length in FILTER_LENGTHS {
        for channels in CHANNEL_COUNTS {
            // The production builder, so the workers are promoted to real-time
            // exactly as they are when CamillaDSP runs. Without the promotion
            // the workers can land on efficiency cores, which measured 9 to 17%
            // slower here and is a configuration that never ships.
            let pool =
                build_processing_threadpool(true, channels, CHUNK, 48000).expect("thread pool");
            let label = format!("{length}taps_{channels}ch");
            for (arm, build) in [
                ("shared", shared_filters as fn(usize, usize) -> Vec<FftConv>),
                ("separate", separate_filters),
            ] {
                for mode in ["par", "serial"] {
                    let parallel = mode == "par";
                    let name = format!("{mode}_{arm}");
                    group.bench_with_input(
                        BenchmarkId::new(name, &label),
                        &length,
                        |b, &length| {
                            let mut filters = build(channels, length);
                            let source = waveforms(channels);
                            let mut waves = source.clone();
                            b.iter(|| {
                                refill(&mut waves, &source);
                                if parallel {
                                    pool.install(|| {
                                        filters.par_iter_mut().zip(waves.par_iter_mut()).for_each(
                                            |(filter, wave)| {
                                                filter.process_waveform(wave).unwrap();
                                            },
                                        );
                                    });
                                } else {
                                    for (filter, wave) in filters.iter_mut().zip(waves.iter_mut()) {
                                        filter.process_waveform(wave).unwrap();
                                    }
                                }
                                black_box(&waves[0][0]);
                            })
                        },
                    );
                }
            }
        }
    }
    group.finish();
}

criterion_group!(benches, bench_conv_parallel);
criterion_main!(benches);
