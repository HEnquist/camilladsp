# ALSA (Linux)

## Introduction

ALSA is the low level audio API that is used in the Linux kernel.
The ALSA project also maintains various user-space tools and utilities
that are installed by default in most Linux distributions.

This readme only covers some basics of ALSA. For more details,
see for example the [ALSA Documentation](#alsa-documentation) and [A close look at ALSA](#a-close-look-at-alsa)

### Hardware devices

In the ALSA scheme, a soundcard or dac corresponds to a "card".
A card can have one or several inputs and/or outputs, denoted "devices".
Finally each device can support one or several streams, called "subdevices".
It depends on the driver implementation how the different physical ports of a card is exposed in terms of devices.
For example a 4-channel unit may present a single 4-channel device, or two separate 2-channel devices.

### PCM devices

An alsa PCM device can be many different things, like a simple alias for a hardware device,
or any of the many plugins supported by ALSA.
PCM devices are normally defined in the ALSA configuration file.
See the [ALSA Plugin Documentation](#alsa-plugin-documentation) for a list of the available plugins.

### Find name of device
To list all hardware playback devices use the `aplay` command with the `-l` option:
```
> aplay -l
**** List of PLAYBACK Hardware Devices ****
card 0: Generic [HD-Audio Generic], device 0: ALC236 Analog [ALC236 Analog]
  Subdevices: 1/1
  Subdevice #0: subdevice #0
```

To list all PCM devices use the `aplay` command with the `-L` option:
```
> aplay -L
hdmi:CARD=Generic,DEV=0
    HD-Audio Generic, HDMI 0
    HDMI Audio Output
```
Capture devices can be found in the same way with `arecord -l` and `arecord -L`.

A hardware device is accessed via the "hw" plugin. The device name is then prefixed by `hw:`.
To use the ALC236 hardware device from above,
put either `hw:Generic` (to use the name, recommended) or `hw:0` (to use the index) in the CamillaDSP config.

To instead use the "hdmi" PCM device, it's enough to give the name `hdmi`.


### Find valid playback and capture parameters
To find the parameters for the playback device "Generic" from the example above, again use `aplay`:
```
> aplay -v -D hw:Generic /dev/zero --dump-hw-params
Playing raw data '/dev/zero' : Unsigned 8 bit, Rate 8000 Hz, Mono
HW Params of device "hw:Generic":
--------------------
ACCESS:  MMAP_INTERLEAVED RW_INTERLEAVED
FORMAT:  S16_LE S32_LE
SUBFORMAT:  STD
SAMPLE_BITS: [16 32]
FRAME_BITS: [32 64]
CHANNELS: 2
RATE: [44100 48000]
PERIOD_TIME: (333 96870749)
PERIOD_SIZE: [16 4272000]
PERIOD_BYTES: [128 34176000]
PERIODS: [2 32]
BUFFER_TIME: (666 178000000]
BUFFER_SIZE: [32 8544000]
BUFFER_BYTES: [128 68352000]
TICK_TIME: ALL
--------------------
aplay: set_params:1343: Sample format non available
Available formats:
- S16_LE
- S32_LE
```
Ignore the error message at the end. The interesting fields are FORMAT, RATE and CHANNELS.
In this example the sample formats this device can use are S16_LE and S32_LE (corresponding to S16LE and S32LE in CamillaDSP,
see the [table of equivalent formats in the main README](./README.md#equivalent-formats) for the complete list).
The sample rate can be either 44.1 or 48 kHz. And it supports only stereo playback (2 channels).

### Combinations of parameter values
Note that all possible combinations of the shown parameters may not be supported by the device.
For example many USB DACS only support 24-bit samples up to 96 kHz,
so that only 16-bit samples are supported at 192 kHz.
For other devices, the number of channels depends on the sample rate.
This is common on studio interfaces that support [ADAT](#adat).

CamillaDSP sets first the number of channels.
Then it sets sample rate, and finally sample format.
Setting a value for a parameter may restrict the allowed values for the ones that have not yet been set.
For the USB DAC just mentioned, setting the sample rate to 192 kHz means that only the S16LE sample format is allowed.
If the CamillaDSP configuration is set to 192 kHz and S24LE3, then there will be an error when setting the format.


Capture parameters are determined in the same way with `arecord`:
```
> arecord -D hw:Generic /dev/null --dump-hw-params
```
This outputs the same table as for the aplay example above, but for a capture device. 

## Routing all audio through CamillaDSP

To route all audio through CamillaDSP using ALSA, the audio output from any application must be redirected.
This can be acheived either by using an [ALSA Loopback device](#alsa-loopback),
or the [ALSA CamillaDSP "I/O" plugin](#alsa-camilladsp-"io"-plugin).

### ALSA Loopback
An ALSA Loopback card can be used. This behaves like a sound card that presents two devices.
The sound being send to the playback side on one device can then be captured from the capture side on the other device. 
To load the kernel module type:
```
sudo modprobe snd-aloop
```
Find the name of the device:
```
aplay -l
```

Play a track on card "Loopback", device 1, subdevice 0:
```
aplay -D hw:Loopback,1,0 sometrack.wav
```
The audio can then be captured from card "Loopback", device 0, subdevice 0, by running `arecord` in a separate terminal:
```
arecord -D hw:Loopback,0,0 sometrack_copy.wav
```
The first application that opens either side of a Loopback decides the sample rate and format.
If `aplay` is started first in this example, this means that `arecord` must use the same sample rate and format. 
To change format or rate, both sides of the loopback must first be closed.

When using the ALSA Loopback approach, see the separate repository [camilladsp-config](#camilladsp-config). 
This contains example configuration files for setting up the entire system, and to have it start automatically after boot.

### ALSA CamillaDSP "I/O" plugin

ALSA can be extended by plugins in user-space.
One such plugin that is intended specifically for CamillaDSP
is the [ALSA CamillaDSP "I/O" plugin](#alsa-camilladsp-plugin) by scripple.

The plugin starts CamillaDSP whenever an application opens the CamillaDSP plugin PCM device.
This makes it possible to support automatic switching of the sample rate.
See the plugin readme for how to install and configure it.

## Configuration of devices

This example configuration will be used to explain the various options specific to ALSA:
```
  capture:
    type: Alsa
    channels: 2
    device: "hw:0,1"
    format: S16LE (*)
    stop_on_inactive: false (*)
    follow_volume_control: "PCM Playback Volume" (*)
  playback:
    type: Alsa
    channels: 2
    device: "hw:Generic_1"
    format: S32LE (*)
```

### Device names
See [Find name of device](#find-name-of-device) for what to write in the `device` field.

### Sample rate and format
The sample format is optional. If set to `null` or left out,
the highest quality available format is chosen automatically.

When the format is set automatically, 32-bit integer (`S32LE`) is considered the best,
followed by 24-bit (`S24LE3` and `S24LE`) and 16-bit integer (`S16LE`).
The 32-bit (`FLOAT32LE`) and 64-bit (`FLOAT64LE`) float formats are high quality,
but are supported by very few devices. Therefore these are checked last.

Please also see [Find valid playback and capture parameters](#find-valid-playback-and-capture-parameters).

### Linking volume control to device volume
It is possible to let CamillaDSP follow the a volume control of the capture device.
This is mostly useful when capturing from the USB Audio Gadget,
which provides a control named `PCM Capture Volume` that is controlled by the USB host.

This does not alter the signal, and can be used to forward the volume setting from a player to CamillaDSP.
To enable this, set the `follow_volume_control` setting to the name of the volume control.
Any change of the volume then gets applied to the CamillaDSP main volume control.

The available controls for a device can be listed with `amixer`.
List controls for card 1:
```sh
amixer -c 1 controls
```

List controls with values and more details:
```sh
amixer -c 1 contents
```

The chosen control should be one that does not affect the signal volume,
otherwise the volume gets applied twice.
It must also have a scale in decibel like in this example:
```
numid=15,iface=MIXER,name='Master Playback Volume'
  ; type=INTEGER,access=rw---R--,values=1,min=0,max=87,step=0
  : values=52
  | dBscale-min=-65.25dB,step=0.75dB,mute=0
```


### Subscribe to Alsa control events
The Alsa capture device subscribes to control events from the USB Gadget and Loopback devices.
For the loopback, it subscribes to events from the `PCM Slave Active` control,
and for the gadget it subscribes to events from `Capture Rate`.
Both of these can indicate when playback has stopped.
If CamillaDSP should stop when that happens, set `stop_on_inactive` to `true`.
For the loopback, this means that CamillaDSP releases the capture side,
making it possible for a player application to re-open at another sample rate.

For the gadget, the control can also indicate that the sample rate changed.
When this happens, the capture can no longer continue and CamillaDSP will stop.
The new sample rate can then be read by the `GetStopReason` websocket command.

## Links
### ALSA Documentation
https://www.alsa-project.org/wiki/Documentation
### A close look at ALSA
https://www.volkerschatz.com/noise/alsa.html
### ALSA Plugin Documentation
https://www.alsa-project.org/alsa-doc/alsa-lib/pcm_plugins.html
### camilladsp-config
https://github.com/HEnquist/camilladsp-config
### ALSA CamillaDSP plugin
https://github.com/scripple/alsa_cdsp/

## Notes
### ADAT
ADAT achieves higher sampling rates by multiplexing two or four 44.1/48kHz audio streams into a single one.
A device implementing 8 channels over ADAT at 48kHz will therefore provide 4 channels over ADAT at 96kHz and 2 channels over ADAT at 192kHz.