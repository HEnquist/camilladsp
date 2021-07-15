# WASAPI (Windows)

## Introduction

WASAPI is a relatively new audio api, that was introduced with Windows Vista. 
It offers two modes that are intended for different use cases.
- Shared mode: As the name suggests, this mode is used when an audio device should be shared by several applications. The audio device then operates at a fixed sample rate, and every stream sent to it (or recorded from it) is resampled to the shared rate. The audio passes through the Windows mixer. Most applications use this mode. The sample rate of the device in shared mode is set in the Sound control panel.
- Exclusive mode: In this mode one application takes full control over an audio device. Only one application at a time can use the device. The sample rate and sample format can be changed, and the audio does not pass through the Windows mixer. This allows bit-perfect playback at any sample rate and sample format the hardware supports. While an application holds the device in exclusive mode, other apps will not be able to play for example notification sounds. This mode is often used for high quality music playback.

## Capturing audio from other applications

Audio can be captured in two ways.
### Virtual sound card 

To capture audio from applications a virtual sound card is used. [VB-CABLE from VB-AUDIO](https://www.vb-audio.com/Cable/) works well.

Set VB-CABLE as the default playback device in the Windows sound control panel. Open "Sound" in the Control Panel, then in the "Playback" tab select "CABLE Input" and click the "Set Default" button. 
The next step is to figure out the device names to enter in the CamillaDSP configuration.
Stay on the "Playback" tab and locate the device you want to use for playback of the processed audio. Note the two lines with the name and description of the device.
Then switch to the Recording tab. There should be a device listed as "CABLE Output". 
See "Device names" below for how to convert this information into the the devicename to enter in the CamillaDSP configuration.

### Loopback

In loopback mode the audio is captured from a Playback device. 
In this mode, a spare unused sound card can be used instead of a virtual sound card. 
The built in audio of the computer should work. The quality of the card doesn't matter, 
since the audio data will not be routed through it. This requires using shared mode, see more below.

Open the Sound Control Panel app, and locate the unused card in the "Playback" tab. Set it as default device. See "Device names" below on how to write the device name to enter in the CamillaDSP configuration. 
Also on the "Playback" tab, locate the device you want to use for playback of the processed audio, and write down the device name.

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
The device name that is used for `device:` for both playback and capture is the same as shown in the Windows volume control. The device name is built from the input/output name and card name, and the format is "{input/output name} ({card name})". For example, the VB-CABLE device name is "CABLE Output (VB-Audio Virtual Cable)".

To help with finding the name of playback and capture devices, use the Windows version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases

Just download the binary and run it in a terminal. It will list all devices with the names. The parameters shown are for shared mode, more sample rates and sample formats will likely be available in exclusive mode.

### Exclusive or shared mode
- In exclusive mode CamillaDSP is able to control the sample rate of the devices. 
  The Windows audio mixer and volume control are both bypassed. 
  The settings for "Default format" in the Windows Sound Control Panel app isn't used. 
  The sample format must be one that the device driver can accept. 
  This usually matches the hardware capabilities of the device. 
  For example a 24-bit USB dac is likely to accept the `S16LE` and `S24LE3` formats. 
  Other formats may be supported depending on driver support.

- In shared mode the sample format in the CamillaDSP configuration is always 32-bit float (`FLOAT32LE`). 
  The sample rate in CamillaDSP must match the "Default format" setting of the device. 
  To change this, open "Sound" in the Control panel, select the sound card, and click "Properties". 
  Then open the "Advanced" tab and select the desired format under "Default Format". 
  Pick the sample rate you want, and the largest number of bits available.

Set `exclusive` to `true` to enable exclusive mode. Setting it to `false` or leaving it out means that shared mode will be used.

### Loopback capture
Setting `loopback` to `true` enables loopback capture. This requires shared mode. See the [Loopback](#loopback) section above for more details.