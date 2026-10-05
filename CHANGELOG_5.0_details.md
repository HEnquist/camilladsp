# CamillaDSP 5.0.0, detailed changes
This is the full list of changes in 5.0.0, with background and measurements.
See [CHANGELOG.md](CHANGELOG.md) for the short version.

## New features
- Config validation already reads and checks every coefficient file, so what it read is now kept
  and reused when the config is applied, rather than being thrown away and read a second time.
  Applying a config with large FIR filters no longer reads or transforms anything on the processing
  thread, so it no longer stalls the audio. Not reading the files twice also removes the chance of
  the second read failing on a file that has moved since.
- Websocket commands for streaming signal level and state change events.
- Websocket commands for audio spectrum data (single read & streaming).
- Websocket command for getting device capabilities.
- New `Slip` resampler for very cheap rate adjust between independent clocks at the same nominal rate.
- RF64 support for reading and writing wav files larger than 4 GB (`use_rf64` for File playback).
- New `LookaheadLimiter`, as a single-channel filter and as a multichannel processor with
  configurable monitor and process channels.
- ASIO: capture and playback can now use two different ASIO devices. Previously both sides had to
  use the same one.
- The corner frequency and Q of the two `Loudness` shelves can be set with the new `high_freq`,
  `low_freq`, `high_q` and `low_q` parameters. They were previously fixed.
- PipeWire capture has a new `loopback` parameter for capturing from the output of a sink instead
  of from a source, matching the `loopback` parameter of the WASAPI backend. This is also what
  makes `autoconnect_to` accept the name of a sink, since WirePlumber only considers sources when
  it resolves a capture target by name.
- PipeWire capture and playback now request their sample rate as the graph rate via `node.rate`.
  A multi-rate DAC can then run at the rate of the active config, if PipeWire is configured to
  allow it with `default.clock.allowed-rates`. With the default settings nothing changes.

## Bugfixes
- Stricter validation of numeric config values. `devices.samplerate` and `devices.capture_samplerate`
  must now be larger than zero, previously `samplerate: 0` reached the coefficient math and
  `capture_samplerate: 0` made the process hang or panic once the resampler was built.
- The free `AsyncSinc` resampler parameters are now validated. `sinc_len` must be larger than zero,
  `oversampling_factor` must be large enough for the chosen interpolation, and `f_cutoff` must be
  larger than 0 and no larger than 1.0. These previously panicked inside the resampler on the first
  chunk, rather than being reported when the config was loaded.
- `rate_measure_interval_s` must now be larger than zero, matching `adjust_interval_s`.
- A mixer mapping that lists the same source channel twice is now rejected. The check existed but
  never fired, so the channel was simply mixed in twice.
- The `Compressor` now rejects a `factor` of zero, which previously gave every sample above the
  threshold an infinite gain. Values below 1.0 are still allowed, for upward expansion.
- No numeric config value accepts `.nan`, `.inf` or `-.inf` any more. The range tests were written
  so that every comparison against NaN passed, and one-sided tests let infinities through. Every
  float field now has type `FiniteF64` or `FiniteF32`, so the rule is part of the field
  declaration and a field added later cannot forget it. The types are confined to the config
  module, the rest of CamillaDSP reads plain floats through getters, and they serialize as plain
  numbers so the websocket config format is unchanged.
- Fields that must be larger than zero now have type `NonZeroUsize` rather than a validator that
  had to be attached by hand. This covers `samplerate`, `chunksize`, `capture_samplerate`, every
  device `channels`, the mixer channel counts, and the dummy convolution `length`.
- PipeWire: an `autoconnect_to` target that cannot be found now leaves the node unconnected,
  instead of falling back to the default device and capturing from or playing to the wrong node.
  The node is connected automatically if the target appears later.
- ASIO: size the ring buffer and prefill from the driver's actual buffer size instead of just
  `chunksize`, fixing continuous underruns when the driver requests a larger buffer than `chunksize`.
- A convolution filter with an empty inline `values` list is now rejected by the config
  validation, instead of being accepted and then panicking on the first chunk.
