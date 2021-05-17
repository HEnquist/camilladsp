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
