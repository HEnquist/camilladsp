//! How long a config reload blocks the processing thread.
//!
//! `benches/pipeline.rs` measures the steady state, one chunk through a built
//! pipeline. This one measures the transient: building a pipeline, hot-reloading
//! filter parameters into an existing one, and dropping the old one. All three
//! run on the processing thread, after it has been promoted to real-time
//! priority, so their cost is a stall in the audio. The `prepared_*` arms are
//! the same passes with the coefficients handed over ready to use, which is
//! what the processing thread pays now.
//!
//! # What the numbers have to be compared against
//!
//! The capture-to-processing and processing-to-playback channels each hold
//! `queuelimit` chunks, 4 by default, and the playback device has a buffer on
//! top of that. At 1024 frames and 48 kHz a chunk is 21.3 ms, so the processing
//! thread can stall somewhere around 50-100 ms before the playback side runs
//! dry. A reload well under that is inaudible; one well over it is a dropout.
//!
//! # Why the convolution filters are the whole question
//!
//! Building a biquad is arithmetic on a handful of coefficients. Building a
//! convolution filter reads a coefficient file from disk and forward-transforms
//! the impulse, once per filter. The `build/biquad_only` arm is here to confirm
//! that an ordinary config is not worth worrying about, and to keep the conv
//! arms in proportion.
//!
//! Note that the shared `FFT_PLANNER` already took plan construction out of this
//! path, so what is left is the read, the parse, the transforms, and the
//! allocation.
//!
//! # The file arms are warm-cache only
//!
//! Criterion repeats each iteration, so only the very first one could ever see
//! a cold page cache and its cost disappears into the average. **Do not read
//! `build/file_*` as the worst case.** It is the floor for a file-backed
//! reload. For the cold case, take the fixture sizes printed at startup and
//! divide by the storage throughput of the machine in question; on a Pi with an
//! SD card at roughly 20 MB/s that term dominates everything measured here.

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use std::time::Duration;

use camillalib::ProcessingParameters;
use camillalib::config;
use camillalib::config::FiniteF64;
use camillalib::filters::fftconv::{ConvCoeffCache, ImpulseCache};
use camillalib::pipeline::Pipeline;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const CHUNK_SIZE: usize = 1024;
const SAMPLERATE: usize = 48000;

/// One FIR per channel, which is the shape of a multiway correction: every
/// driver gets its own filter, so nothing is shared and the coefficient cache
/// has nothing to deduplicate within the pass.
const CHANNELS: usize = 8;

/// A large room correction, and a very long one. Both are realistic; together
/// they show how the cost scales with tap count.
const FIR_LENGTHS: [usize; 2] = [65536, 262144];

const BIQUAD_PARAMS: [(f64, f64); 16] = [
    (120.0, 0.70),
    (220.0, 0.75),
    (350.0, 0.80),
    (500.0, 0.90),
    (700.0, 1.00),
    (900.0, 1.10),
    (1200.0, 0.95),
    (1600.0, 1.05),
    (1800.0, 1.10),
    (2200.0, 0.90),
    (2800.0, 0.95),
    (3200.0, 1.00),
    (3800.0, 0.85),
    (4500.0, 0.80),
    (6200.0, 0.75),
    (8000.0, 0.70),
];

/// A windowed sinc, offset by `seed` so each channel gets its own coefficients
/// and the files are genuinely distinct on disk.
fn sinc(length: usize, seed: usize) -> Vec<f64> {
    let pi = std::f64::consts::PI;
    let centre = (length as f64 - 1.0) * 0.5 + seed as f64;
    (0..length)
        .map(|idx| {
            let x = idx as f64 - centre;
            if x == 0.0 {
                1.0
            } else {
                (pi * x).sin() / (pi * x)
            }
        })
        .collect()
}

fn coeff_path(dir: &Path, length: usize, channel: usize) -> PathBuf {
    dir.join(format!("fir_{length}_{channel}.raw"))
}

/// Write the coefficient files once, under `target/`, and keep them between
/// runs. Returns the directory and the size of one file per length.
fn fixtures() -> &'static (PathBuf, Vec<(usize, u64)>) {
    static FIXTURES: OnceLock<(PathBuf, Vec<(usize, u64)>)> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/bench_fixtures");
        fs::create_dir_all(&dir).expect("can create the fixture directory");
        let mut sizes = Vec::new();
        for length in FIR_LENGTHS {
            let expected = (length * size_of::<f64>()) as u64;
            for channel in 0..CHANNELS {
                let path = coeff_path(&dir, length, channel);
                // Regenerating 20 MB of sinc on every run is pure waste, and a
                // file of the right size is by construction the right file.
                if fs::metadata(&path).is_ok_and(|m| m.len() == expected) {
                    continue;
                }
                let mut bytes = Vec::with_capacity(expected as usize);
                for value in sinc(length, channel) {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                fs::write(&path, &bytes).expect("can write a fixture file");
            }
            sizes.push((length, expected));
        }
        (dir, sizes)
    })
}

