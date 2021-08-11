# WASAPI (Windows)

## Introduction

The WASAPI audio API was introduced with Windows Vista. 
It offers two modes, "shared" and "exclusive", that offer different features and are intended for different use cases. CamillaDSP supports both modes.

### Shared mode
This is the mode that most applications use. As the name suggests, this mode allows an audio device to be shared by several applications.

In shared mode the audio device then operates at a fixed sample rate and sample format. Every stream sent to it (or recorded from it) is resampled to/from the shared rate and format. The sample rate and output sample format of the device are called the "Default format" of the device and can be set in the Sound control panel. Internally, the Windows audio stack uses 32-bit float as the sample format.
The audio passes through the Windows mixer and volume control.

In shared mode, these points apply for the CamillaDSP configuration:
- The `samplerate` parameter must match the "Default format" setting of the device. 
  To change this, open "Sound" in the Control panel, select the sound card, and click "Properties". 
  Then open the "Advanced" tab and select the desired format under "Default Format". 
  Pick the desired sample rate, and the largest number of bits available.
- [Loopback](#loopback-capture) capture mode is available.
- The sample format is always 32-bit float (`FLOAT32LE`). 


### Exclusive mode
This mode is often used for high quality music playback.

In this mode one application takes full control over an audio device. Only one application at a time can use the device. The sample rate and sample format can be changed, and the audio does not pass through the Windows mixer and volume control. This allows bit-perfect playback at any sample rate and sample format the hardware supports. While an application holds the device in exclusive mode, other apps will not be able to play for example notification sounds. 

In exclusive mode, these points apply for the CamillaDSP configuration:
- CamillaDSP is able to control the sample rate of the devices. 
- The sample format must be one that the device driver can accept. 
  This usually matches the hardware capabilities of the device. 
  For example a 24-bit USB dac is likely to accept the `S16LE` and `S24LE3` formats. 
  Other formats may be supported depending on driver support.
  Note that all sample formats may not be available at all sample rates. 
  A USB device might support both 16 and 24 bits at up to 96 kHz, but only 16 bits above that.
- [Loopback](#loopback-capture) capture mode is __not__ available.

## Capturing audio from other applications

CamillaDSP must capture audio from a capture device. This can either be a virtual sound card, or an additional card in loopback mode.

### Virtual sound card 

When using a virtual sound card (sometimes called loopback device), all applications output their audio to the playback side of this virtual sound card. Then this audio signal can be captured from the capture side of the virtual card. [VB-CABLE from VB-AUDIO](https://www.vb-audio.com/Cable/) works well.

#### Sending all audio to the virtual card
Set VB-CABLE as the default playback device in the Windows sound control panel. Open "Sound" in the Control Panel, then in the "Playback" tab select "CABLE Input" and click the "Set Default" button. This will work for all applications that respect this setting, which in practice is nearly all. The exceptions are the ones that provide their own way of selecting playback device.

#### Capturing the audio
The next step is to figure out the device name to enter in the CamillaDSP configuration.
Again open "Sound" in the Control Panel, and switch to the Recording tab. There should be a device listed as "CABLE Output". Unless the default names have been changed, the device name to enter in the CamillaDSP config is "CABLE Output (VB-Audio Virtual Cable)".
See also [Device names](#device-names) for more details on how to build the device names.

### Loopback capture
In loopback mode the audio is captured from a Playback device. This allows capturing the sound that a card is playing. In this mode, a spare unused sound card is used (note that this card can be either real or virtual). 
The built in audio of the computer should work. The quality of the card doesn't matter, 
since the audio data will not be routed through it. This requires using [Shared mode](#shared-mode).

Open the Sound Control Panel app, and locate the unused card in the "Playback" tab. Set it as default device. See [Device names](#device-names) for how to write the device name to enter in the CamillaDSP configuration. 

## Configuration of devices

This example configuration will be used to explain the various options specific to WASAPI:
```
  capture:
    type: Wasapi
    channels: 2
    device: "CABLE Output (VB-Audio Virtual Cable)"
    format: FLOAT32LE
    exclusive: false (*)
    loopback: false (*)
  playback:
    type: Wasapi
    channels: 2
    device: "SPDIF Interface (FX-AUDIO-DAC-X6)"
    format: S24LE3
    exclusive: true (*)
```

### Device names
The device names that are used for `device:` for both playback and capture are entered as shown in the Windows volume control. Click the speaker icon in the notification area, and then click the small up-arrow in the upper right corner of the volume control pop-up. This displays a list of all playback devices, with their names in the right format. The names can also be seen in the "Sound" control panel app. Look at either the "Playback" or "Recording" tab. The device name is built from the input/output name and card name, and the format is "{input/output name} ({card name})". For example, the VB-CABLE device name is "CABLE Output (VB-Audio Virtual Cable)", and the built in audio of a desktop computer can be "Speakers (Realtek(R) Audio)".

Specifying "default" will give the default capture or playback device.

To help with finding the name of playback and capture devices, use the Windows version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases

Just download the binary and run it in a terminal. It will list all devices with the names. The parameters shown are for shared mode, more sample rates and sample formats will likely be available in exclusive mode.

### Shared or exclusive mode
Set `exclusive` to `true` to enable exclusive mode. Setting it to `false` or leaving it out means that shared mode will be used. Playback and capture are independent, they do not need to use the same mode.

### Loopback capture
Setting `loopback` to `true` enables loopback capture. This requires using shared mode for the capture device. See [Loopback capture](#loopback-capture) for more details.