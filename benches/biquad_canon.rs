//! Sizing for the biquad canon.
//!
//! The kernel keeps `C * S` independent recurrences in flight, `C` channels
//! side by side and `S` cascade stages skewed in time. Both axes feed the same
//! FP pipeline, so the question this bench answers is how wide it is worth
//! going and how the width is best divided between the two. Everything else in
//! the design reads its constants from what this measures.
//!
//! Two groups. `kernel_grid` holds the work fixed at four channels of eight
//! biquads and sweeps every split of it, so the numbers compare scheduling and
//! nothing else. `kernel_shapes` walks shapes real configurations have, from a
//! deep stereo cascade to a wide multi-way system with a couple of biquads per
//! channel, and puts the combined split next to each axis on its own.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::time::Duration;

use camillalib::CamillaFloat;
use camillalib::config::{BiquadParameters, NotchWidth};
use camillalib::filters::Filter;
use camillalib::filters::biquad::{
    Biquad, BiquadCoefficients, MAX_CHANNELS, MAX_DEPTH, choose_split, process_cascades_with_split,
};

const CHUNK: usize = 1024;
const SAMPLERATE: usize = 44100;

/// Allpass sections, so the magnitude response is flat and the signal neither
/// grows nor decays as the bench feeds its own output back in for thousands of
/// iterations. A peaking cascade would run away to infinity and start timing
/// the hardware's handling of infinities instead of biquads.
fn cascade(stages: usize, seed: usize) -> Vec<Biquad> {
    (0..stages)
        .map(|k| {
            let conf = BiquadParameters::Allpass(NotchWidth::Q {
                freq: 110.0 + 173.0 * ((k + 3 * seed) as f64),
                q: 0.7 + 0.05 * (k as f64),
            });
            Biquad::new(
                "b",
                SAMPLERATE,
                BiquadCoefficients::from_config(SAMPLERATE, conf),
            )
        })
        .collect()
}

fn signal(seed: usize, frames: usize) -> Vec<CamillaFloat> {
    (0..frames)
        .map(|i| (0.017 * ((7 * i + 13 * seed) as CamillaFloat)).sin() * 0.5)
        .collect()
}

/// What is being filtered: so many channels, so many biquads each, so many
/// frames per chunk.
#[derive(Clone, Copy)]
struct Shape {
    channels: usize,
    depth: usize,
    frames: usize,
}

impl Shape {
    fn new(channels: usize, depth: usize, frames: usize) -> Self {
        Shape {
            channels,
            depth,
            frames,
        }
    }

    fn build(&self) -> (Vec<Vec<Biquad>>, Vec<Vec<CamillaFloat>>) {
        (
            (0..self.channels).map(|c| cascade(self.depth, c)).collect(),
            (0..self.channels).map(|c| signal(c, self.frames)).collect(),
        )
    }

    fn id(&self) -> String {
        format!("{}x{}", self.channels, self.depth)
    }
}

/// One stage at a time over the whole waveform, which is what CamillaDSP does
/// today and what every number here is measured against.
fn bench_sequential(c: &mut Criterion, group_name: &str, shape: Shape) {
    let (mut cascades, mut waves) = shape.build();
    let mut group = c.benchmark_group(group_name);
    group.bench_function(BenchmarkId::new(shape.id(), "sequential"), |b| {
        b.iter(|| {
            for (cascade, wave) in cascades.iter_mut().zip(waves.iter_mut()) {
                for stage in cascade.iter_mut() {
                    stage.process_waveform(wave);
                }
            }
        })
    });
    group.finish();
}

/// The cascades, waveforms and index lists are all built outside `b.iter()` on
/// purpose. Building them inside charges the kernel for allocation that
/// production never does, which measured 2 to 5% and moved where the axes
/// appeared to cross over.
fn bench_split(
    c: &mut Criterion,
    group_name: &str,
    shape: Shape,
    label: String,
    group_width: usize,
    stages: usize,
) {
    let (mut cascades, mut waves) = shape.build();
    // Cascade i filters waveform i and every channel is live, which is what a
    // compiled step looks like when no capture channel is unused.
    let ids: Vec<usize> = (0..shape.channels).collect();
    let mut group = c.benchmark_group(group_name);
    group.bench_function(BenchmarkId::new(shape.id(), label), |b| {
        b.iter(|| {
            process_cascades_with_split(&mut cascades, &mut waves, &ids, &ids, group_width, stages)
        })
    });
    group.finish();
}

