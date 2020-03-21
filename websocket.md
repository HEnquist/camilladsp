# Controlling via websocket

If the sebsocket server is enabled with the -p option, CamillaDSP will listen to incoming websocket connections on the specified port.
The available commands are:
- `getconfig` : read the current configuration as yaml
  * response is the config in yaml format.
- `getconfigname` : get name and path of current config file
  * response is `OK:/path/to/current.yml`
- `reload` : reload current config file (same as SIGHUP)
  * response is `OK:RELOAD` or `ERROR:RELOAD` 
- `exit` : exit (not yet implemented)
- `setconfigname:/path/to/file.yml` : change config file name, not applied until `reload` is called
  * response is `OK:/path/to/file.yml` or `ERROR:/path/to/file.yml`
- `setconfig:<new config in yaml format>` : provide a new config as a yaml string. Applied directly.
  * response is `OK:SETCONFIG` or `ERROR:SETCONFIG`

## Controlling from Python

You need the websocket_client module installed for this to work. The package is called `python-websocket-client` on Fedora and `python3-websocket` on Debian/Ubuntu.

First start CamillaDSP with the -p option:
```
camilladsp -v -p1234 /path/to/someconfig.yml
```

Start Ipython. Import the websocket client and make a connection:
```ipython
In [1]: from websocket import create_connection
In [2]: ws = create_connection("ws://127.0.0.1:3011")

```

### Get the name of the current config file
```ipython
In [3]: ws.send("getconfigname")
Out[3]: 19

In [4]: print(ws.recv())
/path/to/someconfig.yml
```

### Switch to a different config file
The new config is applied when the "reload" command is sent.
```ipython
In [5]: ws.send("setconfigname:/path/to/otherconfig.yml")
Out[5]: 52

In [6]: print(ws.recv())
OK:/path/to/otherconfig.yml

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
---
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