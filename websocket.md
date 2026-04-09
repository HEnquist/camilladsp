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
Your system/language may automatically do the "stringify" / "parse" processes automaticaly for you, many won't. 

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
The available commands are listed below. All commands return the result,
and for the ones that return a value are this described here.

### General
- `GetVersion` : read the CamillaDSP version.
  * returns the version as a string, like `1.2.3`.
- `GetSupportedDeviceTypes` : read which playback and capture device types are supported. 
  * return a list containing two lists of strings (for playback and capture),
    like `[['File', 'Stdout', 'Alsa'], ['File', 'Stdin', 'Alsa']]`.
- `Stop` : stop processing and wait for a new config to be uploaded
  either with `SetConfig` or with `SetConfigFilePath`+`Reload`.
- `Exit` : stop processing and exit.

### Websocket server settings

Commands for reading and changing settings for the websocket server.
- `GetUpdateInterval` : get the update interval in ms for capture rate and signalrange.
  * returns the value as an integer
- `SetUpdateInterval` : set the update interval in ms for capturerate and signalrange.

### Read processing status

#### Commands for reading status parameters.
- `GetState` : get the current state of the processing as a string. Possible values are: 
  * "Running": the processing is running normally.
  * "Paused": processing is paused because the input signal is silent.
  * "Inactive": the program is inactive and waiting for a new configuration.
  * "Starting": the program is starting up processing with a new configuration.
  * "Stalled": processing is stalled because the capture device isn't providing any data.
- `GetStopReason` : get the last reason why CamillaDSP stopped the processing. Possible values are:
  * "None": processing hasn't stopped.
  * "Done": processing stopped when the capture device reached the end of the stream.
  * "CaptureError": the capture device encountered an error.
  * "PlaybackError": the playback device encountered an error.
  * "CaptureFormatChange": the sample rate or format of the capture device changed.
  * "PlaybackFormatChange": the sample rate or format of the playback device changed.

Subscribe to pushed state changes instead of polling.
- `SubscribeState`

When subscribed, CamillaDSP sends a `StateEvent` whenever processing state changes.
For non-stop states, the payload only contains `state`.
For the stop state (`"Inactive"`), the payload also contains `stop_reason`.

Example subscribe request:
```json
"SubscribeState"
```

Example pushed event while running:
```json
{
  "StateEvent": {
    "result": "Ok",
    "value": {
      "state": "Running"
    }
  }
}
```

Example pushed event when stopped:
```json
{
  "StateEvent": {
    "result": "Ok",
    "value": {
      "state": "Inactive",
      "stop_reason": "Done"
    }
  }
}
```

While state streaming is active, only the stop command is accepted:
- `StopSubscription`

Example stop request:
```json
"StopSubscription"
```

Any other command sent during active state streaming gets an `Invalid` response.
Sending `StopSubscription` when no subscription is active also gets an `Invalid` response.

For a minimal end-to-end example client, see `testscripts/state_subscriber.py`.

- `GetCaptureRate` : get the measured sample rate of the capture device.
  * return the value as an integer
- `GetSignalRange` : get the range of values in the last chunk.
  A value of 2.0 means full level (signal swings from -1.0 to +1.0)
  * returns the value as a float
- `GetRateAdjust` : get the adjustment factor applied to the asynchronous resampler.
  * returns the value as a float
- `GetBufferLevel` : get the current buffer level of the playback device when rate adjust is enabled, returns zero otherwise.
  * returns the value as an integer
- `GetClippedSamples` : get the number of clipped samples since the config was loaded.
  * returns the value as an integer
- `ResetClippedSamples` : reset the clipped samples counter to zero.
- `GetProcessingLoad` : get the current pipeline processing capacity utilization in percent.
- `GetResamplerLoad` : get the current resampler processing capacity utilization in percent.
- `GetStateFilePath` : get the current state file path, returns null if no state file is used.
- `GetStateFileUpdated` : check if all changes have been saved to the state file.

#### Commands for reading signal RMS and peak.
These commands all return a vector of floats, with one value per channel.
The values are the channel levels in dB, where 0 dB means full level.

Get the peak or RMS value in the last chunk on the capture or playback side.
- `GetCaptureSignalPeak`
- `GetCaptureSignalRms`
- `GetPlaybackSignalPeak`
- `GetPlaybackSignalRms`

Get the peak or RMS value measured during a specified time interval. Takes a time in seconds (n.nn),
and returns the values measured during the last n.nn seconds.
- `GetCaptureSignalPeakSince`
- `GetCaptureSignalRmsSince`
- `GetPlaybackSignalPeakSince`
- `GetPlaybackSignalRmsSince`

Get the peak or RMS value measured since the last call to the same command from the same client.
The first time a client calls this command it returns the values measured since the client connected.
If the command is repeated very quickly, it may happen that there is no new data.
The response is then an empty vector.
- `GetCaptureSignalPeakSinceLast`
- `GetCaptureSignalRmsSinceLast`
- `GetPlaybackSignalPeakSinceLast`
- `GetPlaybackSignalRmsSinceLast`

