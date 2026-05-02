# Controlling via websocket

If the websocket server is enabled with the `-p` option,
CamillaDSP will listen to incoming websocket connections on the specified port.

If additionally the "wait" flag is given, it will wait for a config to be uploaded
via the websocket server before starting the processing.

By default the websocket server binds to the address 127.0.0.1,
which means it's only accessible locally (on the same machine).
If it should be also available to remote machines, give the IP address
of the interface where it should be available with the `-a` option.
Giving 0.0.0.0 will bind to all interfaces.


## Command syntax
For commands without arguments, this is just a string *with the command name within quotes*:
```
"GetVersion"
```
For commands that take an argument, they are instead given as a key and a value:
```json
{"SetUpdateInterval": 500}
```

The return values are also JSON (in string format).
The commands that don't return a value return a structure containing the command name and the result,
which is either Ok or Error:
```json
{
  "SetUpdateInterval": {
    "result": "Ok"
  }
}
```

The commands that return a value also include a "value" field:
```json
{
  "GetUpdateInterval": {
    "result": "Ok",
    "value": 500
  }
}
```

## String formatting, notably for NodeJS etc

All commands and responses are sent as the string text representation of a JSON object.
Your system/language may automatically do the "stringify" / "parse" processes automatically for you, many won't.

IE if you are using NodeJS/javascript then simply wrap your JSON object with JSON.stringify() before sending.
```
ws.send(JSON.stringify({"SetUpdateInterval": 1000}))
```

Likewise on receiving the value from the websocket server, the JSON **will be in string format**.
Complicating it further, if you're using the defacto 'ws' NodeJS library,
it's likely received as a buffer array that you need to convert to string first.
Either way, you'll need to parse the text with the JSON.parse function.
```
ws.on('message', function message(data) {
    // data is buffer array
    let replyString = Buffer.from(data).toString();
    let parsed = {};
    try {
      parsed = JSON.parse(replyString);
    }
    catch (err){
      console.error('Parse error', err);
    }

    if (parsed.hasOwnProperty('GetVolume')){
      console.log('GetVolume response received', parsed.GetVolume.value);
    }
});
```
*Wrapping the parse with a try/catch is good practice to avoid crashes with improperly formatted JSON etc.*


## All commands

The available commands are listed below.
All commands return a result, and for those that also return a value it is described here.

### General

Basic commands for querying the CamillaDSP version and the list of supported device types, and for stopping or exiting the process.

#### `GetVersion`

Get the CamillaDSP version string.

Returns: `string` — Version string, e.g. `"2.0.0"`.

#### `GetSupportedDeviceTypes`

Get the list of supported playback and capture device types.

Returns: `[string[], string[]]` — `[list_of_playback_types, list_of_capture_types]`.

#### `Stop`

Stop processing and wait for a new configuration to be uploaded
via `SetConfig` or `SetConfigFilePath` +
`Reload`.

#### `Exit`

Stop processing and exit CamillaDSP.

### Websocket server settings

Controls the polling interval used for background status sampling. The update interval determines how often the capture rate and signal range snapshots are refreshed internally.

#### `GetUpdateInterval`

Get the update interval for capture rate and signal range polling.

Returns: `integer (≥ 0)` — Update interval in milliseconds.

#### `SetUpdateInterval`

Set the update interval for capture rate and signal range polling.

Argument: `integer (≥ 0)` — interval in milliseconds as an integer.

### Processing status

Commands for monitoring the running pipeline: processing state, capture rate, signal range, async-resampler adjustment, playback buffer level, clipped samples, and CPU load. Also covers the optional state file and a push subscription for state-change events.

#### `GetState`

Get the current processing state.

Returns: `ProcessingState` — Current processing state.

**`ProcessingState` values:**

- `Running` — Processing is running normally.
- `Paused` — Processing is paused because the input signal is silent.
- `Inactive` — Processing is off and devices are closed, waiting for a new configuration.
- `Starting` — Opening devices and starting up processing with a new configuration.
- `Stalled` — Capture device is not providing data; processing is stalled.

