# Controlling via websocket

If the websocket server is enabled with the `-p` option, CamillaDSP will listen to incoming websocket connections on the specified port.

If additionally the "wait" flag is given, it will wait for a config to be uploaded via the websocket server before starting the processing.

By default the websocket server binds to the address 127.0.0.1, which means it's only accessible locally (on the same machine). If it should be also available to remote machines, give the IP address of the interface where it should be available with the `-a` option. Giving 0.0.0.0 will bind to all interfaces.


## Command syntax
All commands are sent as JSON. For commands without arguments, this is just a string with the command name within quotes:
```
"GetVersion"
```
For commands that take an argument, they are instead given as a key and a value:
```json
{"SetUpdateInterval": 500}
```

The return values are also JSON. The commands that don't return a value return a structure containing the command name and the result, which is either Ok or Error:
```json
{
  "SetUpdateInterval: {
    "result": "Ok"
  }
}
```

The commands that return a value also include a "value" field:
```json
{
  "GetUpdateInterval: {
    "result": "Ok",
    "value": 500
  }
}
```

## All commands
The available commands are listed below. All commands return the result, and for the ones that return a value are this described here.

### General
- `GetVersion` : read the CamillaDSP version.
  * returns the version as a string, like `1.2.3`.
- `GetSupportedDeviceTypes` : read which playback and capture device types are supported. 
  * return a list containing two lists of strings (for playback and capture), like `[['File', 'Stdout', 'Alsa'], ['File', 'Stdin', 'Alsa']]`.
- `Stop` : stop processing and wait for a new config to be uploaded either with `SetConfig` or with `SetConfigName`+`Reload`.
- `Exit` : stop processing and exit.

### Websocket server settings

Commands for reading and changing settings for the websocket server.
- `GetUpdateInterval` : get the update interval in ms for capture rate and signalrange.
  * returns the value as an integer
- `SetUpdateInterval` : set the update interval in ms for capturerate and signalrange.

### Read processing status

Commands for reading status parameters.
- `GetState` : get the current state of the processing as a string. Possible values are: 
  * "RUNNING": the processing is running normally.
  * "PAUSED": processing is paused because the input signal is silent.
  * "INACTIVE": the program is inactive and waiting for a new configuration.
- `GetCaptureRate` : get the measured sample rate of the capture device.
  * return the value as an integer
- `GetSignalRange` : get the range of values in the last chunk. A value of 2.0 means full level (signal swings from -1.0 to +1.0)
  * returns the value as a float
- `GetCaptureSignalPeak` : get the peak value in the last chunk for all channels on the capture side. The scale is in dB, and a value of 0.0 means full level.
  * returns the value as a vector of floats
- `GetCaptureSignalRms` : get the RMS value in the last chunk for all channels on the capture side. The scale is in dB, and a value of 0.0 means full level.
  * returns the value as a vector of floats
- `GetPlaybackSignalPeak` : get the peak value in the last chunk for all channels on the playback side. The scale is in dB, and a value of 0.0 means full level.
  * returns the value as a vector of floats
- `GetPlaybackSignalRms` : get the RMS value in the last chunk for all channels on the playback side. The scale is in dB, and a value of 0.0 means full level.
  * returns the value as a vector of floats
- `GetRateAdjust` : get the adjustment factor applied to the asynchronous resampler.
  * returns the value as a float
- `GetBufferLevel` : get the current buffer level of the playback device when rate adjust is enabled, returns zero otherwise.
  * returns the value as an integer
- `GetClippedSamples` : get the number of clipped samples since the config was loaded.
  * returns the value as an integer


### Volume control

Commands for setting and getting the volume setting. These are only relevant if the pipeline includes "Volume" or "Loudness" filters.
- `GetVolume` : get the current volume setting in dB.
  * returns the value as a float
- `SetVolume` : set the volume control to the given value in dB.
- `GetMute` : get the current mute setting.
  * returns the muting status as a boolean
- `SetMute` : set muting to the given value.

### Config management

Commands for reading and changing the active configuration
- `GetConfig` : read the current configuration as yaml
  * returns the config in yaml as a string
- `GetConfigjson` : read the current configuration as json
  * returns the config in json as a string
- `GetConfigName` : get name and path of current config file
  * returns the path as a string
- `SetConfigName` : change config file name given as a string, not applied until `Reload` is called
- `SetConfig:` : provide a new config as a yaml string. Applied directly.
- `SetConfigJson` : provide a new config as a JSON string. Applied directly.
- `Reload` : reload current config file (same as SIGHUP)


### Config reading and checking

These commands are used to check the syntax and contents of configurations. They do not affect the active configuration.
- `ReadConfig` : read the provided config (as a yaml string) and check it for yaml syntax errors.
  * If the config is ok, it returns the config with all optional fields filled with their default values. If there are problems, the status will be Error and the return value an error message.
- `ReadConfigFile` : same as ReadConfig but reads the config from the file at the given path.
- `ValidateConfig`: same as ReadConfig but performs more extensive checks to ensure the configuration can be applied.



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
In [4]: ws.send(json.dumps("GetConfigName"))
Out[4]: 19

In [5]: print(ws.recv())
{"GetConfigName":{"result":"Ok","value":"/path/to/someconfig.yml"}}
```

### Switch to a different config file
The new config is applied when the "reload" command is sent.
```ipython
In [6]: ws.send(json.dumps({"SetConfigName": "/path/to/otherconfig.yml"}))
Out[6]: 52

In [7]: print(ws.recv())
{"GetConfigName":{"result":"Ok","value":"/path/to/someconfig.yml"}}

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

In [13]: ws.send(json.dumps({"SetConfig": cfg))
Out[13]: 957

In [14]: print(ws.recv())
{"SetConfig":{"result":"Ok"}}
```

## Secure websocket, wss://
By compiling with the optional feature `secure-websocket`, the websocket server also supports loading an identity from a .pfx file. 
This is enabled by providing the two optional parameters "cert" and "pass", where "cert" is the path to the .pfx-file containing the identity, and "pass" is the password for the file.
How to properly generate the identity is outside the scope of this readme, but for simple tests a self-signed certificate can be used.

### Generate self-signed identity
First geneate rsa keys: 
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