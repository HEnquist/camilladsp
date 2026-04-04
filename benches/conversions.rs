extern crate criterion;
use camillalib::utils::stash::{recycle_chunk, vec_from_stash};
use criterion::{Criterion, criterion_group, criterion_main};
extern crate camillalib;

use camillalib::audiochunk::AudioChunk;
use camillalib::config::BinarySampleFormat;
use camillalib::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};

fn bench_to_chunk_small(c: &mut Criterion) {
    let datalen = 2 * 4 * 64;
    let data = vec![0u8; datalen];
    let mask = [true, true];
    c.bench_function("to_chunk_small", |b| {
        b.iter(|| {
            let chunk = buffer_to_chunk_rawbytes(
                &data,
                2,
                &BinarySampleFormat::S32_LE,
                datalen,
                &mask,
                false,
            );
            recycle_chunk(chunk);
        })
    });
}

fn bench_to_chunk_large(c: &mut Criterion) {
    let datalen = 2 * 4 * 4096;
    let data = vec![0u8; datalen];
    let mask = [true, true];
    c.bench_function("to_chunk_large", |b| {
        b.iter(|| {
            let chunk = buffer_to_chunk_rawbytes(
                &data,
                2,
                &BinarySampleFormat::S32_LE,
                datalen,
                &mask,
                false,
            );
            recycle_chunk(chunk);
        })
    });
}

fn bench_to_bytes_large(c: &mut Criterion) {
    let mask = [true, true];
    let num_frames = 4096;
    let datalen = 2 * 4 * num_frames;
    let mut data = vec![0u8; datalen];
    c.bench_function("to_bytes_large", |b| {
        b.iter(|| {
            let mut wfs = Vec::with_capacity(mask.len());
            for _chan in 0..mask.len() {
                wfs.push(vec_from_stash(num_frames));
            }
            let chunk = AudioChunk::new(wfs, 0.0, 0.0, num_frames, num_frames);
            chunk_to_buffer_rawbytes(chunk, &mut data, &BinarySampleFormat::S32_LE)
        })
    });
}

fn bench_to_bytes_small(c: &mut Criterion) {
    let mask = [true, true];
    let num_frames = 64;
    let datalen = 2 * 4 * num_frames;
    let mut data = vec![0u8; datalen];
    c.bench_function("to_bytes_small", |b| {
        b.iter(|| {
            let mut wfs = Vec::with_capacity(mask.len());
            for _chan in 0..mask.len() {
                wfs.push(vec_from_stash(num_frames));
            }
            let chunk = AudioChunk::new(wfs, 0.0, 0.0, num_frames, num_frames);
            chunk_to_buffer_rawbytes(chunk, &mut data, &BinarySampleFormat::S32_LE)
        })
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