#### `GetStopReason`

Get the reason processing last stopped.

Returns: `StopReason` — Reason the processing last stopped.

**`StopReason` values:**

- `None` — Processing is still running; not yet stopped.
- `Done` — Processing completed normally (e.g. end of file input).
- `CaptureError` — Capture device reported an error.
- `PlaybackError` — Playback device reported an error.
- `UnknownError` — An unexpected internal error occurred.
- `CaptureFormatChange` — Capture device sample rate changed to the given value.
- `PlaybackFormatChange` — Playback device sample rate changed to the given value.

#### `SubscribeState`

Subscribe to pushed processing state change events.

While subscribed, CamillaDSP sends a `WsReply::StateEvent` message whenever the
processing state changes. The event payload always contains `state`. When the state is
`"Inactive"` it also contains `stop_reason`.

Send `StopSubscription` to end the stream.

#### `StopSubscription`

Stop an active subscription (signal levels, VU levels, state, or spectrum).

Returns `WsResult::InvalidRequestError` if no subscription is active.

#### `GetCaptureRate`

Get the measured sample rate of the capture device.

Returns: `integer (≥ 0)` — Measured capture sample rate in Hz.

#### `GetSignalRange`

Get the peak-to-peak signal range of the last processed chunk.

A value of 2.0 means full level (signal swings from −1.0 to +1.0).

Returns: `number` — Peak-to-peak amplitude range of the last chunk (2.0 = full level).

#### `GetRateAdjust`

Get the current adjustment factor applied to the asynchronous resampler.

Returns: `number` — Rate adjustment factor applied to the async resampler (1.0 = no adjustment).

#### `GetBufferLevel`

Get the current playback device buffer level when rate adjust is enabled.

Returns: `integer (≥ 0)` — Playback device buffer fill level in frames; 0 if rate adjust is not enabled.

#### `GetClippedSamples`

Get the number of samples that have been clipped since the config was loaded.

Returns: `integer (≥ 0)` — Number of clipped samples since the config was loaded.

#### `ResetClippedSamples`

Reset the clipped-samples counter to zero.

#### `GetProcessingLoad`

Get the current pipeline processing load.

Returns: `number` — Pipeline processing load in percent.

#### `GetResamplerLoad`

Get the current resampler processing load.

Returns: `number` — Resampler processing load in percent.

#### `GetStateFilePath`

Get the path of the state file, if one is configured.

Returns: `string | null` — File path of the state file, or `null` if no state file is used.

#### `GetStateFileUpdated`

Check whether all pending changes have been saved to the state file.

Returns: `boolean` — `true` if all changes have been saved to the state file.

### Signal levels

Read RMS and peak levels for capture and playback channels. Levels are available as an instantaneous snapshot, averaged over a time window, accumulated since the last call, or pushed continuously via a subscription. Also includes peak-since-start tracking, per-channel display labels, and a smoothed VU-meter subscription.

#### `GetCaptureSignalRms`

Get the RMS level of the last chunk on the capture side, per channel.

Returns: `number[]` — RMS level per capture channel in dB (0 dB = full level).

#### `GetCaptureSignalRmsSince`

Get the RMS level averaged over the last `n` seconds on the capture side, per channel.

Argument: `number` — time window in seconds as a float.

Returns: `number[]` — RMS level per capture channel in dB, averaged over the requested window.

#### `GetCaptureSignalRmsSinceLast`

Get the RMS level since the last call to this command from this client, per channel.

On the first call, returns values since the client connected.
If called again before new data is available, returns an empty list.

Returns: `number[]` — RMS level per capture channel in dB since the last call; empty if no new data.

#### `GetCaptureSignalPeak`

Get the peak level of the last chunk on the capture side, per channel.

Returns: `number[]` — Peak level per capture channel in dB (0 dB = full level).

#### `GetCaptureSignalPeakSince`

Get the peak level over the last `n` seconds on the capture side, per channel.

Argument: `number` — time window in seconds as a float.

