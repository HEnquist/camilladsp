# CoreAudio (macOS)

## Introduction
CoreAudio is the standard audio API of macOS. 
The CoreAudio support of CamillaDSP is provided by [an updated and extended fork](https://github.com/HEnquist/coreaudio-rs) of the [coreaudio-rs library](https://github.com/RustAudio/coreaudio-rs). 

CoreAudio is a large API that offers several ways to accomplish most common tasks. CamillaDSP uses the low-level AudioUnits for playback and capture. An AudioUnit that represents a hardware device has two stream formats. One format is used for communicating with the application. This is typically 32-bit float, the same format that CoreAudio uses internally. The other format (called the physical format) is the one used to send or receive data to/from the sound card driver. 

## Capturing audio from other applications

To capture audio from applications a virtual sound card is needed. 
This has been verified to work well with [Soundflower](https://github.com/mattingalls/Soundflower) 
and [BlackHole](https://github.com/ExistentialAudio/BlackHole). 
SoundFlower only supports Intel macs, while BlackHole supports both Intel and Apple Silicon. 
BlackHole has a 2-channel and a 16-channel version. 
There is currently a bug in the 2-channel version that in some cases can lead to choppy sound. 
If this happens, try the 16-channel version instead. 

Some player applications can use hog mode to get exclusive access to the playback device. Using this with a virtual soundcard like BlackHole causes problems, and is therefore not recommended.

### Sending all audio to the virtual card
Set the virtual sound card as the default playback device in the Sound preferences. This will work for all applications that respect this setting, which in practice is nearly all. The exceptions are the ones that provide their own way of selecting playback device.

### Capturing the audio
When applications output their audio to the playback side of the virtual soundcard, then this audio can be captured from the capture side.
This is done by giving the virtual soundcard as the capture device in the CamillaDSP configuration.

### Sample rate change notifications
CamillaDSP will listen for notifications from CoreAudio. If the sample rate of the capture device changes, then CoreAudio will stop providing new samples to any client currently capturing from it. To continue from this state, the capture device needs to be closed and reopened. For CamillaDSP this means that the configuration must be reloaded. If the capture device sample rate changes, then CamillaDSP will stop. Reading the "StopReason" via the websocket server tells that this was due to a sample rate change, and give the value for the new sample rate.

## Configuration of devices

This example configuration will be used to explain the various options specific to CoreAudio:
```
  capture:
    type: CoreAudio
    channels: 2
    device: "Soundflower (2ch)"
    format: S32LE (*)
    change_format: true (*)
  playback:
    type: CoreAudio
    channels: 2
    device: "Built-in Output"
    format: S24LE (*)
    change_format: true (*)
    exclusive: false (*)
```
The parameters marked (*) are optional.

### Device names
The device names that are used for `device:` for both playback and capture are entered as shown in the "Audio MIDI Setup" that can be found under "Other" in Launchpad. 
The name for the 2-channel interface of Soundflower is "Soundflower (2ch)", and the built in audio in a MacBook Pro is called "Built-in Output".

Specifying "default" will give the default capture or playback device.

To help with finding the name of playback and capture devices, use the macOS version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases
Just download the binary and run it in a terminal. It will list all devices with the names.

### Sample format
CamillaDSP always uses 32-bit float uses when transferring data to and from CoreAudio. The conversion from 32-bit float to the sample format used by the actual DAC (the physical format) is performed by CoreAudio.

The physical format can be set using the "Audio MIDI Setup" app.

The option `change_format` determines whether CamillaDSP should change the physical format or not. If it is enabled, then CamillaDSP will change the setting to match the selected `format`. 
To do this, it fetches a list of the supported stream formats for the device. 
It then searches the list until it finds a suitable one. 
The criteria is that it must have the right sample rate, the right number of bits, 
and the right number type (float or integer). 
There exact representation of the given format isn't used. 
This means that S24LE and S24LE3 are equivalent, and the "LE" ending that means 
little-endian for other backends is ignored.

This table shows the mapping between the format setting in "Audio MIDI Setup" and the CamillaDSP `format`:
- 16-bit Integer: S16LE
- 24-bit Integer: S24LE or S24LE3
- 32-bit Integer: S32LE
- 32-bit Float: FLOAT32LE

If the `change_format` is set to `false`, then CamillaDSP will leave the sample format unchanged, and only switch the sample rate.

Both `format` and `change_format` are optional. If left out, `format` defaults to 32-bit integer (S32LE), and `change_format` to false.

The playback device has an `exclusive` setting for whether CamillaDSP should request exclusive
access to the device or not. This is also known as hog mode. When enabled, no other application 
can output sound to the device while CamillaDSP runs. The setting is optional and defaults to false if left out.