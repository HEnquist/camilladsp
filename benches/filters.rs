// Full-precision f64 literals look excessive when an f32 build makes
// `CamillaFloat` an `f32`; silence that only in the f32 build.
#![cfg_attr(camillafloat_f32, allow(clippy::excessive_precision))]

use criterion::{Bencher, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use camilladsp::filters::Filter;
use camilladsp::filters::biquad::{Biquad, BiquadCoefficients};
use camilladsp::filters::diffeq::DiffEq;
use camilladsp::filters::fftconv::FftConv;
use camilladsp::{CamillaFloat, ToCamillaFloat};

/// Bench a single convolution
fn run_conv(b: &mut Bencher, len: usize, chunksize: usize) {
    let filter: Vec<_> = (0..len)
        .map(|i| ((i as f64 * 0.013).cos() / len as f64).to_camilla_float())
        .collect();
    let mut conv = FftConv::new("test", chunksize, &filter);
    let input = signal(chunksize);
    let mut waveform = input.clone();

    b.iter(|| {
        waveform.copy_from_slice(&input);
        conv.process_waveform(black_box(&mut waveform));
    });
}

/// Run all convolution benches
fn bench_conv(c: &mut Criterion) {
    let mut group = c.benchmark_group("Conv");
    for chunksize in [1024, 4096] {
        for filterlen in [1024, 65536, 524288] {
            group.bench_with_input(
                BenchmarkId::new(format!("FftConv_{chunksize}"), filterlen),
                &filterlen,
                |b, filterlen| run_conv(b, *filterlen, chunksize),
            );
        }
    }
    group.finish();
}

fn signal(frames: usize) -> Vec<CamillaFloat> {
    (0..frames)
        .map(|i| (0.5 * (i as f64 * 0.017).sin()).to_camilla_float())
        .collect()
}

/// Bench biquad
fn bench_biquad(c: &mut Criterion) {
    let chunksize = 1024;
    let coeffs = BiquadCoefficients::new(
        -0.1462978543780541,
        0.005350765548905586,
        0.21476322779271284,
        0.4295264555854257,
        0.21476322779271284,
    );
    let mut bq = Biquad::new("test", chunksize, coeffs);
    let input = signal(chunksize);
    let mut waveform = input.clone();

    c.bench_function("Biquad", |b| {
        b.iter(|| {
            waveform.copy_from_slice(&input);
            bq.process_waveform(black_box(&mut waveform));
        })
    });
}

/// Bench diffew
fn bench_diffeq(c: &mut Criterion) {
    let chunksize = 1024;
    let mut group = c.benchmark_group("DiffEq");

    let mut de = DiffEq::new(
        "test",
        vec![1.0, -0.1462978543780541, 0.005350765548905586],
        vec![0.21476322779271284, 0.4295264555854257, 0.21476322779271284],
    );
    let input = signal(chunksize);
    let mut waveform = input.clone();
    group.bench_function(BenchmarkId::new("order", 2), |b| {
        b.iter(|| {
            waveform.copy_from_slice(&input);
            de.process_waveform(black_box(&mut waveform));
        })
    });

    // Higher orders, to also cover the runtime order version.
    for order in [8, 16] {
        let mut a = vec![1.0];
        let mut b_coeffs = Vec::new();
        for n in 0..order {
            a.push(0.1 / (n as f64 + 2.0));
            b_coeffs.push(0.3 / (n as f64 + 1.0));
        }
        b_coeffs.push(0.05);
        let mut de = DiffEq::new("test", a, b_coeffs);
        let mut waveform = input.clone();
        group.bench_function(BenchmarkId::new("order", order), |b| {
            b.iter(|| {
                waveform.copy_from_slice(&input);
                de.process_waveform(black_box(&mut waveform));
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_conv, bench_biquad, bench_diffeq);

criterion_main!(benches);