Returns: `number[]` — Peak level per capture channel in dB over the requested window.

#### `GetCaptureSignalPeakSinceLast`

Get the peak level since the last call to this command from this client, per channel.

Returns: `number[]` — Peak level per capture channel in dB since the last call; empty if no new data.

#### `GetPlaybackSignalRms`

Get the RMS level of the last chunk on the playback side, per channel.

Returns: `number[]` — RMS level per playback channel in dB (0 dB = full level).

#### `GetPlaybackSignalRmsSince`

Get the RMS level averaged over the last `n` seconds on the playback side, per channel.

Argument: `number` — time window in seconds as a float.

Returns: `number[]` — RMS level per playback channel in dB, averaged over the requested window.

#### `GetPlaybackSignalRmsSinceLast`

Get the RMS level since the last call to this command from this client, per channel.

Returns: `number[]` — RMS level per playback channel in dB since the last call; empty if no new data.

#### `GetPlaybackSignalPeak`

Get the peak level of the last chunk on the playback side, per channel.

Returns: `number[]` — Peak level per playback channel in dB (0 dB = full level).

#### `GetPlaybackSignalPeakSince`

Get the peak level over the last `n` seconds on the playback side, per channel.

Argument: `number` — time window in seconds as a float.

Returns: `number[]` — Peak level per playback channel in dB over the requested window.

#### `GetPlaybackSignalPeakSinceLast`

Get the peak level since the last call to this command from this client, per channel.

Returns: `number[]` — Peak level per playback channel in dB since the last call; empty if no new data.

#### `GetSignalLevels`

Get RMS and peak levels for both sides in a single request.

Returns: `AllLevels` — RMS and peak levels for both sides.

**`AllLevels` fields:**

- `playback_rms`: `number[]` — RMS level per playback channel in dB.
- `playback_peak`: `number[]` — Peak level per playback channel in dB.
- `capture_rms`: `number[]` — RMS level per capture channel in dB.
- `capture_peak`: `number[]` — Peak level per capture channel in dB.

#### `GetSignalLevelsSince`

Get RMS and peak levels over the last `n` seconds for both sides.

Argument: `number` — time window in seconds as a float.

Returns: `AllLevels` — RMS and peak levels for both sides, averaged over the requested window.

**`AllLevels` fields:**

- `playback_rms`: `number[]` — RMS level per playback channel in dB.
- `playback_peak`: `number[]` — Peak level per playback channel in dB.
- `capture_rms`: `number[]` — RMS level per capture channel in dB.
- `capture_peak`: `number[]` — Peak level per capture channel in dB.

#### `GetSignalLevelsSinceLast`

Get RMS and peak levels since the last call to this command from this client, for both sides.

Returns: `AllLevels` — RMS and peak levels for both sides since the last call; empty if no new data.

**`AllLevels` fields:**

- `playback_rms`: `number[]` — RMS level per playback channel in dB.
- `playback_peak`: `number[]` — Peak level per playback channel in dB.
- `capture_rms`: `number[]` — RMS level per capture channel in dB.
- `capture_peak`: `number[]` — Peak level per capture channel in dB.

#### `SubscribeSignalLevels`

Subscribe to pushed signal level events.

Argument: `WsSignalLevelSide` — which side to receive events for — `"playback"`, `"capture"`, or `"both"`.

While subscribed, CamillaDSP sends a `WsReply::SignalLevelsEvent` message each time a
new chunk is analyzed. The event rate therefore depends on the configured chunk size and
sample rate. Send `StopSubscription` to end the stream.

**`WsSignalLevelSide` values:**

- `Playback` — Playback side only.
- `Capture` — Capture side only.
- `Both` — Both playback and capture sides.

#### `SubscribeVuLevels`

Subscribe to smoothed, rate-capped VU-meter level events.

If `attack` or `release` is out of range the command returns `WsResult::InvalidValueError`
and no subscription is started.

While subscribed, CamillaDSP sends `WsReply::VuLevelsEvent` messages containing
smoothed `playback_rms`, `playback_peak`, `capture_rms`, and `capture_peak` vectors.
Send `StopSubscription` to end the stream.

