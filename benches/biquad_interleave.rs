// Sizing bench for interleaved multi-channel biquads.
use camillalib::CamillaFloat;
use camillalib::filters::Filter;
use camillalib::filters::biquad::{
    Biquad, BiquadCoefficients, process_cascade_canon, process_cascade_canon_depth,
    process_cascades_interleaved,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const CHUNK: usize = 1024;

/// `Biquad::new` wants a samplerate, not a chunk length. Nothing in the
/// kernel reads it, since the coefficients are given directly, but passing a
/// frame count here would be misleading.
const SAMPLERATE: usize = 44100;

/// Short chunks are where the canon's ramp-up and drain cost the most: the
/// wavefront takes `n + depth - 1` iterations for `n` samples.
const SHORT_CHUNK: usize = 64;

fn coeffs() -> BiquadCoefficients {
    BiquadCoefficients::new(
        -0.1462978543780541,
        0.005350765548905586,
        0.21476322779271284,
        0.4295264555854257,
        0.21476322779271284,
    )
}

/// A real signal. All-zero input is not representative: it never exercises the
/// magnitudes the state variables actually take.
fn signal() -> Vec<CamillaFloat> {
    signal_of_len(CHUNK)
}

fn signal_of_len(len: usize) -> Vec<CamillaFloat> {
    (0..len)
        .map(|n| (n as CamillaFloat * 0.013).sin() * 0.5)
        .collect()
}

fn bench(c: &mut Criterion) {
    let mut g = c.benchmark_group("interleave");
    for n in [1usize, 2, 3, 4, 5, 6, 8] {
        g.throughput(criterion::Throughput::Elements((n * CHUNK) as u64));

        // Reference: one channel at a time, the current production path.
        let mut seq: Vec<Biquad> = (0..n)
            .map(|_| Biquad::new("t", SAMPLERATE, coeffs()))
            .collect();
        let mut seq_w: Vec<Vec<CamillaFloat>> = (0..n).map(|_| signal()).collect();
        g.bench_with_input(BenchmarkId::new("sequential", n), &n, |b, _| {
            b.iter(|| {
                for (bq, w) in seq.iter_mut().zip(seq_w.iter_mut()) {
                    bq.process_waveform(w);
                }
            })
        });

        let mut int: Vec<Vec<Biquad>> = (0..n)
            .map(|_| vec![Biquad::new("t", SAMPLERATE, coeffs())])
            .collect();
        let mut int_w: Vec<Vec<CamillaFloat>> = (0..n).map(|_| signal()).collect();
        // Built once, outside the timed loop. Production destructures instead
        // of collecting, precisely so the audio path allocates nothing, so
        // collecting per iteration would time something it never does.
        let mut cr: Vec<&mut [Biquad]> = int.iter_mut().map(|c| c.as_mut_slice()).collect();
        let mut wr: Vec<&mut [CamillaFloat]> = int_w.iter_mut().map(|w| w.as_mut_slice()).collect();
        g.bench_with_input(BenchmarkId::new("interleaved", n), &n, |b, _| {
            b.iter(|| {
                process_cascades_interleaved(&mut cr, &mut wr);
            })
        });
    }
    g.finish();
}

/// Sizing bench for the cascade canon: one channel, so the channel axis
/// supplies no parallelism at all and every independent chain has to come
/// from cascade depth.
///
/// `depth = 1` is the control. It runs the same kernel one stage at a time,
/// so it isolates the canon's effect from any difference between the kernel
/// and `process_waveform`.
fn bench_canon(c: &mut Criterion) {
    for chunk in [CHUNK, SHORT_CHUNK] {
        let mut g = c.benchmark_group(format!("canon_{chunk}"));
        for stages in [4usize, 16] {
            g.throughput(criterion::Throughput::Elements((stages * chunk) as u64));

            // Reference: the current production path for a single channel.
            let mut seq: Vec<Biquad> = (0..stages)
                .map(|_| Biquad::new("t", SAMPLERATE, coeffs()))
                .collect();
            let mut seq_w = signal_of_len(chunk);
            g.bench_with_input(BenchmarkId::new("sequential", stages), &stages, |b, _| {
                b.iter(|| {
                    for bq in seq.iter_mut() {
                        bq.process_waveform(&mut seq_w);
                    }
                })
            });

            for depth in [1usize, 2, 4, 6, 8] {
                let mut casc: Vec<Biquad> = (0..stages)
                    .map(|_| Biquad::new("t", SAMPLERATE, coeffs()))
                    .collect();
                let mut w = signal_of_len(chunk);
                g.bench_with_input(
                    BenchmarkId::new(format!("canon{depth}"), stages),
                    &stages,
                    |b, _| b.iter(|| process_cascade_canon_depth(&mut casc, &mut w, depth)),
                );
            }
        }
        g.finish();
    }
}

/// How the two axes compare as the chunk shrinks.
///
/// The canon pays a fixed cost per pass, loading and storing eight stages,
/// which the channel interleave does not. On a short chunk that cost is
/// spread over few samples, so short chunks are where the axis choice flips.
/// 16 stages per channel, the shape a PEQ configuration has.
fn bench_small_chunks(c: &mut Criterion) {
    let mut g = c.benchmark_group("axis_vs_chunk");
    for nch in [2usize, 4] {
        for chunk in [16usize, 32, 64, 128] {
            g.throughput(criterion::Throughput::Elements((nch * 16 * chunk) as u64));

            let mut int: Vec<Vec<Biquad>> = (0..nch)
                .map(|_| {
                    (0..16)
                        .map(|_| Biquad::new("t", SAMPLERATE, coeffs()))
                        .collect()
                })
                .collect();
            let mut int_w: Vec<Vec<CamillaFloat>> =
                (0..nch).map(|_| signal_of_len(chunk)).collect();
            // Outside the timed loop: the crossover this table decides is a
            // few percent wide, and two allocations per iteration are not.
            let mut cr: Vec<&mut [Biquad]> = int.iter_mut().map(|c| c.as_mut_slice()).collect();
            let mut wr: Vec<&mut [CamillaFloat]> =
                int_w.iter_mut().map(|w| w.as_mut_slice()).collect();
            g.bench_with_input(
                BenchmarkId::new(format!("ch{nch}_channels"), chunk),
                &chunk,
                |b, _| {
                    b.iter(|| {
                        process_cascades_interleaved(&mut cr, &mut wr);
                    })
                },
            );

            let mut can: Vec<Vec<Biquad>> = (0..nch)
                .map(|_| {
                    (0..16)
                        .map(|_| Biquad::new("t", SAMPLERATE, coeffs()))
                        .collect()
                })
                .collect();
            let mut can_w: Vec<Vec<CamillaFloat>> =
                (0..nch).map(|_| signal_of_len(chunk)).collect();
            g.bench_with_input(
                BenchmarkId::new(format!("ch{nch}_cascade"), chunk),
                &chunk,
                |b, _| {
                    b.iter(|| {
                        for (casc, w) in can.iter_mut().zip(can_w.iter_mut()) {
                            process_cascade_canon(casc, w);
                        }
                    })
                },
            );
        }
    }
    g.finish();
}

criterion_group!(benches, bench, bench_canon, bench_small_chunks);
criterion_main!(benches);