Combined commands for reading several levels with a single request.
These commands provide the same data as calling all the four commands in each of the groups above. 
The values are returned as a json object with keys `playback_peak`, `playback_rms`, `capture_peak` and `capture_rms`.
- `GetSignalLevels`
- `GetSignalLevelsSince`
- `GetSignalLevelsSinceLast`

Subscribe to pushed level updates instead of polling.
The command takes one argument, either `"playback"`, `"capture"`, or `"both"`:
- `SubscribeSignalLevels`

When subscribed, CamillaDSP sends a `SignalLevelsEvent` message each time a new chunk has been analyzed for the selected side.
The event payload has keys `side`, `rms`, and `peak`.

Example subscribe request:
```json
{"SubscribeSignalLevels": "playback"}
```

Example subscribe request for both sides:
```json
{"SubscribeSignalLevels": "both"}
```

Example pushed event:
```json
{
  "SignalLevelsEvent": {
    "result": "Ok",
    "value": {
      "side": "playback",
      "rms": [-18.2, -18.5],
      "peak": [-6.1, -6.0]
    }
  }
}
```

While streaming is active, only the stop command is accepted:
- `StopSubscription`

Example stop request:
```json
"StopSubscription"
```

Any other command sent during active streaming gets an `Invalid` response.
Sending `StopSubscription` when no subscription is active also gets an `Invalid` response.

For a minimal end-to-end example client, see `testscripts/signal_level_subscriber.py`.

There is also a separate command for subscribing to smoothed, rate-capped VU updates instead of handling that in a separate backend.

- `SubscribeVuLevels`

This command takes an object with three parameters:
- `max_rate` : maximum event rate in Hz. A value less than or equal to zero disables rate limiting.
- `attack` : attack time constant in milliseconds for rising values. Valid range is `0` to `60000`. `0` disables attack smoothing.
- `release` : release time constant in milliseconds for falling values. Valid range is `0` to `60000`. `0` disables release smoothing.

If `attack` or `release` is outside the valid range, CamillaDSP replies with `SubscribeVuLevels` and `InvalidValueError` instead of starting the subscription.

This stream always includes both playback and capture levels in each pushed event.

When subscribed, CamillaDSP sends a `VuLevelsEvent` message containing the latest smoothed `playback_rms`, `playback_peak`, `capture_rms`, and `capture_peak` vectors.

Example subscribe request:
```json
{
  "SubscribeVuLevels": {
    "max_rate": 30.0,
    "attack": 10.0,
    "release": 200.0
  }
}
```

Example pushed event:
```json
{
  "VuLevelsEvent": {
    "result": "Ok",
    "value": {
      "playback_rms": [-18.2, -18.5],
      "playback_peak": [-6.1, -6.0],
      "capture_rms": [-42.3, -41.9],
      "capture_peak": [-30.5, -29.8]
    }
  }
}
```

While VU streaming is active, the same subscription rule applies:
- `StopSubscription`

Get the peak since start.
- `GetSignalPeaksSinceStart` : Get the playback and capture peak level since processing started.
  The values are returned as a json object with keys `playback` and `capture`.
- `ResetSignalPeaksSinceStart` : Reset the peak values. Note that this resets the peak for all clients.

The configuration may include labels for the channels.
These are intended for display, for example in VU meters, and are not used by CamillaDSP itself.
- `GetChannelLabels` : Get the playback and capture channel labels.
  The labels are returned as a json object with keys `playback` and `capture`.


### Volume control

Commands for setting and getting the volume and mute of the default volume control on control `Main`.

- `GetVolume` : Get the current volume setting in dB.
  * Returns the value as a float.

- `SetVolume` : Set the volume control to the given value in dB. Clamped to the range -150 to +50 dB.

- `AdjustVolume` : Change the volume setting by the given number of dB, positive or negative.
  The resulting volume is clamped to the range -150 to +50 dB.
  The allowed range can be reduced by providing two more values, for minimum and maximum.

  Example, reduce the volume by 3 dB, with limits of -50 and +10 dB:
  ```{"AdjustVolume": [-3.0, -50.0, 10.0]}```

  * Returns the new value as a float.

- `GetMute` : Get the current mute setting.
  * Returns the muting status as a boolean.

- `SetMute` : Set muting to the given value.

- `ToggleMute` : Toggle muting.
  * Returns the new muting status as a boolean.


Commands for setting and getting the volume and mute setting of a given fader.
The faders are selected using an integer, 0 for `Main` and 1 to 4 for `Aux1` to `Aux4`.
All commands take the fader number as the first parameter.

- `GetFaderVolume` : Get the current volume setting in dB.
  * Returns a struct with the fader as an integer and the volume value as a float.

- `SetFaderVolume` : Set the volume control to the given value in dB. Clamped to the range -150 to +50 dB.

- `SetFaderExternalVolume` : Special command for setting the volume when a Loudness filter
  is being combined with an external volume control (without a Volume filter).
  Clamped to the range -150 to +50 dB.

