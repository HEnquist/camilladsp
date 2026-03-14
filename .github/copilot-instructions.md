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
- `src/countertimer.rs`: shared timing/averaging/watch utilities (`Stopwatch`, `Averager`, `TimeAverage`, `ValueWatcher`, `SilenceCounter`, `ValueHistory`).
	Reused by all major backend device files and status reporting.
- `src/helpers.rs`: shared DSP/helper math and control loops (`multiply_elements`, `multiply_add_elements`, `linear_to_db`, `PIRateController`).
	Used in FFT convolution, websocket reporting, and backend rate adjust loops.
- Backend-specific utility modules:
	- `src/alsadevice_utils.rs`
	- `src/asiodevice_utils.rs`
	- `src/filedevice_bluez.rs`

When debugging or implementing cross-backend behavior, inspect these utility modules before editing backend-specific loops.

## Working conventions
- For backend/device changes, inspect the matching `src/*device*.rs` files and relevant `backend_*.md` docs.
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
