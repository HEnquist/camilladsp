## Alsa (linux)

### Find name of device
To list all hardware playback devices use the `aplay -l` command:
```
> aplay -l
**** List of PLAYBACK Hardware Devices ****
card 0: Generic [HD-Audio Generic], device 0: ALC236 Analog [ALC236 Analog]
  Subdevices: 1/1
  Subdevice #0: subdevice #0
```
Capture devices can be found in the same way with `arecord -l`.

To use the ALC236 device from above, put either `hw:Generic` (to use the name, recommended) or `hw:0` (to use the index) in the CamillaDSP config. The `hw:` prefix means it's a hardware device.

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
The interesting fields are FORMAT, RATE and CHANNELS. In this example the sample formats this device can use are S16_LE and S32_LE (corresponding to S16LE and S32LE in CamillaDSP, see the table in the main README for all formats). The sample rate can be either 44.1, or 48 kHz. And it supports only stereo playback.


Capture parameters are determined in the same way with `arecord`:
```
> arecord -D hw:Generic /dev/null --dump-hw-params
```
This outputs the same table as for the aplay example above, but for a capture device. 

A minimal configuration using the "Generic" card from above for both playback and capture could look like this:
```
devices:
  samplerate: 48000
  chunksize: 1024
  capture:
    type: Alsa
    channels: 2
    device: "hw:Generic"
    format: S32LE
  playback:
    type: Alsa
    channels: 2
    device: "hw:Generic"
    format: S32LE
``` 

## CoreAudio (macOS)
The device name is the same as the one shown in the "Audio MIDI Setup" that can be found under "Other" in Launchpad. The name for the 2-channel interface of Soundflower is "Soundflower (2ch)", and the built in audio in a MacBook Pro is called "Built-in Output".

The sample format is always 32-bit float (FLOAT32LE) even if the device is configured to use another format.

To help with finding the name of playback and capture devices, use the macOS version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases

Just download the binary and run it in a terminal. It will list all devices with the names and parameters to enter in the configuration.


## WASAPI (Windows)
The device name is the same as seen in the Windows volume control. For example, the VB-CABLE device name is "CABLE Output (VB-Audio Virtual Cable)". The device name is built from the input/output name and card name, and the format is "{input/output name} ({card name})".

The sample format is always 32-bit float (FLOAT32LE) even if the device is configured to use another format.

The sample rate must match the default format of the device. To change this, open "Sound" in the Control panel, select the sound card, and click "Properties". Then open the "Advanced" tab and select the desired format under "Default Format".

To help with finding the name of playback and capture devices, use the Windows version of "cpal-listdevices" program from here: https://github.com/HEnquist/cpal-listdevices/releases

Just download the binary and run it in a terminal. It will list all devices with the names and parameters to enter in the configuration.