- The `DiffEq` filter now rejects unstable coefficients. The `a` coefficients are checked with the
  Schur-Cohn stability test when the config is loaded, and a filter with poles on or outside the
  unit circle is no longer accepted and then allowed to run away to full scale.
- The `DiffEq` filter now scales its coefficients so that a0 becomes unity. The first `a`
  coefficient was previously ignored, so any value other than 1.0 gave a filter with the wrong
  gain compared to the documented transfer function.
- Changing the parameters of a `Biquad` filter on the fly no longer produces a large transient.
  The filter state is kept as before, but is now scaled down when the new coefficients would ring
  louder from it than the old ones would have. A pole near DC amplifies inherited state by a factor
  of several hundred, so swapping a peaking EQ for a 25 Hz highpass while audio was playing
  previously peaked at sixteen times full scale.
- `chunksize` must now be larger than zero. A `chunksize` of zero was accepted as a valid
  configuration and then hung on startup without producing any audio. A samplerate override that
  would scale a small `chunksize` down to zero now keeps one frame instead.
- CoreAudio and WASAPI playback copy the audio data as whole slices instead of one byte at a time,
  which lowers the CPU load of the real-time playback thread.
- The clipped samples counter no longer loses counts. The clipped samples of a chunk were dropped
  whenever the playback status was busy, for example while a websocket client read it, so the
  counter read low on a loaded machine, which is when clipping is most likely.
- Threaded ALSA (`threaded-alsa` feature): rate adjust no longer corrects twice when the device has
  a pitch control. Capture from a Loopback or UAC2 gadget with an async resampler adjusted both
  the pitch control and the resampler, and playback to a UAC2 gadget adjusted both the gadget pitch
  and the capture speed. Like the default ALSA backend, it now uses the pitch control when there
  is one, and otherwise the resampler or the capture speed.
- File capture now stops on a read error. It previously reported the error but kept going, passing
  on the data of the previous read as if it were new.
- ASIO: when capture timed out waiting for the driver, the part of the chunk that had not arrived
  was filled with leftover audio from the previous chunk. It is now filled with silence.
- PipeWire: losing the connection to the processing thread is now reported as a playback error
  rather than as a normal end of playback, like the other backends do.
- WASAPI exclusive mode no longer asks the driver about 24-bit formats in the WAVEFORMATEX form,
  which cannot tell packed 24-bit samples from padded ones. A driver could accept the format there
  and then treat the samples as the other layout.

## Changes
- Checking a config no longer stops at the first problem. Every problem is listed, one per line,
  each starting with where it is in the config, like `filters.lp.parameters.freq`. A YAML error
  still stops the parsing, so only that one is listed.
- `Volume` filters in the pipeline that use the same fader must now have the same `ramp_time_ms`
  and `limit`, since these now belong to the fader. A `Loudness` filter on an Aux fader without a
  `Volume` filter now also follows `SetFaderVolume`, not only `SetFaderExternalVolume`.
- The ASIO backend no longer uses the ASIO SDK from Steinberg. It talks to the ASIO drivers
  directly through the COM interfaces they expose, using the `azo` crate. Windows builds with ASIO
  are therefore no longer restricted to GPLv3, and the usual dual license applies to every build.
  Building no longer requires the SDK, LLVM/Clang or `bindgen`.
- ASIO: the ASIO4ALL driver is refused with an error pointing to the Wasapi backend. It tolerates
  only one instance per process, which crashed CamillaDSP when a configuration was reloaded after a
  failed one. It only makes an ordinary Windows device reachable over ASIO, which the Wasapi backend
  already does, in exclusive mode with one emulation layer less.
- ASIO support is now always included in Windows builds. The `asio-backend` build feature is gone,
  and so is the separate `camilladsp-windows-asio-amd64.zip` download. The regular Windows binary
  now includes ASIO, and still runs on systems without any ASIO drivers installed, where it simply
  reports no available ASIO devices. Anyone building with `--features asio-backend` should drop
  the flag.
