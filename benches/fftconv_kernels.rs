//! Micro-benchmarks for fftconv complex-multiply kernels: scalar vs AVX+FMA.

extern crate camillalib;
extern crate criterion;
extern crate num_complex;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use std::hint::black_box;
use std::time::Duration;

use camillalib::PrcFmt;
#[cfg(target_arch = "x86_64")]
use camillalib::filters::fftconv::{
    bench_has_avx_fma, bench_multiply_add_elements_avx_fma, bench_multiply_elements_avx_fma,
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

        #[cfg(target_arch = "x86_64")]
        group.bench_with_input(BenchmarkId::new("avx", len), &len, |b, &len| {
            if !bench_has_avx_fma() {
                return;
            }
            let (a, bv, mut result) = make_buffers(len);
            b.iter(|| unsafe {
                bench_multiply_elements_avx_fma(
                    black_box(&mut result),
                    black_box(&a),
                    black_box(&bv),
                )
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

        #[cfg(target_arch = "x86_64")]
        group.bench_with_input(BenchmarkId::new("avx", len), &len, |b, &len| {
            if !bench_has_avx_fma() {
                return;
            }
            let (a, bv, result_init) = make_buffers(len);
            b.iter_batched_ref(
                || result_init.clone(),
                |result| unsafe {
                    bench_multiply_add_elements_avx_fma(
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

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(10));
    targets = bench_multiply_elements, bench_multiply_add_elements
}
criterion_main!(benches);
