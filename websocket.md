# Controlling via websocket

If the websocket server is enabled with the `-p` option, CamillaDSP will listen to incoming websocket connections on the specified port.

If additionally the "wait" flag is given, it will wait for a config to be uploaded via the websocket server before starting the processing.

By default the websocket server binds to the address 127.0.0.1, which means it's only accessible locally (on the same machine). If it should be also available to remote machines, give the IP address of the interface where it should be available with the `-a` option. Giving 0.0.0.0 will bind to all interfaces.

The available commands are:

### General
- `getversion` : read the CamillaDSP version
  * response is `OK:GETVERSION:1.2.3` 
- `stop` : stop processing and wait for a new config to be uploaded with `setconfig`
  * response is `OK:STOP` 
- `exit` : stop processing and exit
  * response is `OK:EXIT` 

### Websocket server settings

Commands for reading and changing settings for the websocket server.
- `getupdateinterval` : get the update interval in ms for capture rate and signalrange.
  * response is `OK:GETUPDATEINTERVAL:123456`
- `setupdateinterval:<new_number>` : set the update interval in ms for capturerate and signalrange.
  * response is `OK:SETUPDATEINTERVAL`

### Read processing status

Commands for reading status parameters.
- `getstate` : get the current state of the processing. Possible values are: 
  * "RUNNING": the processing is running normally.
  * "PAUSED": processing is paused because the input signal is silent.
  * "INACTIVE": the program is inactive and waiting for a new configuration.
  * response is `OK:GETSTATE:RUNNING`
- `getcapturerate` : get the measured sample rate of the capture device.
  * response is `OK:GETCAPTURERATE:123456`
- `getsignalrange` : get the range of values in the last chunk. A value of 2.0 means full level (signal swings from -1.0 to +1.0)
  * response is `OK:GETSIGNALRANGE:1.23456`
- `getrateadjust` : get the adjustment factor applied to the asynchronous resampler.
  * response is `OK:GETRATEADJUST:1.0023`

### Config management

Commands for reading and changing the active configuration
- `getconfig` : read the current configuration as yaml
  * response is `OK:GETCONFIG:(yamldata)` where yamldata is the config in yaml format.
- `getconfigjson` : read the current configuration as json
  * response is `OK:GETCONFIG:(jsondata)` where yamldata is the config in JSON format.
- `getconfigname` : get name and path of current config file
  * response is `OK:GETCONFIGNAME:/path/to/current.yml`
- `setconfigname:/path/to/file.yml` : change config file name, not applied until `reload` is called
  * response is `OK:/path/to/file.yml` or `ERROR:/path/to/file.yml`
- `setconfig:<new config in yaml format>` : provide a new config as a yaml string. Applied directly.
  * response is `OK:SETCONFIG` or `ERROR:SETCONFIG`
- `setconfigjson:<new config in JSON format>` : provide a new config as a JSON string. Applied directly.
  * response is `OK:SETCONFIGJSON` or `ERROR:SETCONFIGJSON`
- `reload` : reload current config file (same as SIGHUP)
  * response is `OK:RELOAD` or `ERROR:RELOAD` 

### Config reading and checking

These commands are used to check the syntax and contents of configurations. They do not affect the active configuration.
- `readconfig:<config in yaml format>` : read the provided config, check for syntax errors, and return the config with all optional fields filled with their default values.
  * response is `OK:READCONFIG:<config in yaml format>` or `ERROR:READCONFIG:<error message>`
- `readconfigfile:/path/to/file.yml` : read a config from a file, check for syntax errors, and return the config with all optional fields filled with their default values.
  * response is `OK:READCONFIGFILE:<config in yaml format>` or `ERROR:READCONFIGFILE:<error message>`
- `validateconfig:<config in yaml format>` : read the provided config, check for syntax errors and validate the contents. Returns the config with all optional fields filled with their default values.
  * response is `OK:VALIDATECONFIG:<config in yaml format>` or `ERROR:VALIDATECONFIG:<error message>`


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
In [2]: ws = create_connection("ws://127.0.0.1:1234")
```

### Get the name of the current config file
```ipython
In [3]: ws.send("getconfigname")
Out[3]: 19

In [4]: print(ws.recv())
OK:GETCONFIGNAME:/path/to/someconfig.yml
```

### Switch to a different config file
The new config is applied when the "reload" command is sent.
```ipython
In [5]: ws.send("setconfigname:/path/to/otherconfig.yml")
Out[5]: 52

In [6]: print(ws.recv())
OK:SETCONFIGNAME:/path/to/otherconfig.yml

In [7]: ws.send("reload")
Out[7]: 12

In [8]: print(ws.recv())
OK:RELOAD
```


### Get the current configuration
```
In [9]: ws.send("getconfig")
Out[9]: 15

In [10]: print(ws.recv())
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
In [11]: with open('/path/to/newconfig.yml') as f:
    ...:     cfg=f.read()
    ...:

In [12]: ws.send("setconfig:{}".format(cfg))
Out[12]: 957

In [13]: print(ws.recv())
OK:SETCONFIG
```