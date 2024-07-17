# Controlling via websocket

If the websocket server is enabled with the `-p` option, CamillaDSP will listen to incoming websocket connections on the specified port.

If additionally the "wait" flag is given, it will wait for a config to be uploaded via the websocket server before starting the processing.

By default the websocket server binds to the address 127.0.0.1, which means it's only accessible locally (on the same machine). If it should be also available to remote machines, give the IP address of the interface where it should be available with the `-a` option. Giving 0.0.0.0 will bind to all interfaces.


## Command syntax
For commands without arguments, this is just a string *with the command name within quotes*:
```
"GetVersion"
```
For commands that take an argument, they are instead given as a key and a value:
```json
{"SetUpdateInterval": 500}
```

The return values are also JSON (in string format). The commands that don't return a value return a structure containing the command name and the result, which is either Ok or Error:
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

All commands and responses are sent as the string text representation of a JSON object. Your system/language may automatically do the "stringify" / "parse" processes automaticaly for you, many won't. 

IE if you are using NodeJS/javascript then simply wrap your JSON object with JSON.stringify() before sending. 
``` 
ws.send(JSON.stringify({"SetUpdateInterval": 1000}))
```

Likewise on receiving the value from the websocket server, the JSON **will be in string format**. Complicating it further, if you're using the defacto 'ws' NodeJS library, it's likely received as a buffer array that you need to convert to string first. Either way, you'll need to parse the text with the JSON.parse function.
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
The available commands are listed below. All commands return the result, and for the ones that return a value are this described here.

### General
- `GetVersion` : read the CamillaDSP version.
  * returns the version as a string, like `1.2.3`.
- `GetSupportedDeviceTypes` : read which playback and capture device types are supported. 
  * return a list containing two lists of strings (for playback and capture), like `[['File', 'Stdout', 'Alsa'], ['File', 'Stdin', 'Alsa']]`.
- `Stop` : stop processing and wait for a new config to be uploaded either with `SetConfig` or with `SetConfigFilePath`+`Reload`.
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
- `GetCaptureRate` : get the measured sample rate of the capture device.
  * return the value as an integer
- `GetSignalRange` : get the range of values in the last chunk. A value of 2.0 means full level (signal swings from -1.0 to +1.0)
  * returns the value as a float
- `GetRateAdjust` : get the adjustment factor applied to the asynchronous resampler.
  * returns the value as a float
- `GetBufferLevel` : get the current buffer level of the playback device when rate adjust is enabled, returns zero otherwise.
  * returns the value as an integer
- `GetClippedSamples` : get the number of clipped samples since the config was loaded.
  * returns the value as an integer
- `ResetClippedSamples` : reset the clipped samples counter to zero.
- `GetProcessingLoad` : get the current pipeline processing capacity utilization in percent.
- `GetStateFilePath` : get the current state file path, returns null if no state file is used.
- `GetStateFileUpdated` : check if all changes have been saved to the state file.

#### Commands for reading signal RMS and peak. 
These commands all return a vector of floats, with one value per channel. The values are the channel levels in dB, where 0 dB means full level.

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

Get the peak or RMS value measured since the last call to the same command from the same client. The first time a client calls this command it returns the values measured since the client connected.
If the command is repeated very quickly, it may happen that there is no new data. The response is then an empty vector.
- `GetCaptureSignalPeakSinceLast`
- `GetCaptureSignalRmsSinceLast`
- `GetPlaybackSignalPeakSinceLast`
- `GetPlaybackSignalRmsSinceLast`

Combined commands for reading several levels with a single request. These commands provide the same data as calling all the four commands in each of the groups above. The values are returned as a json object with keys `playback_peak`, `playback_rms`, `capture_peak` and `capture_rms`.
- `GetSignalLevels`
- `GetSignalLevelsSince`
- `GetSignalLevelsSinceLast`

Get the peak since start.
- `GetSignalPeaksSinceStart` : Get the playback and capture peak level since processing started.
  The values are returned as a json object with keys `playback` and `capture`.
- `ResetSignalPeaksSinceStart` : Reset the peak values. Note that this resets the peak for all clients.


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

- `SetFaderExternalVolume` : Special command for setting the volume when a Loudness filter is being combined with an external volume control (without a Volume filter). Clamped to the range -150 to +50 dB.

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
- `Reload` : Reload current config file (same as SIGHUP).


### Config reading and checking

These commands are used to check the syntax and contents of configurations. They do not affect the active configuration.
- `ReadConfig` : read the provided config (as a yaml string) and check it for yaml syntax errors.
  * If the config is ok, it returns the config with all optional fields filled with their default values. If there are problems, the status will be Error and the return value an error message.
- `ReadConfigFile` : same as ReadConfig but reads the config from the file at the given path.
- `ValidateConfig`: same as ReadConfig but performs more extensive checks to ensure the configuration can be applied.

### Audio device listing

These commands query the audio backend for a list of devices.
They accept a backend name as input, and return a list of names.

- `GetAvailableCaptureDevices` : get a list of available capture devices. 
- `GetAvailablePlaybackDevices` : get a list of available playback devices. 

Each element in the returned list consists of one string for the device identifier, and one optional string for the name.
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

## Controlling from Python using pyCamillaDSP

The recommended way of controlling CamillaDSP with Python is by using the [pyCamillaDSP library](https://github.com/HEnquist/pycamilladsp).

Please see the readme in that library for instructions.


## Controlling directly using Python

You need the websocket_client module installed for this to work. The package is called `python-websocket-client` on Fedora and `python3-websocket` on Debian/Ubuntu.

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
By compiling with the optional feature `secure-websocket`, the websocket server also supports loading an identity from a .pfx file. 
This is enabled by providing the two optional parameters "cert" and "pass", where "cert" is the path to the .pfx-file containing the identity, and "pass" is the password for the file.
How to properly generate the identity is outside the scope of this readme, but for simple tests a self-signed certificate can be used.

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
Note the "wss" instead of "ws" in the address. Since the certificate is self.signed, we need to use ssl.CERT_NONE for the connection to be accepted.