**`VuSubscription` fields:**

- `max_rate`: `number` — Maximum event rate in Hz. A value ≤ 0 disables rate limiting.
- `attack`: `number` — Attack time constant in ms for rising values. Valid range: 0–60000. `0` disables smoothing.
- `release`: `number` — Release time constant in ms for falling values. Valid range: 0–60000. `0` disables smoothing.

#### `GetSignalPeaksSinceStart`

Get the peak capture and playback levels measured since processing started.

Returns: `PbCapLevels` — Peak levels since processing started, for both sides.

**`PbCapLevels` fields:**

- `playback`: `number[]` — Peak level per playback channel in dB, measured since processing started.
- `capture`: `number[]` — Peak level per capture channel in dB, measured since processing started.

#### `ResetSignalPeaksSinceStart`

Reset the peak-since-start counters. Affects all connected clients.

#### `GetChannelLabels`

Get the optional display labels for capture and playback channels.

Returns: `ChannelLabels` — Display labels for capture and playback channels.

**`ChannelLabels` fields:**

- `playback`: `string | null[] | null` — Labels for playback channels. `null` if no labels are configured. Each entry is a label
string, or `null` if that specific channel has no label.
- `capture`: `string | null[] | null` — Labels for capture channels. Same structure as `playback`.

### Spectrum analysis

Compute an FFT-based frequency spectrum from the audio passing through the pipeline. Available as a one-shot request or a continuous push subscription.

#### `GetSpectrum`

Compute a one-shot frequency spectrum from the audio currently passing through the pipeline.

**`SpectrumRequest` fields:**

- `side`: `SpectrumSide` — Which side to analyze: `"capture"` or `"playback"`.
- `channel`: `integer (≥ 0) | null` — Channel to analyze. `null` averages all channels; an integer selects a single channel (zero-based).
- `min_freq`: `number` — Lower edge of the frequency range in Hz. Must be > 0.
- `max_freq`: `number` — Upper edge of the frequency range in Hz. Must be > `min_freq`.
- `n_bins`: `integer (≥ 0)` — Number of output bins. Must be ≥ 2.

Returns: `SpectrumData | null` — Computed spectrum with frequency and magnitude arrays.

**`SpectrumData` fields:**

- `frequencies`: `Arc<?>` — Center frequency of each output bin in Hz.
- `magnitudes`: `number[]` — Per-bin peak magnitude in dBFS (0 dBFS = full-scale sine wave).

#### `SubscribeSpectrum`

Subscribe to pushed spectrum events.

If processing is not running when this is sent, the result is
`WsResult::ProcessingNotRunningError` and no subscription is started.

While subscribed, CamillaDSP sends `WsReply::SpectrumEvent` each time a new spectrum is
ready. If processing stops, a final event with `WsResult::ProcessingStopped` is sent and
the subscription is cancelled. Resubscribe once processing has resumed.

Send `StopSubscription` to end the stream.

**`SpectrumSubscription` fields:**

- `side`: `SpectrumSide` — Which side to analyze: `"capture"` or `"playback"`.
- `channel`: `integer (≥ 0) | null` — Channel to analyze. `null` averages all channels; an integer selects a single channel (zero-based).
- `min_freq`: `number` — Lower edge of the frequency range in Hz. Must be > 0.
- `max_freq`: `number` — Upper edge of the frequency range in Hz. Must be > `min_freq`.
- `n_bins`: `integer (≥ 0)` — Number of output bins. Must be ≥ 2.
- `max_rate`: `number | null` — Maximum push rate in Hz. `None` = natural rate (one push per 50 % overlap hop).

### Volume control

Read and adjust the volume and mute state of the faders. The Main fader (index 0) maps to fader index 0; Aux1–Aux4 map to indices 1–4. Volume adjustments can optionally specify a clamping range so that the result stays within caller-defined limits.

#### `GetVolume`

Get the current volume of the Main fader.

Returns: `number` — Current volume in dB.

