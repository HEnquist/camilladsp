# 5.0.0
New features:
- Websocket commands for streaming signal level and state change events.
- Websocket commands for audio spectrum data (single read & streaming).
- Websocket command for getting device capabilities.
- New `Slip` resampler for very cheap rate adjust between independent clocks at the same nominal rate.
- RF64 support for reading and writing wav files larger than 4 GB (`use_rf64` for File playback).

Bugfixes:
- ASIO: size the ring buffer and prefill from the driver's actual buffer size instead of just
  `chunksize`, fixing continuous underruns when the driver requests a larger buffer than `chunksize`.

Changes:
- Improved DSP library separation for easier external integration.
- File playback now writes correct wav header sizes, and stops at the 4 GB limit for plain wav.
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

Config changes (breaking):
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

Websocket protocol changes (breaking):
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

Removed:
- Dropped the Jack, Pulse and Bluez backends. On Linux, use the native PipeWire backend, or
  PipeWire's Pulse/JACK compatibility layers. PipeWire can also bridge Bluetooth A2DP directly.

# 4.1.3
Bugfixes:
- Increased capture ringbuffer sizes in CoreAudio, WASAPI, ASIO, and threaded ALSA
  to avoid stalling capture with very large downsampling ratios.

# 4.1.2
Bugfixes:
- Removed memory allocations in real time audio paths to improve stability.

# 4.1.1
Bugfixes:
- Correct mistake in version string.

## 4.1.0
New features:
- Experimental optional multi-threaded Alsa backend.
- Added SIMD acceleration for FFT convolution,
  using NEON on aarch64 and AVX/FMA on amd64.

Bugfixes:
- Linux: fix stuttering on PipeWire playback.

## v4.0.1
Changes:
- Improved documentation on sample formats and rate adjust

## v4.0.0
New features:
- Add PipeWire backend.
- Add ASIO backend.
- Add RACE processor.
- Add option for custom log filtering.
- Support polling mode for WASAPI.
- Websocket commands for reading and writing partial configs.
- Websocket command for reading resampler load.

Changes:
- New sample format names on all backends.
- Removed sample format selection for Pulse backend.
- Add microsecond delay unit.
- Allow larger buffer target levels.
- Change mixer config rules to not allow duplicated channels.
- Improved accuracy of subsample delay.
- Windows: Optional automatic sample format selection.

Bugfixes:
- Windows: Fix Wasapi exclusive mode for padded 24-bit samples.
- Windows & macOS: Fix audio data loss in playback when using
  a capture device with no inherent rate limit,
  such as File and SignalGenerator.

## v3.0.1
Bugfixes:
- Make sure that Alsa playback device resumes after pause.

## v3.0.0
New features:
- Optional multithreaded filter processing.
- Request higher proprity of audio threads for improved stability.
- Add a signal generator capture device.
- Optionally write wav header when outputting to file or stdout.
- Add `WavFile` capture device type for reading wav files.
- Optional limit for volume controls.
- Add websocket command for reading all faders with a single call.
- Linux: Subscribe to capture device control events for volume, sample rate and format changes.
- Linux: Optionally select Alsa sample format automatically.
- Improved controller for rate adjustment.
- Command line options for setting aux volume and mute.
- Optional user-defined volume limits for volume adjust commands.
- Add noise gate.
- Add optional channel labels for capture devices and mixers.
- Optional log file rotation.

Changes:
- Remove the optional use of FFTW instead of RustFFT.
- Rename `File` capture device to `RawFile`.
- Filter pipeline steps take a list of channels to filter instead of a single one.

Bugfixes:
- Windows: Fix compatibility issues for some WASAPI devices.
- MacOS: Support devices appearing as separate capture and playback devices.
- Linux: Improved Alsa error handling.

## v2.0.3
Bugfixes:
- MacOS: Fix using Aggregate devices for playback.

## v2.0.2
Bugfixes:
- MacOS: Fix a segfault when reading clock source names for some capture devices.
- Windows: Adjust the missed event threshold, avoids some rare stuttering.

## v2.0.1
Bugfixes:
- Ignore capture_samplerate when resampling is disabled.
- Increase Alsa device buffer sizes to avoid errors.

