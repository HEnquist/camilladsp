//! Micro-benchmarks for fftconv complex-multiply kernels: scalar vs NEON.

extern crate camillalib;
extern crate criterion;
extern crate num_complex;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use std::hint::black_box;

use camillalib::PrcFmt;
#[cfg(target_arch = "aarch64")]
use camillalib::filters::fftconv::{
    bench_multiply_add_elements_neon, bench_multiply_elements_neon,
};
use camillalib::filters::fftconv::{
    bench_multiply_add_elements_scalar, bench_multiply_elements_scalar,
};

const SIZES: &[usize] = &[129, 513, 1025, 4097, 16385];

fn make_buffers(
    len: usize,
) -> (
    Vec<Complex<PrcFmt>>,
    Vec<Complex<PrcFmt>>,
    Vec<Complex<PrcFmt>>,
) {
    let scale = 1.0 / len as PrcFmt;
    let a: Vec<Complex<PrcFmt>> = (0..len)
        .map(|i| Complex::new((i as PrcFmt + 1.0) * scale, (len - i) as PrcFmt * scale))
        .collect();
    let b: Vec<Complex<PrcFmt>> = (0..len)
        .map(|i| {
            Complex::new(
                (len - i) as PrcFmt * scale,
                (i as PrcFmt + 1.0) * scale * 0.5,
            )
        })
        .collect();
    let result = vec![Complex::new(0.5, -0.5); len];
    (a, b, result)
}

fn throughput_bytes(len: usize) -> u64 {
    (3 * len * std::mem::size_of::<Complex<PrcFmt>>()) as u64
}

fn bench_multiply_elements(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiply_elements");

    for &len in SIZES {
        group.throughput(Throughput::Bytes(throughput_bytes(len)));

        group.bench_with_input(BenchmarkId::new("scalar", len), &len, |b, &len| {
            let (a, bv, mut result) = make_buffers(len);
            b.iter(|| {
                bench_multiply_elements_scalar(
                    black_box(&mut result),
                    black_box(&a),
                    black_box(&bv),
                )
            });
        });

        #[cfg(target_arch = "aarch64")]
        group.bench_with_input(BenchmarkId::new("neon", len), &len, |b, &len| {
            let (a, bv, mut result) = make_buffers(len);
            b.iter(|| unsafe {
                bench_multiply_elements_neon(black_box(&mut result), black_box(&a), black_box(&bv))
            });
        });
    }

    group.finish();
}

fn bench_multiply_add_elements(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiply_add_elements");

    for &len in SIZES {
        group.throughput(Throughput::Bytes(throughput_bytes(len)));

        group.bench_with_input(BenchmarkId::new("scalar", len), &len, |b, &len| {
            let (a, bv, result_init) = make_buffers(len);
            b.iter_batched_ref(
                || result_init.clone(),
                |result| {
                    bench_multiply_add_elements_scalar(
                        black_box(result),
                        black_box(&a),
                        black_box(&bv),
                    );
                },
                BatchSize::SmallInput,
            );
        });

        #[cfg(target_arch = "aarch64")]
        group.bench_with_input(BenchmarkId::new("neon", len), &len, |b, &len| {
            let (a, bv, result_init) = make_buffers(len);
            b.iter_batched_ref(
                || result_init.clone(),
                |result| unsafe {
                    bench_multiply_add_elements_neon(
                        black_box(result),
                        black_box(&a),
                        black_box(&bv),
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_multiply_elements,
    bench_multiply_add_elements
);
criterion_main!(benches);