/// `ConvParametersRaw` keeps its format and range fields private, so a bench
/// cannot reach them with a struct literal. Going through serde builds the same
/// value and exercises the real parse path on the way.
fn conv_filter_from_file(path: &Path) -> config::Filter {
    let filename = serde_json::to_string(&path.to_string_lossy().into_owned())
        .expect("a path serialises as a json string");
    let json = format!(
        r#"{{"type":"Conv","parameters":{{"type":"Raw","filename":{filename},"format":"F64_LE"}}}}"#
    );
    serde_json::from_str(&json).expect("the conv filter json is valid")
}

fn conv_filter_from_values(length: usize, seed: usize) -> config::Filter {
    config::Filter::Conv {
        description: None,
        parameters: config::ConvParameters::Values {
            values: sinc(length, seed)
                .into_iter()
                .map(FiniteF64::expect_finite)
                .collect(),
        },
    }
}

fn biquad_filter(freq: f64, q: f64) -> config::Filter {
    config::Filter::Biquad {
        description: None,
        parameters: config::BiquadParameters::Peaking(config::PeakingWidth::Q {
            freq: FiniteF64::expect_finite(freq),
            q: FiniteF64::expect_finite(q),
            gain: FiniteF64::expect_finite(1.5),
        }),
    }
}

/// Devices and pipeline from json, so the bench does not carry a hundred lines
/// of struct literal that has to be updated every time a config field is added.
/// The filters are filled in afterwards, since the values-backed ones would be
/// megabytes of json text.
fn base_config(pipeline: &str) -> config::Configuration {
    let json = format!(
        r#"{{
            "devices": {{
                "samplerate": {SAMPLERATE},
                "chunksize": {CHUNK_SIZE},
                "capture": {{"type": "Stdin", "channels": {CHANNELS}, "format": "F32_LE"}},
                "playback": {{"type": "Stdout", "channels": {CHANNELS}, "format": "F32_LE"}}
            }},
            "pipeline": {pipeline}
        }}"#
    );
    serde_json::from_str(&json).expect("the base config json is valid")
}

