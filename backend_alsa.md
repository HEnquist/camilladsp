# ALSA (Linux)

## Introduction

ALSA is the low level audio API that is used in the Linux kernel. The ALSA project also maintains various user-space tools and utilities that are installed by default in most Linux distributions.

This readme only covers some basics of ALSA. For more details, see for example the [ALSA Documentation](#alsa-documentation) and [A close look at ALSA](#a-close-look-at-alsa)

### Hardware devices

In the ALSA scheme, a soundcard or dac corresponds to a "card". A card can have one or several inputs and/or outputs, denoted "devices". Finally each device can support one or several streams, called "subdevices". It depends on the driver implementation how the different physical ports of a card is exposed in terms of devices. For example a 4-channel unit may present a single 4-channel device, or two separate 2-channel devices.

### PCM devices

An alsa PCM device can be many different things, like a simple alias for a hardware device, or any of the many plugins supported by ALSA. PCM devices are normally defined in the ALSA configuration file see the [ALSA Plugin Documentation](#alsa-plugin-documentation) for a list of the available plugins.

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

A hardware device is accessed via the "hw" plugin. The device name is then prefixed by `hw:`. To use the ALC236 hardware device from above, put either `hw:Generic` (to use the name, recommended) or `hw:0` (to use the index) in the CamillaDSP config.

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
Ignore the error message at the end. The interesting fields are FORMAT, RATE and CHANNELS. In this example the sample formats this device can use are S16_LE and S32_LE (corresponding to S16LE and S32LE in CamillaDSP, see the [table of equivalent formats in the main README](./README.md#equivalent-formats) for the complete list). The sample rate can be either 44.1 or 48 kHz. And it supports only stereo playback (2 channels).


Capture parameters are determined in the same way with `arecord`:
```
> arecord -D hw:Generic /dev/null --dump-hw-params
```
This outputs the same table as for the aplay example above, but for a capture device. 

## Routing all audio through CamillaDSP

To route all audio through CamillaDSP using ALSA, the audio output from any application must be redirected. This can be acheived either by using an [ALSA Loopback device](#alsa-loopback), or the [ALSA CamillaDSP "I/O" plugin](#alsa-camilladsp-"io"-plugin).

### ALSA Loopback
An ALSA Loopback card can be used. This behaves like a sound card that presents two devices. The sound being send to the playback side on one device can then be captured from the capture side on the other device. 
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
The first application that opens either side of a Loopback decides the sample rate and format. If `aplay` is started first in this example, this means that `arecord` must use the same sample rate and format. 
To change format or rate, both sides of the loopback must first be closed.

When using the ALSA Loopback approach, see the separate repository [camilladsp-config](#camilladsp-config). 
This contains example configuration files for setting up the entire system, and to have it start automatically after boot.

### ALSA CamillaDSP "I/O" plugin

ALSA can be extended by plugins in user-space. One such plugin that is intended specifically for CamillaDSP is the [ALSA CamillaDSP "I/O" plugin](#alsa-camilladsp-plugin) by scripple.

The plugin starts CamillaDSP whenever an application opens the CamillaDSP plugin PCM device. This makes it possible to support automatic switching of the sample rate. See the plugin readme for how to install and configure it.

## Configuration of devices

This example configuration will be used to explain the various options specific to ALSA:
```
  capture:
    type: Alsa
    channels: 2
    device: "hw:0,1"
    format: S16LE
    retry_on_error: false (*)
    avoid_blocking_read: false (*)
  playback:
    type: Alsa
    channels: 2
    device: "hw:Generic_1"
    format: S32LE
```

### Device names
See [Find name of device](#find-name-of-device) for what to write in the `device` field.

### Sample rate and format
Please see [Find valid playback and capture parameters](#find-valid-playback-and-capture-parameters).

### Workarounds for device quirks
The ALSA capture device has two optional extra properties that are used to work around quirks of some devices. 
Both should normally be left out, or set to the default value of `false`.
- `retry_on_error`: Set this to `true` if capturing from the USB gadget driver on for example a Raspberry Pi. 
  This device stops providing data if playback is stopped or paused, and retrying capture after an error 
  allows capture to continue when more data becomes available.
- `avoid_blocking_read`: Some devices misbehave when using the blocking IO of Alsa, 
  typically when there is no incoming data. Examples are spdif inputs when there is no signal present, 
  or the USB gadget driver when the source isn't sending any data. 
  Set this to `true` if you get capture errors when stopping the signal. This then allows processing to continue once the signal returns. 

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