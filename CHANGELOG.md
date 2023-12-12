## Unreleased
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
- Fix handling of negative values when reading filter coeffs in S24LE3 format.
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
