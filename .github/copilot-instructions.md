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

## Shared utility index
- `src/resampling.rs`: shared resampler wrapper and selection (`ChunkResampler`, `new_resampler`, `resampler_is_async`).
	Reused by all major backend device files.
- `src/conversions.rs`: shared sample format and buffer/chunk conversion helpers.
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