#### `SetVolume`

Set the volume of the Main fader. Clamped to −150 to +50 dB.

Argument: `number` — volume in dB as a float.

#### `AdjustVolume`

Adjust the volume of the Main fader by a delta in dB.

Argument: `ValueWithOptionalLimits` — either `delta` or `[delta, min, max]`.

**`ValueWithOptionalLimits` values:**

- `Plain` — Adjust by `delta` dB, clamped to the global −150 to +50 dB range.
- `Limited` — Adjust by `delta` dB, clamped to `[min, max]` instead of the global range.

Returns: `number` — New volume in dB after the adjustment.

#### `GetMute`

Get the mute state of the Main fader.

Returns: `boolean` — `true` if muted.

#### `SetMute`

Set the mute state of the Main fader.

Argument: `boolean` — `true` to mute, `false` to unmute.

#### `ToggleMute`

Toggle the mute state of the Main fader.

Returns: `boolean` — New mute state after the toggle.

#### `GetFaders`

Get the volume and mute state of all faders in a single request.

Returns: `Fader[]` — List of faders: Main (index 0) followed by Aux1–Aux4 (indices 1–4).

**`Fader` fields:**

- `volume`: `number` — Current volume in dB.
- `mute`: `boolean` — Whether the fader is muted.

#### `GetFaderVolume`

Get the volume of a specific fader.

Argument: `integer (≥ 0)` — fader index — 0 for Main, 1–4 for Aux1–Aux4.

Returns: `[integer (≥ 0), number]` — `[fader_index, volume_dB]`.

#### `SetFaderVolume`

Set the volume of a specific fader. Clamped to −150 to +50 dB.

Arguments: `[integer (≥ 0), number]` — `[fader_index, volume_dB]`.

#### `SetFaderExternalVolume`

Special volume setter for use with a Loudness filter and an external volume control
(without a Volume filter). Clamped to −150 to +50 dB.

Arguments: `[integer (≥ 0), number]` — `[fader_index, volume_dB]`.

#### `AdjustFaderVolume`

Adjust the volume of a specific fader by a delta in dB.

Arguments: `[integer (≥ 0), ValueWithOptionalLimits]` — `[fader_index, delta]` or `[fader_index, [delta, min, max]]`.

**`ValueWithOptionalLimits` values:**

- `Plain` — Adjust by `delta` dB, clamped to the global −150 to +50 dB range.
- `Limited` — Adjust by `delta` dB, clamped to `[min, max]` instead of the global range.

Returns: `[integer (≥ 0), number]` — `[fader_index, new_volume_dB]` after the adjustment.

#### `GetFaderMute`

Get the mute state of a specific fader.

Argument: `integer (≥ 0)` — fader index.

Returns: `[integer (≥ 0), boolean]` — `[fader_index, is_muted]`.

#### `SetFaderMute`

Set the mute state of a specific fader.

Arguments: `[integer (≥ 0), boolean]` — `[fader_index, mute_bool]`.

#### `ToggleFaderMute`

Toggle the mute state of a specific fader.

Argument: `integer (≥ 0)` — fader index.

Returns: `[integer (≥ 0), boolean]` — `[fader_index, new_mute_state]` after the toggle.

### Config management

Read and modify the active configuration. Changes applied via `SetConfig`, `SetConfigJson`, or `PatchConfig` take effect immediately. Changes via `SetConfigFilePath` require a subsequent `Reload` to be applied.

#### `GetConfig`

Read the active configuration.

Returns: `string` — Active config in YAML format.

#### `GetConfigJson`

Read the active configuration as JSON.

Returns: `string` — Active config in JSON format.

#### `GetConfigTitle`

Read the `title` field from the active configuration.

Returns: `string` — Title string from the active config.

#### `GetConfigDescription`

Read the `description` field from the active configuration.

Returns: `string` — Description string from the active config.

#### `GetConfigFilePath`

Get the path of the currently loaded config file.

Returns: `string | null` — File path of the active config, or `null` if no file is loaded.