/// Fixed work, every split of it. Four channels of eight biquads is 32
/// stage-channels however it is divided, so these numbers are directly
/// comparable and the shape of the curve is the answer.
///
/// The shape is deliberately not tied to `MAX_DEPTH`. Every split of fewer than
/// eight stages exercises the division into passes, while `S8` is the one column
/// that fits in a single pass as long as `MAX_DEPTH` stays at eight. The numbers
/// stay comparable across runs either way.
const GRID_CHANNELS: usize = 4;
const GRID_DEPTH: usize = 8;

fn bench_grid(c: &mut Criterion) {
    let grid = Shape::new(GRID_CHANNELS, GRID_DEPTH, CHUNK);
    bench_sequential(c, "kernel_grid", grid);
    for group_width in 1..=MAX_CHANNELS {
        for stages in 1..=MAX_DEPTH {
            bench_split(
                c,
                "kernel_grid",
                grid,
                format!("C{group_width}_S{stages}"),
                group_width,
                stages,
            );
        }
    }
}

/// Shapes real configurations have. A deep cascade on a couple of channels is
/// the parametric equaliser case; sixteen channels of two or three biquads is
/// a large multi-way active system, where the cascade axis alone has almost
/// nothing to work with.
fn bench_shapes(c: &mut Criterion) {
    const SHAPES: [(usize, usize); 7] =
        [(1, 16), (2, 16), (4, 16), (4, 4), (8, 3), (16, 2), (16, 4)];
    for (channels, depth) in SHAPES {
        let shape = Shape::new(channels, depth, CHUNK);
        bench_sequential(c, "kernel_shapes", shape);
        // Each axis on its own, then the split the pipeline would choose.
        let cascade_only = depth.min(MAX_DEPTH);
        bench_split(
            c,
            "kernel_shapes",
            shape,
            format!("cascade_only_S{cascade_only}"),
            1,
            cascade_only,
        );
        let channel_only = channels.min(MAX_CHANNELS);
        bench_split(
            c,
            "kernel_shapes",
            shape,
            format!("channel_only_C{channel_only}"),
            channel_only,
            1,
        );
        let (group_width, stages) = choose_split(channels, depth);
        bench_split(
            c,
            "kernel_shapes",
            shape,
            format!("split_C{group_width}_S{stages}"),
            group_width,
            stages,
        );
    }
}

/// Short budgets, following `benches/conv_parallel.rs`: criterion budgets by
/// wall time, so the default settings would spend eight seconds per point
/// repeating a result that settled immediately. The real threat to these
/// numbers was never sampling noise, it was comparing runs taken under
/// different power states, and a bench that finishes quickly is easier to
/// repeat, which is the actual defence against that.
fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50)
        .without_plots()
}

/// The canon at a short chunk length.
///
/// Not a target to optimise: chunk sizes this small are inefficient for
/// reasons that dwarf the biquads, and nothing in the kernel reads the chunk
/// length. This is here to confirm the canon does not go backwards there. It
/// should not be able to: every stage still sees every sample exactly once, so
/// the ramp-up and drain add no arithmetic, they only leave fewer stages in
/// flight at the edges. At 64 frames with a depth of 8 that is 57 of 71
/// iterations at full width. Measure it rather than trusting the argument.
fn bench_short_chunks(c: &mut Criterion) {
    const SHORT: usize = 64;
    for (channels, depth) in [(2usize, 16usize), (16, 2)] {
        let shape = Shape::new(channels, depth, SHORT);
        bench_sequential(c, "kernel_short_chunk", shape);
        let (group_width, stages) = choose_split(channels, depth);
        bench_split(
            c,
            "kernel_short_chunk",
            shape,
            format!("split_C{group_width}_S{stages}"),
            group_width,
            stages,
        );
    }
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_grid, bench_shapes, bench_short_chunks
}

criterion_main!(benches);