- Much faster biquad filtering. A biquad waits on its own feedback path, leaving the processor
  idle, so several independent ones are now run at once: several channels side by side, and
  several positions of a channel's cascade skewed against each other. A run of biquads in a
  filter step is compiled into one cascade per channel, so the same trick reaches across the
  filters inside the step. Measured per chunk at a chunksize of 1024: a pipeline of 16 biquads
  on four channels followed by 16 more on two went from 249 to 39 us, sixteen channels of three
  biquads went from 128 to 28 us, and a mixed pipeline of biquads and FIR filters went from 707
  to 481 us. Results are bit-identical to running the filters one at a time.
- Biquad filters are no longer sent to the thread pool when `multithreaded` is enabled, since
  running them several at a time already keeps the processor busy. The same biquad pipeline
  measured 39 us on the main thread against 222 us on the thread pool.
- Biquads now use fused multiply-add on hardware that has it, about 18% faster on aarch64. The
  fused form rounds once instead of twice, so results can differ from 4.1.3 in the last few bits.
- The `DiffEq` filter is now a direct form 2 transposed structure, the same form the biquads use,
  instead of two ring buffers addressed with modulo. Orders up to eight keep their state in
  registers for a whole chunk. About 1.4 times faster at second order and 3.4 times at eighth
  order. The new form rounds differently, so results can differ from 4.1.3 in the last few bits.
- Faster convolution setup and processing, and lower memory use. Three changes: convolution
  filters share one FFT planner instead of each building and discarding its own, channels that use
  the same filter share one copy of its transformed coefficients instead of each keeping a copy,
  and the segmented spectra are held in one contiguous allocation rather than one block per
  segment. Reloading eight 16384 tap filters at a chunksize of 16384 went from about 3.0 ms to
  about 0.9 ms, and a four channel pipeline with a one million tap filter went from 5.1 ms to
  3.8 ms per chunk with `multithreaded` enabled, using a quarter of the coefficient memory. The
  setup saving grows with `chunksize` and applies even when every channel uses a different impulse
  response. The coefficient sharing requires the channels to refer to the same named filter, and
  helps most with `multithreaded` enabled, where the copies would otherwise compete for memory
  bandwidth at the same moment. Without it the gain is smaller, and limited to filters large
  enough to crowd the cache but small enough that a single copy still fits.
- Improved DSP library separation for easier external integration.
- File playback now writes correct wav header sizes, and stops at the 4 GB limit for plain wav.
- The `websocket` build feature is gone. The websocket server is now always built in, since every
  known packager and build script enabled it anyway, and the control interface is what the GUI and
  the python bindings talk to. `secure-websocket` remains optional and no longer implies anything.
  There are now no default features, so `--no-default-features` has no effect and can be dropped
  from build commands.
- The `32bit` build feature is gone. 32-bit float processing is now selected with the compiler
  flag `RUSTFLAGS="--cfg camillafloat_f32"` instead. Cargo features are unified across the whole
  dependency graph, so as a feature it could be switched on by any other crate in a build that
  uses CamillaDSP as a library. Anyone building with `--features 32bit` needs to switch to the
  new flag.
- The sample type `PrcFmt` is renamed to `CamillaFloat`. The active precision is now shown as
  `Sample precision` in `camilladsp --help`.
- Configuration values and filter coefficient math are now always 64-bit, independent of the
  processing precision. An f32 build therefore parses configs, serialises them over the websocket,
  and computes filter coefficients exactly like a normal build, and rounds only once when the
  finished coefficients enter the processing path. This noticeably improves f32 accuracy for
  low-frequency biquads.
- The audio buffer used for spectrum analysis is now only filled after a client has asked for
  spectrum data. It was previously written on every chunk, on both the capture and playback
  threads, whether or not anything was reading it. Setups that never use the spectrum no longer
  pay for it. The first spectrum request after startup can report insufficient data until enough
  audio has accumulated, typically well under a tenth of a second.
- Spectrum analysis is done in 32-bit float, which halves the memory used by its audio buffer.
  The numerical noise floor stays far below the displayed range.
- The pre-built Linux binaries now need glibc 2.34 or newer, meaning Raspberry Pi OS Bookworm
  or another distribution of similar age. Older systems must build from source.
