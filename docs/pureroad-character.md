# PureRoad character processor

`PureroadCharacter` is a stereo CamillaDSP processor containing selected,
statically linked Airwindows DSP kernels. It does not load VST, LV2 or LADSPA
plugins and has no runtime plugin-host dependency.

The supported algorithms are `Acceleration2` and `ToTape8`, pinned to Airwindows commit
`781eaee378303c7dc4d9edcaabb086cf160ff5df`. Its MIT notice is in
`THIRD_PARTY_NOTICES.md`.

## Activation model

The mapping key under `processors` is only an instance name. Defining an
instance does not run it: the pipeline must contain a matching `Processor`
step. Conversely, putting two character instances in the pipeline runs both in
series; it does not select between them. The production control model should
normally keep one `pureroad_character` instance in the pipeline and update its
`algorithm` between `Original`, `Acceleration2` and `ToTape8`.

`Original` is the user-facing off state and is bit-exact. Keep `mix: 1.0` in
the stored configuration so switching from `Original` to either enabled
algorithm requires changing only `algorithm` (plus the chosen algorithm's
controls). `mix: 0.0` is also a bit-exact, CPU-saving bypass, but it is intended
as an internal dry/wet/transition control rather than the normal user switch.
The processor rejects configurations at 40 kHz or below instead of silently
constructing an unavailable native kernel; production validation covers 44.1,
48, 88.2, 96, 176.4 and 192 kHz.

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

For a product UI, expose `Original`, `Acceleration2` and `ToTape8` as the three
sound choices. `Acceleration2` needs at most one user control, `intensity`.
Keep `mix: 1.0`, `transition_ms: 100.0`, `channels: 2`, and all nine ToTape8
controls as implementation/preset values. In particular, a permanent partial
`mix` is not a good ToTape8 strength control because its flutter, filters and
clip stage are not guaranteed to remain phase-aligned with the dry path.

All ToTape8 controls are normalized to `0.0..=1.0`:

| Control | Meaning | Neutral / operational guidance |
| --- | --- | --- |
| `input` | Drive into the tape model; gain is `(value * 2)^2` | `0.5` is unity; `1.0` is +12.04 dB; `0.0` mutes the wet input |
| `tilt` | Dubly encode/decode emphasis balance | Keep `0.5` in the first production preset |
| `shape` | Tape split/filter shape | Keep `0.5` until listening and response validation select a preset |
| `flutter` | Modulated delay depth | `0.5` default; higher values increase pitch/time movement nonlinearly |
| `flutter_speed` | Flutter modulation speed | `0.5` default; effective rate rises cubically |
| `bias` | Asymmetric slew/bias character | `0.5` is centered; treat either direction as a creative preset parameter |
| `head_bump` | Low-frequency head-bump drive and mix | `0.5` default; higher values add more low-frequency resonance |
| `head_bump_frequency` | Head-bump center-frequency control | Maps approximately from 25 Hz at `0.0` to 200 Hz at `1.0` |
| `output` | Output gain before the safety clip stage | `0.5` is unity; `1.0` is +6.02 dB; `0.0` mutes |

The upstream-default candidate baseline is therefore all `0.5`; it is not yet
a product safety claim. Do not expose the nine raw
ToTape8 values to ordinary users until named presets have passed level-matched
listening, true-peak and hardware headroom tests. Measurement, FIR calibration, bit-exact comparison and fault
isolation should use `Original`; normal listening enables a character only by
an explicit persisted user selection, not automatically per track.

Direct changes between `Acceleration2`, `ToTape8` and `Original` use a wet
crossfade; all nine ToTape8 parameters are independently smoothed. If another
mode request arrives before a crossfade finishes, the processor retains only
the newest request and applies it after the active transition completes.

Place the processor at the desired point in the pipeline. Capture-side sample
rate conversion happens before the pipeline, so this processor sees the final
CamillaDSP processing sample rate. `Original` and `mix: 0.0` are bit-exact
bypasses. A fully dry processor skips the native wet path; resuming the same
algorithm resets its delay and filter history before restoring the wet signal
according to `transition_ms` (values above zero fade it in), so pre-bypass audio
cannot leak into the resumed output. Algorithm
changes ramp the wet mix over `transition_ms`; intensity changes are smoothed
inside the native kernel over the same interval.

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
