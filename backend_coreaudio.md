# CoreAudio (macOS)

## Introduction
CoreAudio is the standard audio API of macOS. 
In the current version of CamillaDSP (v0.6.0 at the time of writing), CoreAudio is supported via the [CPAL cross-platform sound library](https://github.com/RustAudio/cpal).

## Capturing audio from other applications

To capture audio from applications a virtual sound card is needed. 
This has been verified to work well with [Soundflower](https://github.com/mattingalls/Soundflower) 
and [BlackHole](https://github.com/ExistentialAudio/BlackHole). 
SoundFlower only supports Intel macs, while BlackHole supports both Intel and Apple Silicon. 
BlackHole has a 2-channel and a 16-channel version. 
There is currently a bug in the 2-channel version that in some cases can lead to choppy sound. 
If this happens, try the 16-channel version instead. 

### Sending all audio to the virtual card
Set the virtual sound card as the default playback device in the Sound preferences. This will work for all applications that respect this setting, which in practice is nearly all. The exceptions are the ones that provide their own way of selecting playback device.

### Capturing the audio
When applications output their audio to the playback side of the virtual soundcard, then this audio can be captured from the capture side.
This is done by giving the virtual soundcard as the capture device in the CamillaDSP configuration.


## Configuration of devices

This example configuration will be used to explain the various options specific to CoreAudio:
```
  capture:
    type: CoreAudio
    channels: 2
    device: "Soundflower (2ch)"
    format: FLOAT32LE
  playback:
    type: CoreAudio
    channels: 2
    device: "Built-in Output"
    format: FLOAT32LE
```

### Device names
The device names that are used for `device:` for both playback and capture are entered as shown in the "Audio MIDI Setup" that can be found under "Other" in Launchpad. 
The name for the 2-channel interface of Soundflower is "Soundflower (2ch)", and the built in audio in a MacBook Pro is called "Built-in Output".

Specifying "default" will give the default capture or playback device.

To help with finding the name of playback and capture devices, use the macOS version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases
Just download the binary and run it in a terminal. It will list all devices with the names.

### Sample format
Currently, the sample format should always be set to 32-bit float, `FLOAT32LE`. This is the format that CoreAudio uses internally. 
The sample format used by the actual DAC is the one that has been configured in "Audio MIDI Setup". 
Use "Audio MIDI Setup" to select a combination of sample rate and format, for example "2 ch 24-bit Integer 44.1 kHz".
CamillaDSP will switch the sample rate, and leave the sample format unchanged.