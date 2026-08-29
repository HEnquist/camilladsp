// Sizing bench for interleaved multi-channel biquads.
use camillalib::CamillaFloat;
use camillalib::filters::Filter;
use camillalib::filters::biquad::{
    Biquad, BiquadCoefficients, process_cascade_canon_depth, process_cascades_interleaved,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const CHUNK: usize = 1024;

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
        let mut seq: Vec<Biquad> = (0..n).map(|_| Biquad::new("t", CHUNK, coeffs())).collect();
        let mut seq_w: Vec<Vec<CamillaFloat>> = (0..n).map(|_| signal()).collect();
        g.bench_with_input(BenchmarkId::new("sequential", n), &n, |b, _| {
            b.iter(|| {
                for (bq, w) in seq.iter_mut().zip(seq_w.iter_mut()) {
                    bq.process_waveform(w).unwrap();
                }
            })
        });

        let mut int: Vec<Vec<Biquad>> = (0..n)
            .map(|_| vec![Biquad::new("t", CHUNK, coeffs())])
            .collect();
        let mut int_w: Vec<Vec<CamillaFloat>> = (0..n).map(|_| signal()).collect();
        g.bench_with_input(BenchmarkId::new("interleaved", n), &n, |b, _| {
            b.iter(|| {
                let mut cr: Vec<&mut [Biquad]> = int.iter_mut().map(|c| c.as_mut_slice()).collect();
                let mut wr: Vec<&mut [CamillaFloat]> =
                    int_w.iter_mut().map(|w| w.as_mut_slice()).collect();
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
                .map(|_| Biquad::new("t", chunk, coeffs()))
                .collect();
            let mut seq_w = signal_of_len(chunk);
            g.bench_with_input(BenchmarkId::new("sequential", stages), &stages, |b, _| {
                b.iter(|| {
                    for bq in seq.iter_mut() {
                        bq.process_waveform(&mut seq_w).unwrap();
                    }
                })
            });

            for depth in [1usize, 2, 4, 6, 8] {
                let mut casc: Vec<Biquad> = (0..stages)
                    .map(|_| Biquad::new("t", chunk, coeffs()))
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

criterion_group!(benches, bench, bench_canon);
criterion_main!(benches);