/// One filter step per channel, each naming that channel's own filter.
fn per_channel_pipeline(prefix: &str) -> String {
    let steps: Vec<String> = (0..CHANNELS)
        .map(|ch| format!(r#"{{"type":"Filter","channels":[{ch}],"names":["{prefix}_{ch}"]}}"#))
        .collect();
    format!("[{}]", steps.join(","))
}

fn conv_names() -> Vec<String> {
    (0..CHANNELS).map(|ch| format!("conv_{ch}")).collect()
}

/// Eight convolution filters, one per channel, either read from the fixture
/// files or carried inline in the config.
fn conv_config(length: usize, from_file: bool) -> config::Configuration {
    let (dir, _) = fixtures();
    let mut conf = base_config(&per_channel_pipeline("conv"));
    let mut filters = HashMap::new();
    for (channel, name) in conv_names().into_iter().enumerate() {
        let filter = if from_file {
            conv_filter_from_file(&coeff_path(dir, length, channel))
        } else {
            conv_filter_from_values(length, channel)
        };
        filters.insert(name, filter);
    }
    conf.filters = Some(filters);
    conf
}

/// The control: a sixteen band parametric equaliser on every channel and no
/// convolution anywhere.
fn biquad_config() -> config::Configuration {
    let names: Vec<String> = (0..BIQUAD_PARAMS.len())
        .map(|idx| format!("bq_{idx}"))
        .collect();
    let steps = format!(
        r#"[{{"type":"Filter","names":[{}]}}]"#,
        names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut conf = base_config(&steps);
    let mut filters = HashMap::new();
    for (name, (freq, q)) in names.into_iter().zip(BIQUAD_PARAMS) {
        filters.insert(name, biquad_filter(freq, q));
    }
    conf.filters = Some(filters);
    conf
}

fn params() -> Arc<ProcessingParameters> {
    Arc::new(ProcessingParameters::new(&[0.0_f32; 5], &[false; 5]))
}

/// Single threaded, matching the `multithreaded: false` default. No pool means
/// no `parallelize_filters` pass, which is a small extra on top of everything
/// measured here for the users who do enable it.
fn build(conf: config::Configuration) -> Pipeline {
    Pipeline::from_config(conf, params(), None, &mut ConvCoeffCache::new())
}

/// As `build`, but with the coefficients already read and transformed, the way
/// the processing thread receives them from the supervisor.
fn build_prepared(input: (config::Configuration, ConvCoeffCache)) -> Pipeline {
    let (conf, mut cache) = input;
    Pipeline::from_config(conf, params(), None, &mut cache)
}

/// Transform straight from the fixture files, as the supervisor does for a
/// config whose impulse responses it was not handed.
fn transformed_from_files(conf: &config::Configuration, names: &[String]) -> ConvCoeffCache {
    ConvCoeffCache::transformed(conf, names, &ImpulseCache::new())
        .expect("the fixture files are readable")
}

/// Everything config validation read for `conf`, as the supervisor is handed
/// it.
fn validated_impulses(conf: &config::Configuration) -> ImpulseCache {
    let mut conf = conf.clone();
    config::validate_config(&mut conf, None).expect("the fixture config is valid")
}

fn bench_pipeline_build(c: &mut Criterion) {
    let (dir, sizes) = fixtures();
    eprintln!("coefficient fixtures in {}", dir.display());
    for (length, bytes) in sizes {
        eprintln!(
            "  {length} taps: {} channels x {:.1} MB = {:.1} MB read per reload",
            CHANNELS,
            *bytes as f64 / 1e6,
            *bytes as f64 * CHANNELS as f64 / 1e6
        );
    }
    eprintln!(
        "chunk is {:.1} ms at {CHUNK_SIZE} frames and {SAMPLERATE} Hz; \
         the playback side runs dry somewhere around 50-100 ms of stall",
        1000.0 * CHUNK_SIZE as f64 / SAMPLERATE as f64
    );

    let mut group = c.benchmark_group("pipeline_build");

    // The control. Expected to be orders of magnitude below everything else.
    group.bench_function(BenchmarkId::new("build", "biquad_only"), |b| {
        b.iter_batched(
            biquad_config,
            build,
            // Returned pipelines are dropped outside the timed section, so this
            // is the build cost alone.
            BatchSize::PerIteration,
        )
    });

    for length in FIR_LENGTHS {
        // Coefficients inline in the config: the transforms and the allocation
        // with no file system in the picture. The floor for a conv reload.
        group.bench_function(BenchmarkId::new("build", format!("values_{length}")), |b| {
            b.iter_batched(
                || conv_config(length, false),
                build,
                BatchSize::PerIteration,
            )
        });

        // The same pipeline read from disk. The difference against the arm
        // above is the read and the parse, with the page cache warm.
        group.bench_function(BenchmarkId::new("build", format!("file_{length}")), |b| {
            b.iter_batched(|| conv_config(length, true), build, BatchSize::PerIteration)
        });

        // ConfigChange::FilterParameters, which is what the GUI sends when a
        // coefficient file is swapped. It never goes through from_config, so it
        // has to be measured separately, and it reads the files just the same.
        let names = conv_names();
        let mut pipeline = build(conv_config(length, true));
        group.bench_function(BenchmarkId::new("update", format!("file_{length}")), |b| {
            b.iter_batched(
                || conv_config(length, true),
                |conf| {
                    pipeline.update_parameters(conf, &names, &[], &mut ConvCoeffCache::new());
                },
                BatchSize::PerIteration,
            )
        });

        // The same two passes with the reading and the transforms already done
        // on the supervisor thread. This is what the processing thread actually
        // pays now, and the gap against the arms above is what moving the work
        // bought.
        group.bench_function(
            BenchmarkId::new("build", format!("prepared_{length}")),
            |b| {
                b.iter_batched(
                    || {
                        let conf = conv_config(length, true);
                        let cache = transformed_from_files(&conf, &conv_names());
                        (conf, cache)
                    },
                    build_prepared,
                    BatchSize::PerIteration,
                )
            },
        );

        let mut warm_pipeline = build(conv_config(length, true));
        group.bench_function(
            BenchmarkId::new("update", format!("prepared_{length}")),
            |b| {
                b.iter_batched(
                    || {
                        let conf = conv_config(length, true);
                        let cache = transformed_from_files(&conf, &names);
                        (conf, cache)
                    },
                    |(conf, mut cache)| {
                        warm_pipeline.update_parameters(conf, &names, &[], &mut cache);
                    },
                    BatchSize::PerIteration,
                )
            },
        );

        // Freeing the old pipeline happens on the processing thread too, right
        // after the new one is swapped in, so it adds to the same stall.
        group.bench_function(BenchmarkId::new("drop", format!("file_{length}")), |b| {
            b.iter_batched(
                || build(conv_config(length, true)),
                drop,
                BatchSize::PerIteration,
            )
        });

        // The supervisor's own half of a reload, the work that used to be the
        // processing thread's. `from_files` is what it costs with nothing
        // carried over from validation; `from_validated` is what it costs with
        // the impulse responses validation read, so the gap between them is
        // what not reading the files twice is worth.
        group.bench_function(
            BenchmarkId::new("prepare", format!("from_files_{length}")),
            |b| {
                b.iter_batched(
                    || conv_config(length, true),
                    |conf| transformed_from_files(&conf, &conv_names()),
                    BatchSize::PerIteration,
                )
            },
        );

        group.bench_function(
            BenchmarkId::new("prepare", format!("from_validated_{length}")),
            |b| {
                let conf = conv_config(length, true);
                let impulses = validated_impulses(&conf);
                b.iter_batched(
                    || conf.clone(),
                    |conf| {
                        ConvCoeffCache::transformed(&conf, &conv_names(), &impulses)
                            .expect("the impulse responses are all cached")
                    },
                    BatchSize::PerIteration,
                )
            },
        );
    }

    group.finish();
}

/// Same reasoning as `benches/pipeline.rs`: these iterations take milliseconds,
/// and criterion budgets by wall time rather than by iteration count, so the
/// default 3 s warm-up and 5 s measurement would spend all of it repeating a
/// reading that stabilised immediately.
fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50)
        .without_plots()
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_pipeline_build
}
criterion_main!(benches);
