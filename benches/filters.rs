extern crate criterion;
use criterion::{criterion_group, criterion_main, Bencher, BenchmarkId, Criterion};
extern crate camillalib;

use camillalib::biquad::{Biquad, BiquadCoefficients};
use camillalib::diffeq::DiffEq;
use camillalib::fftconv::FftConv;
use camillalib::filters::Filter;
use camillalib::PrcFmt;

/// Bench a single convolution
fn run_conv(b: &mut Bencher, len: usize, chunksize: usize) {
    let filter = vec![0.0 as PrcFmt; len];
    let mut conv = FftConv::new("test".to_string(), chunksize, &filter);
    let mut waveform = vec![0.0 as PrcFmt; chunksize];

    //let mut spectrum = signal.clone();
    b.iter(|| conv.process_waveform(&mut waveform));
}

/// Run all convolution benches
fn bench_conv(c: &mut Criterion) {
    let mut group = c.benchmark_group("Conv");
    let chunksize = 1024;
    for filterlen in [chunksize, 4 * chunksize, 16 * chunksize].iter() {
        group.bench_with_input(
            BenchmarkId::new("FftConv", filterlen),
            filterlen,
            |b, filterlen| run_conv(b, *filterlen, chunksize),
        );
    }
    group.finish();
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
    let mut bq = Biquad::new("test".to_string(), chunksize, coeffs);
    let mut waveform = vec![0.0 as PrcFmt; chunksize];

    c.bench_function("Biquad", |b| b.iter(|| bq.process_waveform(&mut waveform)));
}

/// Bench diffew
fn bench_diffeq(c: &mut Criterion) {
    let chunksize = 1024;
    let mut de = DiffEq::new(
        "test".to_string(),
        vec![1.0, -0.1462978543780541, 0.005350765548905586],
        vec![0.21476322779271284, 0.4295264555854257, 0.21476322779271284],
    );
    let mut waveform = vec![0.0 as PrcFmt; chunksize];

    c.bench_function("DiffEq", |b| b.iter(|| de.process_waveform(&mut waveform)));
}

criterion_group!(benches, bench_conv, bench_biquad, bench_diffeq);

criterion_main!(benches);
