## 0.4.0
New features:
- New commands to get more playback information from the websocket server.
- Changed all websocket commands to use Json input and output.
- Added support for secure websocket connections (wss).
- Rename the optional websocket to feature to `websocket`.
- Add new optional feature `secure-websocket` for wss support.
- Added an option to generate arbitrary length filters for testing convolution cpu load.

Bugfixes:
- Better handling of input device errors, fixes 100% cpu usage after panic.
- Use `Instant` instead of `SystemTime`to avoid issues when system clock is changed.


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
