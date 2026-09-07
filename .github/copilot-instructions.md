# Copilot instructions for CamillaDSP

This repository is the **CamillaDSP engine** (Rust).

## Primary goals
- Keep changes minimal, focused, and production-safe.
- Prefer fixing root causes over quick workarounds.
- Preserve existing behavior unless a change is explicitly requested.
- Match existing style and naming in surrounding Rust code.

## Repository map
- `src/`: main Rust codebase (DSP, devices, config, pipeline, filters, processors).
- `exampleconfigs/`: YAML configuration examples used by users.
- `backend_*.md`: backend-specific docs (`ALSA`, `Wasapi`, `PipeWire`, `CoreAudio`).
- `README.md`: main user and build documentation.
- `filterfunctions.md`, `sample_formats.md`, `websocket.md`: domain docs and references.
- `benches/`: criterion benches.

## Cargo feature map
The authoritative feature list is in `Cargo.toml`. When a task depends on optional functionality, check both the feature gate and any platform gate before editing.

There are no default features. The websocket control server is always built in, see `src/websocket_server/mod.rs`, with helper code in `src/websocket_server/utils.rs` and runtime setup in `src/bin.rs` and `src/engine.rs`.

- `threaded-alsa`: switches Linux ALSA playback and capture over to the threaded ALSA backend instead of the legacy backend. Main implementation switch: `src/alsa_backend/mod.rs`. Threaded code lives in `src/alsa_backend/threaded_device.rs` and `src/alsa_backend/threaded_buffermanager.rs`; legacy code lives in `src/alsa_backend/device.rs` and `src/alsa_backend/buffermanager.rs`.
- `pipewire-backend`: enables the Linux PipeWire backend. Main implementation: `src/pipewire_backend/device.rs`, with module gating in `src/lib.rs`.
The Windows ASIO backend is always built on Windows, gated only on `cfg(target_os = "windows")`. Main implementation: `src/asio_backend/device.rs`, `src/asio_backend/driver.rs` and `src/asio_backend/utils.rs`, with module gating in `src/lib.rs`.
The sample precision is not a Cargo feature. It is the rustc cfg `camillafloat_f32`, set with `RUSTFLAGS="--cfg camillafloat_f32"`, which changes the internal processing sample type from `f64` to `f32`. Main type definition: `src/lib.rs` (`CamillaFloat`), with numerics and conversion consequences across the DSP codebase, especially `src/utils/conversions.rs`, `src/utils/resampling.rs`, and filter implementations. Because it is a cfg rather than a feature, `--all-features` does not cover it and it needs its own build to verify.

Rule when adding or changing filters: setup runs in `f64`, processing runs in `CamillaFloat`, telemetry is `f32`. Config values (`src/config/mod.rs`) and coefficient math are always `f64`. Convert to the processing precision exactly once, at the point where a finished value is stored for per-sample use, using `ToCamillaFloat::to_camilla_float()`. `src/filters/biquad.rs` is the reference example: `BiquadCoefficients` is `f64`, `RuntimeCoefficients` is `CamillaFloat`.

Values that only get reported (signal levels, volumes in dB, spectrum data) are `f32`, matching the websocket API, and are converted with `ToF32::to_f32()`. Use these traits rather than `as` casts, so the direction that is a no-op in a given build does not need a `clippy::unnecessary_cast` allow.

The SIMD convolution kernels in `src/filters/fftconv_avx.rs` and `src/filters/fftconv_neon.rs` exist in both precisions and are always compiled, dispatched through the `ConvKernel` trait in `src/filters/fftconv.rs`. Do not put precision cfgs back in those files.
- `bench`: enables benchmark-only code paths needed by Criterion benches. Main gated code: `src/filters/fftconv.rs`, and the benches themselves live in `benches/`.
- `secure-websocket`: adds TLS support to the websocket server. Main implementation: TLS-specific branches in `src/websocket_server/mod.rs` and certificate-related CLI/runtime handling in `src/bin.rs`.
- `debug`: enables extra trace and debug-only instrumentation, not a separate subsystem. Representative gated locations: `src/lib.rs`, `src/wasapi_backend/device.rs`, and `src/coreaudio_backend/device.rs`.

When changing backend selection, config parsing, CLI flags, or websocket behavior, verify the relevant feature-gated code paths and do not assume the default build includes every backend.

## Shared utility index
- `src/utils/resampling.rs`: shared resampler wrapper and selection (`ChunkResampler`, `new_resampler`, `resampler_is_async`).
	Reused by all major backend device files.
- `src/utils/conversions.rs`: shared sample format and buffer/chunk conversion helpers.
	Reused by all major backend device files.
- `src/utils/countertimer.rs`: shared timing/averaging/watch utilities (`Stopwatch`, `Averager`, `TimeAverage`, `ValueWatcher`, `SilenceCounter`, `ValueHistory`).
	Reused by all major backend device files and status reporting.
- `src/utils/decibels.rs`: shared dB/linear conversion helpers (`linear_to_db`, `linear_to_db_inplace`, `db_to_linear`, `gain_from_value`).
	Used in websocket reporting and gain/rate-related paths.
- `src/utils/rate_controller.rs`: rate adjust control loop (`PIRateController`).
- `src/utils/stash.rs`: shared audio/vector stash allocation and recycling (`vec_from_stash`, `container_from_stash`, `recycle_chunk`).
- `src/audiochunk.rs`: `AudioChunk`/`ChunkStats` structures and chunk statistics helpers.
- Backend-specific utility modules:
	- `src/alsa_backend/utils.rs`
	- `src/asio_backend/utils.rs`

When debugging or implementing cross-backend behavior, inspect these utility modules before editing backend-specific loops.

## Working conventions
- For backend/device changes, inspect the matching `src/**/*device*.rs` files and relevant `backend_*.md` docs.
- For config/schema changes, update both Rust config handling and docs/examples where needed.
- Keep public YAML keys and CLI behavior backward compatible unless explicitly requested.
- Do not add new dependencies unless clearly justified.

## Validation checklist
When practical, run targeted checks for touched areas before broad checks:
1. `cargo fmt`
2. `cargo clippy --all-targets --all-features`
3. `cargo test`

If a full check is too heavy, run the smallest relevant command and state what was skipped.

## Documentation expectations
- Update docs in the same change when user-facing behavior changes.
- Prefer editing `README.md` for cross-cutting behavior and backend markdown files for backend details.
- Keep wording concrete and avoid introducing undocumented options.