#### `GetPreviousConfig`

Read the previously active configuration (before the last reload or upload).

Returns: `string` — Previously active config in YAML format.

#### `SetConfigFilePath`

Change the active config file path. Not applied until `Reload` is called.

Argument: `string` — file path as a string.

#### `SetConfig`

Upload and immediately apply a new configuration as a YAML string.

Argument: `string` — config in YAML format as a string.

#### `SetConfigJson`

Upload and immediately apply a new configuration as a JSON string.

Argument: `string` — config in JSON format as a string.

#### `PatchConfig`

Apply a partial patch to the active configuration.

The patch is a partial config object containing only the fields to change.
If the resulting config is valid it is applied immediately.

Argument: `any` — partial config as a JSON value.

#### `GetConfigValue`

Read a single value from the active configuration using a JSON Pointer (RFC 6901).

Argument: `string` — JSON Pointer string, e.g. `"/devices/samplerate"`.

Returns: `any` — Value at the specified JSON Pointer path.

#### `SetConfigValue`

Set a single value in the active configuration using a JSON Pointer (RFC 6901).

Arguments: `[string, any]` — `[pointer, value]` where `pointer` is a JSON Pointer string such as
`"/devices/samplerate"`.

#### `Reload`

Reload the current config file from disk. Equivalent to sending `SIGHUP`.

### Config reading and checking

Parse and validate a configuration string or file without affecting the running pipeline. Useful for checking a config before applying it. The `Validate` variants perform more thorough cross-field checks than the `Read` variants.

#### `ReadConfig`

Parse and fill defaults for a YAML config string without changing the active config.

Argument: `string` — config in YAML format as a string.

Returns: `string` — Config with all optional fields filled with defaults, or an error message.

#### `ReadConfigJson`

Parse and fill defaults for a JSON config string without changing the active config.

Argument: `string` — config in JSON format as a string.

Returns: `string` — Config with all optional fields filled with defaults, or an error message.

#### `ReadConfigFile`

Parse and fill defaults for a config file without changing the active config.

Argument: `string` — path to the config file as a string.

Returns: `string` — Config with all optional fields filled with defaults, or an error message.

#### `ValidateConfig`

Like `ReadConfig` but performs more extensive validation checks.

Argument: `string` — config in YAML format as a string.

Returns: `string` — Validated config with defaults, or an error message.

#### `ValidateConfigJson`

Like `ReadConfigJson` but performs more extensive validation checks.

Argument: `string` — config in JSON format as a string.

Returns: `string` — Validated config with defaults, or an error message.

### Audio device listing

Enumerate available audio devices for a given backend and query their supported sample rates, formats, and channel counts. See the section below for the response format.

#### `GetAvailableCaptureDevices`

List available capture devices for a given backend.

Argument: `string` — backend name — one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.

Returns: `[string, string][]` — List of `[identifier, name_or_null]` pairs.

#### `GetAvailablePlaybackDevices`

List available playback devices for a given backend.

Argument: `string` — backend name — one of `"Alsa"`, `"CoreAudio"`, `"Wasapi"`, `"Asio"`.

Returns: `[string, string][]` — List of `[identifier, name_or_null]` pairs.

#### `GetCaptureDeviceCapabilities`

Get the capabilities of a specific capture device.

Arguments: `[string, string]` — `[backend_name, device_name]`.

Errors: `WsResult::DeviceNotFoundError`, `WsResult::DeviceBusyError`, `WsResult::DeviceError`.

Returns: `AudioDeviceDescriptor` — Capabilities of the requested capture device.

**`AudioDeviceDescriptor` fields:**

- `name`: `string` — Backend-specific device identifier (e.g. `"hw:0,0"` for ALSA).
- `description`: `string` — Human-readable device name.
- `capability_sets`: `DeviceCapabilitySet[]` — Capability sets, one per access mode supported by the backend.

#### `GetPlaybackDeviceCapabilities`

Get the capabilities of a specific playback device.

Arguments: `[string, string]` — `[backend_name, device_name]`.