- No more pre-built armv6 binary for the Raspberry Pi 1 and the original Pi Zero.
  Those must build from source.
- Threaded ALSA and PipeWire capture now wait when the queue to the processing thread is full,
  instead of dropping the chunk. Every other backend already waited, and `queuelimit` is meant to
  bound the latency. Under overload the loss now happens at the device, which logs it.
- All backends handle a playback ring buffer that stays full the same way. They wait for the
  device to make room for up to eight chunk durations, then drop the whole chunk. CoreAudio, ASIO
  and PipeWire previously pushed as much of the chunk as fitted, and every backend except threaded
  ALSA gave up after half the time.
- Overruns and underruns are logged the same way in every backend. A warning is printed once when
  one starts, recovery from a playback underrun is logged at info, and the details of each event
  are logged at trace. Some backends previously warned on every chunk or callback, flooding the
  log, while others only logged at debug level.
- The capture command handling, the pushing of playback chunks to the device, and the playback
  side of rate adjust are now shared code, instead of one copy per backend.

## Config changes (breaking)
- The `FivePointPeq` biquad combo is extended to a free number of bands, and is renamed
  `NPointPeq`. The fifteen numbered parameters are replaced by a `bands` list of at least two
  entries, each with `freq`, `gain` and `q`. The first band is a low shelf, the last a high shelf,
  and the ones in between peaking filters, so an old five band equalizer becomes a list of five
  bands in the order it already used, `fls`/`gls`/`qls` first and `fhs`/`ghs`/`qhs` last. Two new
  rules come with it: the bands must be listed with rising frequency, and a band with a gain
  smaller than 0.001 dB is left out when the filter is built, which is how a band is disabled
  without removing it.
- Time values no longer accept unitless numbers. Every time-valued parameter now states its unit.
- Tunable times take a mandatory companion unit field:
  - `Delay` filter: `unit` renamed to `delay_unit` (now required).
  - `RACE` processor: `delay_unit` now required.
  - `Compressor` and `NoiseGate` processors: added required `attack_unit` and `release_unit`.
    The previous `attack`/`release` values were in seconds, so add `attack_unit: s` and `release_unit: s`
    to keep the old behavior.
  - `LookaheadLimiter` filter: the shared `unit` is split into `attack_unit` and `release_unit`.
- Fixed-unit times bake the unit into the field name:
  - `adjust_period` renamed to `adjust_interval_s` (also aligns wording with `rate_measure_interval_s`).
  - `silence_timeout` renamed to `silence_timeout_s`.
  - `rate_measure_interval` renamed to `rate_measure_interval_s`.
  - `volume_ramp_time` renamed to `volume_ramp_time_ms`.
  - `Volume` filter: `ramp_time` renamed to `ramp_time_ms`.
- Delay and RACE now also accept `s` (seconds) as a unit.
- The `Limiter` filter is renamed to `Clipper` (`type: Limiter` becomes `type: Clipper`), to avoid
  confusion with the new `LookaheadLimiter`. Its parameters are unchanged.
- General tweaks and improvements.

## Websocket protocol changes (breaking)
- Messages are now internally tagged with a uniform object shape.
  - Commands carry the name in a `command` field, with arguments in named fields:
    `"GetVersion"` becomes `{"command": "GetVersion"}`, and `{"SetUpdateInterval": 500}` becomes
    `{"command": "SetUpdateInterval", "value": 500}`.
  - Replies carry the name in a `reply` field as a single flat object:
    `{"GetUpdateInterval": {"result": "Ok", "value": 500}}` becomes
    `{"reply": "GetUpdateInterval", "result": "Ok", "value": 500}`.
  - Errors are flat too: `result` holds the error name, and any description rides at the top level
    in a `message` field, replacing the previous double-nested shape.
  - Commands that took multiple arguments now use named fields instead of an array, for example
    `AdjustVolume` takes `value` plus optional `min` and `max`.

## Removed
- Dropped the Jack, Pulse and Bluez backends. On Linux, use the native PipeWire backend, or
  PipeWire's Pulse/JACK compatibility layers. PipeWire can also bridge Bluetooth A2DP directly.
