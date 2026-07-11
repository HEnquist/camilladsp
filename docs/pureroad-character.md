# PureRoad character processor

`PureroadCharacter` is a stereo CamillaDSP processor containing selected,
statically linked Airwindows DSP kernels. It does not load VST, LV2 or LADSPA
plugins and has no runtime plugin-host dependency.

The supported algorithms are `Acceleration2` and `ToTape8`, pinned to Airwindows commit
`781eaee378303c7dc4d9edcaabb086cf160ff5df`. Its MIT notice is in
`THIRD_PARTY_NOTICES.md`.

```yaml
processors:
  gentle_character:
    type: PureroadCharacter
    parameters:
      channels: 2
      algorithm: Acceleration2
      intensity: 0.32
      mix: 1.0
      transition_ms: 100.0

pipeline:
  - type: Processor
    name: gentle_character
  # Optional explicit placement of CamillaDSP's global user volume. Without
  # this step, legacy behavior applies it before the pipeline.
  - type: DefaultVolume
```

ToTape8 keeps all nine official normalized controls. Omitting the `totape8`
block uses Airwindows' `0.5` defaults:

```yaml
processors:
  tape_character:
    type: PureroadCharacter
    parameters:
      channels: 2
      algorithm: ToTape8
      mix: 1.0
      transition_ms: 100.0
      totape8:
        input: 0.5
        tilt: 0.5
        shape: 0.5
        flutter: 0.5
        flutter_speed: 0.5
        bias: 0.5
        head_bump: 0.5
        head_bump_frequency: 0.5
        output: 0.5
```

Direct changes between `Acceleration2`, `ToTape8` and `Original` use a wet
crossfade; all nine ToTape8 parameters are independently smoothed. If another
mode request arrives before a crossfade finishes, the processor retains only
the newest request and applies it after the active transition completes.

Place the processor at the desired point in the pipeline. Capture-side sample
rate conversion happens before the pipeline, so this processor sees the final
CamillaDSP processing sample rate. `Original` and `mix: 0.0` are bit-exact
bypasses. Algorithm changes ramp the wet mix over `transition_ms`; intensity
changes are smoothed inside the native kernel over the same interval.

The real-time path performs no allocation, locking, logging or I/O. Native allocation or
processing failure, non-finite output, channel mismatch and oversized chunks
fail open to the unmodified input. A non-finite input chunk is bypassed and the
native state is reset.

Run the validation matrix with:

```sh
cargo test --all-targets
cargo test --features 32bit pureroad_character --lib
cargo clippy --lib -- -D warnings -A clippy::collapsible-match
cargo build --release
cargo build --release --features 32bit
cargo test --release host_realtime_budget_diagnostic --lib -- --ignored --nocapture
cargo test --release totape8_sweep_and_crossfade_realtime_budget_diagnostic --lib -- --ignored --nocapture
```

The host timing diagnostic is not a substitute for ROCK 5C soak testing. Before
release, run 24-hour playback at the maximum supported rate and record xrun,
CPU, temperature, rate-change and bypass-recovery results on production hardware.

PureRoad's production target is CamillaDSP's default f64 processing path. The
optional `32bit` build runs the same double-precision Acceleration2 and ToTape8
kernels with f32 input/output conversion and deliberately omits Airwindows'
float-output dither; it is tested for safety and consistency, but neither
algorithm is claimed to be sample-identical to Airwindows VST
`processReplacing` in a 32-bit build.
