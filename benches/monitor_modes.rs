//! Same-process reducer microbenchmarks, including independent legacy baselines.
//! Run separately for f64/f32. Results are not end-to-end audio latency.
use camilladsp::{CamillaFloat, audiochunk, config};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

// Criterion does not execute the imported module's unit-test helpers.
#[allow(dead_code)]
#[path = "../src/processors/monitor.rs"]
mod monitor;

fn legacy_sum(input: &audiochunk::AudioChunk, channels: &[usize], scratch: &mut [CamillaFloat]) {
    scratch.copy_from_slice(&input.waveforms[channels[0]]);
    for &ch in channels.iter().skip(1) {
        for (acc, val) in scratch.iter_mut().zip(input.waveforms[ch].iter()) {
            *acc += *val;
        }
    }
}

fn legacy_max(input: &audiochunk::AudioChunk, channels: &[usize], scratch: &mut [CamillaFloat]) {
    for (peak, val) in scratch.iter_mut().zip(input.waveforms[channels[0]].iter()) {
        *peak = val.abs();
    }
    for &ch in channels.iter().skip(1) {
        for (peak, val) in scratch.iter_mut().zip(input.waveforms[ch].iter()) {
            *peak = peak.max(val.abs());
        }
    }
}

fn benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("monitor_aggregation");
    for channels in [2, 4, 8] {
        for frames in [64, 256, 1024] {
            let waveforms = (0..channels)
                .map(|ch| {
                    (0..frames)
                        .map(|i| ((i + ch * 11) as CamillaFloat * 0.13).sin())
                        .collect()
                })
                .collect();
            let input = audiochunk::AudioChunk::new(waveforms, 1.0, -1.0, frames, frames);
            let selected: Vec<_> = (0..channels).collect();
            let mut scratch = vec![0.0; frames];
            group.throughput(Throughput::Elements((channels * frames) as u64));
            for mode in [
                config::MonitorMode::Sum,
                config::MonitorMode::Max,
                config::MonitorMode::Rms,
            ] {
                let id = BenchmarkId::new(format!("{mode:?}"), format!("{channels}x{frames}"));
                group.bench_function(id, |b| {
                    b.iter(|| {
                        monitor::aggregate_monitor_channels(
                            black_box(&input),
                            black_box(&selected),
                            black_box(mode),
                            black_box(&mut scratch),
                        );
                        black_box(scratch[0]);
                    })
                });
            }
            type Reducer = fn(&audiochunk::AudioChunk, &[usize], &mut [CamillaFloat]);
            for (name, reduce) in [
                ("legacy_sum", legacy_sum as Reducer),
                ("legacy_max", legacy_max as Reducer),
            ] {
                let id = BenchmarkId::new(name, format!("{channels}x{frames}"));
                group.bench_function(id, |b| {
                    b.iter(|| {
                        reduce(
                            black_box(&input),
                            black_box(&selected),
                            black_box(&mut scratch),
                        );
                        black_box(scratch[0]);
                    })
                });
            }
        }
    }
    group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