Errors: `WsResult::DeviceNotFoundError`, `WsResult::DeviceBusyError`, `WsResult::DeviceError`.

Returns: `AudioDeviceDescriptor` — Capabilities of the requested playback device.

**`AudioDeviceDescriptor` fields:**

- `name`: `string` — Backend-specific device identifier (e.g. `"hw:0,0"` for ALSA).
- `description`: `string` — Human-readable device name.
- `capability_sets`: `DeviceCapabilitySet[]` — Capability sets, one per access mode supported by the backend.


## Audio device capability response format

The capability commands return an `AudioDeviceDescriptor` object:
```json
{
  "name": "Device name",
  "description": "Readable description",
  "capability_sets": [
    {
      "mode": "Unified",
      "capabilities": [
        {
          "channels": 2,
          "samplerates": [
            {
              "samplerate": 44100,
              "formats": ["S16_LE", "S32_LE"]
            }
          ]
        }
      ]
    }
  ]
}
```

Each entry in `capability_sets` has a `mode` field:
- `Unified` — used by ALSA, CoreAudio, and ASIO, which have a single capability model.
- `Shared` — WASAPI shared mode. Always exactly one channel count and one sample rate; format is always `F32`.
- `Exclusive` — WASAPI exclusive mode. Probed independently; supports multiple channel counts, sample rates, and formats.
  WASAPI does not provide a structured capability API, so the exclusive-mode scan must probe configurations one at a time.
  Heuristics keep probe time reasonable, so not every valid configuration is guaranteed to appear for unusual devices.

For WASAPI, the response contains two sets — one `Shared` and one `Exclusive`:
```json
{
  "name": "Speakers (Realtek HD Audio)",
  "description": "Speakers (Realtek HD Audio)",
  "capability_sets": [
    {
      "mode": "Shared",
      "capabilities": [{"channels": 2, "samplerates": [{"samplerate": 48000, "formats": ["F32"]}]}]
    },
    {
      "mode": "Exclusive",
      "capabilities": [
        {
          "channels": 2,
          "samplerates": [
            {"samplerate": 44100, "formats": ["S16", "S24", "S32", "F32"]},
            {"samplerate": 48000, "formats": ["S16", "S24", "S32", "F32"]}
          ]
        }
      ]
    }
  ]
}
```

For ALSA, capability results are representative rather than exhaustive: continuous sample-rate ranges are reduced
to the standard rates that CamillaDSP probes, and channel probing is capped to a practical maximum.

Example device list entries for ALSA:
```
[
  ["hw:Loopback,0,0", "Loopback, Loopback PCM, subdevice #0"],
  ["hw:Generic,0,0", "HD-Audio Generic, ALC236 Analog, subdevice #0"]
]
```

Example device list entries for WASAPI (identifier is the display name; name field is `null`):
```
[
  ["Microphone (USB Microphone)", null],
  ["In 3-4 (MOTU M Series)", null]
]
```

If the device is not found, busy, or the probe fails, the result field contains an error
and `capability_sets` is an empty list.

## Error responses

If a command succeeds, CamillaDSP will reply with `Ok` in the `result` field.
If not, this field will instead contain an error string.

Errors without a message are returned as a plain string:
```json
"result": "InvalidFaderError"
```

Errors with a message are returned as a JSON object with one key:
```json
"result": {"ConfigValidationError": "Description of the error"}
```

### `ShutdownInProgressError`

CamillaDSP is shutting down and cannot handle the request.

### `RateLimitExceededError`

Too many requests were sent in a short time.

### `InvalidFaderError`

The request referred to a fader index that does not exist.

### `ConfigValidationError`

The configuration could be parsed but contains a logical error.

Includes a message describing the problem.

### `ConfigReadError`

The configuration could not be read (file missing, YAML/JSON syntax error, etc.).

Includes a message describing the problem.

### `InvalidValueError`

A parameter value was outside the accepted range.

Includes a message describing the problem.

### `InvalidRequestError`

The request itself was malformed or not valid in the current state.

