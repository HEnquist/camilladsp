// Sizing bench for interleaved multi-channel biquads.
use camillalib::CamillaFloat;
use camillalib::filters::Filter;
use camillalib::filters::biquad::{Biquad, BiquadCoefficients, process_cascades_interleaved};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const CHUNK: usize = 1024;

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
    (0..CHUNK)
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

criterion_group!(benches, bench);
criterion_main!(benches);