## v2.0.0
New features:
- Add dynamic range compressor.
- Add websocket commands to read peak and rms history.
- Add ToggleMute websocket command.
- Add AdjustVolume websocket command for relative volume changes.
- Better handling of USB gadget in Alsa backend.
- Add option to bypass pipeline steps.
- Bluetooth capture support on Linux via Bluez.
- Updated resampler with faster lower quality options.
- Higher precision of biquad filters.
- More flexible configuration of resampler type and quality.
- Allow setting optional config parameters to `null` to use default value.
- Add "Dummy" convolution filter type for easier CPU load testing.
- Add title and description fields to various parts of the config.
- Gain can be specified in dB or linear scale.
- Websocket command to reset clipped samples counter.
- Add an always enabled default volume control.
- Add several volume control channels (faders).
- Change Loudness filter to only perform loudness compensation.
- Add more ditherers.
- Add GeneralNotch biquad type.
- Add Tilt equalizer biquad combo.
- Add GraphicEqualizer biquad combo.
- Support rate adjust for BlachHole on macOS.
- Add statefile for persisting runtime parameters to file.
- Websocket command to get pipeline processing capacity utilization.
- Add commands to read statefile path and updating status.
- Improved handling of config changes via SIGHUP and websocket.

Changes:
- Optimize cpu load in general, and of dithering and delay filters in particular.
- More logical names for dither types.
- Updated Shibata dither coeffients.
- Rename `Set/GetConfigName` websocket commands to `Set/GetConfigFilePath`.
- Removed redundant`change_format` parameter to simplify CoreAudio device config.

## 1.0.3
Bugfixes:
 - Volume and Loudness filters keep mute state on config reload.

## 1.0.2
Bugfixes:
 - Optimize inefficient peak and rms calculations.
 - Switch to stable compiler for release builds, fixes memory leak in pre-built v1.0.1 binary on macOS.

## 1.0.1
Bugfixes:
- Alsa: Avoid opening capture and playback devices at the same time since this causes trouble with some devices.

## 1.0.0
New features:
- New improved CoreAudio backend.
- Switch to faster logging library.
- Improved support for Wasapi loopback capture.
- Add "Stalled" state.
- Some Mixer parameters made optional.
- Delay value can be given in millimetres.
- Improved Alsa backend.
- Handle subnormal numbers in IIR filters (Biquad and DiffEq).

## 0.6.3
Bugfixes:
- Fix slow start with Alsa plug devices (regression in 0.6.2).

## 0.6.2
New features:
- Updated wasapi library.
- Add FivePointPeq biquad combo.
- Support wav with extended header.

Bugfixes:
- Stop properly after failing to start with bad wasapi config.

## 0.6.1
New features:
- Add lists of supported device types in help message.

Bugfixes:
- Fix broken Wasapi shared mode.
- Correct "built with features" list in help.
- Correct list of supported device types.

## 0.6.0
New features:
- New Wasapi backend with support for exclusive mode and loopback.
- Do proper shutdown on SIGINT (ctrl-c).
- Add StopReason websocket command.
- Add GetPreviousConfig websocket command to get the previously active config.
- Add option to stop on detected sample rate change.
- Add support for rate adjust on the ALSA USB gadget capture device (introduced in kernel 5.14).

Bugfixes:
- Add missing token handling in .wav FIR coefficient filenames.

## 0.5.2
New features:
- Peaking, Notch, Bandpass and Allpass filters can be defined with bandwidth.
- Highshelf and Lowshelf can be defined with Q-value.

## 0.5.1
New features:
- Add JACK support.
- Add `GetSupportedDeviceTypes` websocket command.

Bugfixes:
- Handle wav files with extended fmt chunk.
- Don't allow starting with zero channels.