Includes a message describing the problem.

### `DeviceNotFoundError`

The named audio device does not exist.

Includes the device name.

### `DeviceBusyError`

The audio device is currently in use and cannot be probed.

Includes the device name.

### `DeviceError`

The device probe failed for another reason.

Includes a description.

### `ProcessingStopped`

Processing stopped while a subscription was active.

Sent as the final event of a spectrum subscription when processing stops.

### `ProcessingNotRunningError`

Processing is not currently running.

Returned by `WsCommand::SubscribeSpectrum` when processing is inactive.


## Controlling from Python using pyCamillaDSP

The recommended way of controlling CamillaDSP with Python is by using the
[pyCamillaDSP library](https://github.com/HEnquist/pycamilladsp).

Please see the readme in that library for instructions.


## Controlling directly using Python

You need the websocket_client module installed for this to work.
The package is called `python-websocket-client` on Fedora and `python3-websocket` on Debian/Ubuntu.

First start CamillaDSP with the -p option:
```
camilladsp -v -p1234 /path/to/someconfig.yml
```

Start Ipython. Import the websocket client and make a connection:
```ipython
In [1]: from websocket import create_connection
In [2]: import json
In [3]: ws = create_connection("ws://127.0.0.1:1234")
```

### Get the name of the current config file
```ipython
In [4]: ws.send(json.dumps("GetConfigFilePath"))
Out[4]: 19

In [5]: print(ws.recv())
{"GetConfigFilePath":{"result":"Ok","value":"/path/to/someconfig.yml"}}
```

### Switch to a different config file
The new config is applied when the "reload" command is sent.
```ipython
In [6]: ws.send(json.dumps({"SetConfigFilePath": "/path/to/otherconfig.yml"}))
Out[6]: 52

In [7]: print(ws.recv())
{"GetConfigFilePath":{"result":"Ok","value":"/path/to/someconfig.yml"}}

In [8]: ws.send(json.dumps("Reload"))
Out[8]: 12

In [9]: print(ws.recv())
{"Reload":{"result":"Ok"}}
```


### Get the current configuration
Use json.loads to parse the json response.
```
In [10]: ws.send(json.dumps("GetConfig"))
Out[10]: 15

In [11]: reply = json.loads(ws.recv())
In [12]: print(reply["GetConfig"]["value"])
OK:GETCONFIG:---
devices:
  samplerate: 44100
  buffersize: 1024
  silence_threshold: 0.0
  silence_timeout: 0.0
  capture:
    type: Alsa
    ...
```

### Send a new config as yaml
The new config is applied directly.
```ipython
In [12]: with open('/path/to/newconfig.yml') as f:
    ...:     cfg=f.read()
    ...:

In [13]: ws.send(json.dumps({"SetConfig": cfg}))
Out[13]: 957

In [14]: print(ws.recv())
{"SetConfig":{"result":"Ok"}}
```

## Secure websocket, wss://
By compiling with the optional feature `secure-websocket`,
the websocket server also supports loading an identity from a .pfx file.
This is enabled by providing the two optional parameters "cert" and "pass",
where "cert" is the path to the .pfx-file containing the identity, and "pass" is the password for the file.
How to properly generate the identity is outside the scope of this readme,
but for simple tests a self-signed certificate can be used.

### Generate self-signed identity
First generate rsa keys:
```sh
openssl req -newkey rsa:2048 -new -nodes -x509 -days 3650 -keyout key.pem -out cert.pem
```

Then use these to generate the identity:
```sh
openssl pkcs12 -export -out identity.pfx -inkey key.pem -in cert.pem
```
This will prompt for an Export password. This is the password that must then be provided to CamillaDSP.


To connect with a Python client, do this:
```python
import websocket
import ssl

ws = websocket.WebSocket(sslopt={"cert_reqs": ssl.CERT_NONE})
ws.connect("wss://localhost:1234")
```
Note the "wss" instead of "ws" in the address. Since the certificate is self-signed,
it must be used with `ssl.CERT_NONE` to skip certificate verification.
