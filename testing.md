# Testing

## Built-in tests
Some of the functionality is covered by tests implemented in Rust.
These tests are run via cargo:
```sh
cargo test
```

## End-to-end tests
A pytest suite drives the real binary.
Each test starts `camilladsp` as a child process with a config, controls it over the websocket,
and shuts it down again.
It covers the lifecycle and exit codes, the config command surface including rapid config churn,
volume, mute and the faders, the signal level getters, the spectrum analysis, the pushed event
subscriptions, the state file, the error paths, the stalled and paused states, the rate
control loop, the capture side resampler, the clipping that comes with converting to a
sample format, the stop reasons left behind by a device failure, a sample rate change or
a stream that ends, the processing itself through a mixer and a filter, and the file devices
in every combination of a paced and a free-running end.

Most of the tests run on the test-only dummy capture and playback devices,
so they need a build with the `dummy-backend` feature.
They also need the Python packages `pytest`, `pytest-timeout`, `websocket-client`
and `numpy`.

```sh
cargo build --profile e2e --features dummy-backend
pip install pytest pytest-timeout websocket-client numpy
pytest -v testscripts/e2e -m "not stock"
```

A complete run takes a couple of minutes.

The rest carry the `stock` marker and run against a build without the feature, which is the
binary a release ships.
They cover the raw file, wav, stdin, stdout and generator devices in every sample format,
the wav header and its RF64 variant, resampling with both ends of the pipeline free running,
and the processing asserted sample by sample rather than through a meter.
They also check that a stock build has no dummy devices and rejects a config asking for one.

```sh
cargo build --profile e2e
pytest -v testscripts/e2e -m stock
```

That run takes a couple of seconds, since nothing in it is paced.
Both builds land on the same path, so the two selections are two commands and not one.
The `e2e` profile is optimised because the devices are paced in real time and an unoptimised
build cannot keep up with itself, see Cargo.toml.
The tests use the most recently built binary among the `e2e`, `release-fast`, `release` and
`debug` profiles, and `CAMILLADSP_BIN` overrides that.
See `testscripts/e2e/README.md` for the layout, for why the suite is in Python rather than
Rust, and for what to know before adding tests.

# Benchmarks

There are benchmarks to monitor the performance of some filters.
These use the `criterion` framework.
Run them with cargo:
```sh
cargo bench
```

## FFT Convolution Kernel Benchmarks

Micro-benchmarks for the complex-multiply kernels are available.
They compare scalar against NEON on `aarch64` and AVX+FMA on `x86_64`.
The `bench` feature flag is required to expose the kernel functions:
```sh
cargo bench --features bench --bench fftconv_kernels
```
