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

- `default = ["websocket"]`: enables the websocket control server by default. Main implementation: `src/socketserver.rs`, with startup and CLI integration in `src/bin.rs` and module gating in `src/lib.rs`.
- `threaded-alsa`: switches Linux ALSA playback and capture over to the threaded ALSA backend instead of the legacy backend. Main implementation switch: `src/alsa_backend/mod.rs`. Threaded code lives in `src/alsa_backend/threaded_device.rs` and `src/alsa_backend/threaded_buffermanager.rs`; legacy code lives in `src/alsa_backend/device.rs` and `src/alsa_backend/buffermanager.rs`.
- `pulse-backend`: enables the Linux PulseAudio backend and related config parsing. Main implementation: `src/pulse_backend/device.rs`, with gating in `src/lib.rs`, `src/audiodevice.rs`, `src/config/mod.rs`, `src/config/utils.rs`, and `src/bin.rs`.
- `pipewire-backend`: enables the Linux PipeWire backend. Main implementation: `src/pipewire_backend/device.rs`, with module gating in `src/lib.rs`.
- `cpal-backend`: enables the CPAL-based backend support. Main implementation: `src/cpal_backend/device.rs`, with supporting conversion helpers in `src/utils/conversions.rs` and module gating in `src/lib.rs`.
- `jack-backend`: enables JACK support through CPAL rather than a separate backend module. Main implementation is still `src/cpal_backend/device.rs`; this feature extends `cpal-backend` and activates CPAL's JACK support in `Cargo.toml`.
- `bluez-backend`: enables BlueZ and D-Bus integration used by the file backend Bluetooth support. Main implementation: `src/file_backend/bluez.rs`.
- `asio-backend`: enables the Windows ASIO backend. Main implementation: `src/asio_backend/device.rs` and `src/asio_backend/utils.rs`, with module gating in `src/lib.rs`.
- `32bit`: changes the internal processing sample type from `f64` to `f32`. Main type definition: `src/lib.rs` (`PrcFmt`), with numerics and conversion consequences across the DSP codebase, especially `src/utils/conversions.rs`, `src/utils/resampling.rs`, and filter implementations.
- `bench`: enables benchmark-only code paths needed by Criterion benches. Main gated code: `src/filters/fftconv.rs`, and the benches themselves live in `benches/`.
- `websocket`: enables the websocket control and monitoring server. Main implementation: `src/socketserver.rs`, with runtime setup in `src/bin.rs` and module gating in `src/lib.rs`.
- `secure-websocket`: adds TLS support on top of `websocket`. Main implementation: TLS-specific branches in `src/socketserver.rs` and certificate-related CLI/runtime handling in `src/bin.rs`.
- `debug`: enables extra trace and debug-only instrumentation, not a separate subsystem. Representative gated locations: `src/lib.rs`, `src/wasapi_backend/device.rs`, `src/coreaudio_backend/device.rs`, and `src/cpal_backend/device.rs`.

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
	- `src/file_backend/bluez.rs`

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
