extern crate criterion;
use criterion::{criterion_group, criterion_main, Criterion};
extern crate camillalib;

use camillalib::audiodevice::AudioChunk;
use camillalib::config::SampleFormat;
use camillalib::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};

fn bench_to_chunk_small(c: &mut Criterion) {
    let datalen = 2 * 4 * 64;
    let data = vec![0u8; datalen];
    let mask = vec![true, true];
    c.bench_function("to_chunk_small", |b| {
        b.iter(|| buffer_to_chunk_rawbytes(&data, 2, &SampleFormat::S32LE, datalen, &mask))
    });
}

fn bench_to_chunk_large(c: &mut Criterion) {
    let datalen = 2 * 4 * 4096;
    let data = vec![0u8; datalen];
    let mask = vec![true, true];
    c.bench_function("to_chunk_large", |b| {
        b.iter(|| buffer_to_chunk_rawbytes(&data, 2, &SampleFormat::S32LE, datalen, &mask))
    });
}

fn bench_to_bytes_large(c: &mut Criterion) {
    let mask = vec![true, true];
    let num_frames = 4096;
    let wfs = vec![vec![0.0; num_frames]; mask.len()];
    let chunk = AudioChunk::new(wfs, 0.0, 0.0, num_frames, num_frames);
    let datalen = 2 * 4 * num_frames;
    let mut data = vec![0u8; datalen];
    c.bench_function("to_bytes_large", |b| {
        b.iter(|| chunk_to_buffer_rawbytes(&chunk, &mut data, &SampleFormat::S32LE))
    });
}

fn bench_to_bytes_small(c: &mut Criterion) {
    let mask = vec![true, true];
    let num_frames = 64;
    let wfs = vec![vec![0.0; num_frames]; mask.len()];
    let chunk = AudioChunk::new(wfs, 0.0, 0.0, num_frames, num_frames);
    let datalen = 2 * 4 * num_frames;
    let mut data = vec![0u8; datalen];
    c.bench_function("to_bytes_small", |b| {
        b.iter(|| chunk_to_buffer_rawbytes(&chunk, &mut data, &SampleFormat::S32LE))
    });
}

criterion_group!(
    benches,
    bench_to_chunk_small,
    bench_to_chunk_large,
    bench_to_bytes_small,
    bench_to_bytes_large
);

criterion_main!(benches);
