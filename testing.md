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
volume, mute and the faders, the signal level getters, the state file, and the error paths.

The tests run on the test-only dummy capture and playback devices,
so they need a build with the `dummy-backend` feature.
They also need the Python packages `pytest` and `websocket-client`.

```sh
cargo build --features dummy-backend
pip install pytest websocket-client
pytest -v testscripts/e2e
```

A complete run takes about a minute.
The tests look for `target/debug/camilladsp`, set `CAMILLADSP_BIN` to test another build.
See `testscripts/e2e/README.md` for the layout and for what to know before adding tests.

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
