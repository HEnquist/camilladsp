use camilladsp::utils::stash::{recycle_chunk, vec_from_stash};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use camilladsp::audiochunk::AudioChunk;
use camilladsp::config::BinarySampleFormat;
use camilladsp::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};

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

fn bench_formats(c: &mut Criterion) {
    use camilladsp::ToCamillaFloat;
    use camilladsp::utils::stash::container_from_stash;

    let mut group = c.benchmark_group("sample_formats");
    for format in [
        BinarySampleFormat::S16_LE,
        BinarySampleFormat::S24_3_LE,
        BinarySampleFormat::S24_4_RJ_LE,
        BinarySampleFormat::S24_4_LJ_LE,
        BinarySampleFormat::S32_LE,
        BinarySampleFormat::F32_LE,
        BinarySampleFormat::F64_LE,
    ] {
        for (channels, frames) in [(2, 64), (2, 1024), (16, 4096)] {
            let source: Vec<_> = (0..frames)
                .map(|i| (0.9 * (i as f64 * 0.1).sin()).to_camilla_float())
                .collect();
            let mask = vec![true; channels];
            let mut data = vec![0; channels * frames * format.bytes_per_sample()];
            let input = AudioChunk::new(vec![source.clone(); channels], 0.9, -0.9, frames, frames);
            chunk_to_buffer_rawbytes(input, &mut data, &format);
            let label = format!("{format:?}_{channels}ch_{frames}");
            group.bench_function(BenchmarkId::new("capture", &label), |b| {
                b.iter(|| {
                    let chunk = buffer_to_chunk_rawbytes(
                        black_box(&data),
                        channels,
                        &format,
                        data.len(),
                        &mask,
                        true,
                    );
                    black_box(&chunk);
                    recycle_chunk(chunk);
                })
            });
            group.bench_function(BenchmarkId::new("playback", &label), |b| {
                b.iter(|| {
                    let mut waves = container_from_stash(channels);
                    for _ in 0..channels {
                        let mut wave = vec_from_stash(frames);
                        wave.copy_from_slice(&source);
                        waves.push(wave);
                    }
                    let chunk = AudioChunk::new(waves, 0.9, -0.9, frames, frames);
                    black_box(chunk_to_buffer_rawbytes(chunk, &mut data, &format));
                    black_box(&data);
                })
            });
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_to_chunk_small,
    bench_to_chunk_large,
    bench_to_bytes_small,
    bench_to_bytes_large,
    bench_formats
);

criterion_main!(benches);