## 0.5.0
New features:
- Add RMS and Peak measurement for each channel at input and output.
- Add a `Volume` filter for volume control.
- Add exit codes.
- Adapt `check` output to be more suitable for scripts.
- Search for filter coefficient files with relative paths first in config file dir. 
- Add `ShibataLow` dither types.
- Add option to write logs to file.
- Skip processing of channels that are not used in the pipeline.
- Update to new faster RustFFT.
- Overriding samplerate also scales chunksize.
- Use updated faster resampler.
- Enable experimental neon support in resampler via `neon` feature.
- Add `Loudness` volume control filter.
- Add mute options in mixer and Gain filters.
- Add mute function to Volume and Loundness filters, with websocket commands.
- Add `debug` feature for extra logging.
- Improve validation of filters.
- Setting to enable retry on reads from Alsa capture devices (helps avoiding driver bugs/quirks for some devices).
- Optionally avoid blocking reads on Alsa capture devices (helps avoiding driver bugs/quirks for some devices).
- Read FIR coefficients from WAV.
- Add subsample delay.

Bugfixes:
- Don't block playback for CoreAudio/Wasapi if there is no data in time.
- Validate `silence_threshold` and `silence_timeout` fields.
- Fix panic when reloading config if a new filter was defined but not added to the pipeline.
- Check for mixer parameter changes when reloading config.
- Token substutution and overrides also work via websocket.
- Don't exit on SIGHUP when waiting for a config.
- Fix handling of negative values when reading filter coeffs in I24_3_LE format.
- Gain filters react to mute setting on reload.
- Fix noise in output when resampling and muting all channels in mixer.
- Fix handling of negative values for input and output in S24LE format.


## 0.4.2
Bugfixes:
- Fix random garbage output when using the Stdout playback device.

## 0.4.1
Bugfixes:
- Fix incorrect config checks for LinkwitzRiley and Butterworth biquads.

## 0.4.0
New features:
- New commands to get more playback information from the websocket server.
- Changed all websocket commands to use Json input and output.
- Added optional support for secure websocket connections (wss).
- Rename the optional websocket to feature to `websocket`.
- Add new optional feature `secure-websocket` for wss support.
- Added an option to generate arbitrary length filters for testing convolution cpu load.
- Possible to use Reload command to restart from inactive state.
- Handle quirks of the USB audio gadget when used as Alsa capture source.
- Add `loglevel` option.
- Use local time instead of UTC in log messages.
- Add command line options to override some parameters.
- Add substitution of `$samplerate$` and `$channels$` tokens in config.

Bugfixes:
- Better handling of input device errors, fixes 100% cpu usage after panic.
- Use `Instant` instead of `SystemTime`to avoid issues when system clock is changed.
- Fix 100% cpu when Stdin doesn't provide any data.
- Reduce cpu usage when using PulseAudio.
- Fix buffer size handling for alsa capture.
- Fix high frequency noise from synchronous resampler.


## 0.3.2
New features:
- New commands to get more information from the websocket server.
- Possible to skip lines or bytes in coefficient files.
- Updated Cpal library.
- Added capture and playback devices Stdin & Stdout.
- Improved error messages.
- Improved validation of mixer config.
- Added option to set which IP address to bind websocket server to.

Bugfixes:
- Fix websocket `exit` command.
- Correct response of `setconfigname` websocket command.
- Fix buffer underrun soon after starting Alsa playback.
- Correct scaling of FIR coefficients when reloading config.


## 0.3.1
New features:
- Rate adjust via the resampler also for Wasapi and CoreAudio. 


## 0.3.0
New features:
- Support for Windows (Wasapi) and macOS (CoreAudio) via the Cpal library.


## 0.2.2
New features:
- Fix building on Windows and macOS.
- Updated versions of several libraries.
- Improved speed from optimization of several important loops.


## 0.2.1
New features:
- Convolver was optimized to be up to a factor 2 faster.

## 0.2.0
New features:
- Synchronous resampler that replaces the previous FastSync, BalancedSync and AccurateSync types with a single one called Synchronous. This uses FFT for a major speedup.
- The Async resamplers have been optimized and are now around a factor 2 faster than before.

Bugfixes:
- Fixed error when setting Alsa buffer size in some cases.


## 0.1.0
New features:
- Support for asynchronous resampling in all backends.
- Added S24LE3 format (corresponds to Alsa S24_3LE)
- File capture device can skip a number of bytes at the beginning of a file and then read a limited number of bytes

Other:
- Alsa backend rewritten to reduce code duplication
- Improved debug output


## 0.0.14
Last version without resampling