- `AdjustFaderVolume` : Change the volume setting by the given number of dB, positive or negative.
  The resulting volume is clamped to the range -150 to +50 dB.
  The allowed range can be reduced by providing two more values, for minimum and maximum.

  Example, reduce the volume of fader 0 by 3 dB, with default limits:
  ```{"AdjustFaderVolume": [0, -3.0]}```

  Example, reduce the volume of fader 0 by 3 dB, with limits of -50 and +10 dB:
  ```{"AdjustFaderVolume": [0, [-3.0, -50.0, 10.0]]}```

  * Returns a struct with the fader as an integer and the new volume value as a float.

- `GetFaderMute` : Get the current mute setting.
  * Returns a struct with the fader as an integer and the muting status as a boolean.
- `SetFaderMute` : Set muting to the given value.
- `ToggleFaderMute` : Toggle muting.
  * Returns a struct with the fader as an integer and the new muting status as a boolean.

There is also a command for getting the volume and mute settings for all faders with a single query.
- `GetFaders` : Read all faders.
  * Returns a list of objects, each containing a `volume` and a `mute` property.


### Config management

Commands for reading and changing the active configuration.
- `GetConfig` : Read the current configuration as yaml.
  * Returns the config in yaml as a string.
- `GetConfigJson` : Read the current configuration as json.
  * Returns the config in json as a string.
- `GetConfigTitle` : Read the title from the current configuration.
  * Returns the title as a string.
- `GetConfigDescription` : Read the description from the current configuration.
  * Returns the description as a string.
- `GetConfigFilePath` : Get name and path of current config file.
  * Returns the path as a string.
- `GetPreviousConfig` : Read the previous configuration as yaml.
  * Returns the previously active config in yaml as a string.
- `SetConfigFilePath` : Change config file name given as a string, not applied until `Reload` is called.
- `SetConfig:` : Provide a new config as a yaml string. Applied directly.
- `SetConfigJson` : Provide a new config as a JSON string. Applied directly.
- `PatchConfig` : Apply a patch to the current config. A patch consists of a partial config that only
  contains the fields that should be changed. If the updated config is valid, it is applied directly.
- `GetConfigValue` : Read a value from the active config. The value to read is selected using a json pointer,
  see [RFC6901](https://datatracker.ietf.org/doc/html/rfc6901).
- `SetConfigValue` : Set a value in the active config, see `GetConfigValue`.
- `Reload` : Reload current config file (same as SIGHUP).


### Config reading and checking

These commands are used to check the syntax and contents of configurations. They do not affect the active configuration.
- `ReadConfig` : read the provided config (as a yaml string) and check it for yaml syntax errors.
  * If the config is ok, it returns the config with all optional fields filled with their default values.
    If there are problems, the status will be Error and the return value an error message.
- `ReadConfigJson` : same as ReadConfig but reads the provided config as a json string.
- `ReadConfigFile` : same as ReadConfig but reads the config from the file at the given path.
- `ValidateConfig`: same as ReadConfig but performs more extensive checks to ensure the configuration can be applied.
- `ValidateConfigJson`: same as ReadConfigJson but performs more extensive checks to ensure the configuration can be applied.

### Audio device listing

These commands query the audio backend for a list of devices.
They accept a backend name as input, and return a list of names.

- `GetAvailableCaptureDevices` : get a list of available capture devices. 
- `GetAvailablePlaybackDevices` : get a list of available playback devices. 

Each element in the returned list consists of one string for the device identifier,
and one optional string for the name.
Some backends use the name as identifier, they then return `null` as name.

The currently supported backend names are `Alsa`, `CoreAudio` and `Wasapi`.

Example entries for Wasapi:
```
[
  ["Microphone (USB Microphone)", null],
  ["In 3-4 (MOTU M Series)", null]
]
```

Example entries for Alsa:
```
[
  ["hw:Loopback,0,0", "Loopback, Loopback PCM, subdevice #0"],
  ["hw:Generic,0,0", "HD-Audio Generic, ALC236 Analog, subdevice #0"]
]
```

## Error responses
If a command succeeds, CamillaDSP will reply with the string `Ok` in the `result` field.
If not, this field will instead contain an error.

The possible errors are:
- `ShutdownInProgressError`: CamillaDSP is shutting down and unable to handle the request.
- `RateLimitExceededError`: Too many requests were sent in a short time.
- `InvalidFaderError`: The request tried to modify a fader that does not exist,
- `ConfigValidationError`: The config could be read but contains some error.
  The response includes a message describing the error.
- `ConfigReadError`: The config could not be read.
  The reason can be that the file does not exist, or that it contains some error.
  The response includes a message describing the error.
- `InvalidValueError`: The request tried to change a parameter to an invalid value.
  The response includes a message describing the error.
- `InvalidRequestError`: The request was invalid.
  The response includes a message describing the error.

### Simple error
Simple errors do not contain any message. They are returned as just a string.
Example:
```json
"result": "InvalidFaderError"
```

### Error with message
If the error has a message, it is instead returned as a a json object with one key and one value.
Example:
```json
"result": {"ConfigValidationError": "Description of the error"}
``` 

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
Note the "wss" instead of "ws" in the address. Since the certificate is self.signed,
we need to use ssl.CERT_NONE for the connection to be accepted.
