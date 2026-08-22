# ASIO (Windows)

## Introduction

The ASIO backend is an alternative audio backend for Windows.
It provides low-latency access to audio devices via ASIO drivers.
It is always included in Windows builds, so no special build options are needed.

This backend does not use the ASIO SDK from Steinberg.
It is an independent implementation that talks to the ASIO drivers directly
through the COM interfaces they expose.
There are therefore no extra license restrictions on builds with ASIO enabled,
and no SDK to download.
See the [ASIO backend](./README.md#asio-backend) section for details.

ASIO is a trademark of Steinberg Media Technologies GmbH.
CamillaDSP is not affiliated with or endorsed by Steinberg.

## ASIO4ALL and other generic wrapper drivers

Generic wrapper drivers such as ASIO4ALL, FlexASIO and Steinberg's
Generic Low Latency ASIO Driver are best avoided when possible.
All they do is make a device without its own ASIO driver
usable by applications that require ASIO, nothing more.
Note that they don't give any sound quality improvements:
Wasapi in exclusive mode is just as bit-perfect,
since it also gives CamillaDSP direct control of the device without going through
the Windows mixer. What a generic wrapper adds on top of that is an extra
emulation layer, which has been a source of real-world bugs and quirks.
For a device without its own ASIO driver,
using the [Wasapi backend in exclusive mode](./backend_wasapi.md#shared-or-exclusive-mode)
directly is usually more reliable.
One case where Wasapi alone isn't enough is when it exposes a multichannel
device as several separate stereo pairs instead of one multichannel device.
Devices like this typically ship with their own ASIO driver though,
and that native driver is almost always a better choice than a generic
wrapper such as ASIO4ALL.

### ASIO4ALL

ASIO4ALL tolerates only one driver instance per process.
Once an instance has been created and released,
creating another one either hangs in `ASIOInit` or takes the process down.
Its author has explained that the audio device stays open
until `ASIOStop` is called or the driver dll is unloaded,
which is what causes this.

CamillaDSP handles it in two ways.
It never reloads the driver to apply a sample rate change,
something only the Steinberg generic driver needs.
And it refuses to probe ASIO4ALL for capabilities,
since a probe creates an instance and releases it again.
A capability request for ASIO4ALL therefore returns an error,
and the GUI cannot list its supported rates and formats.
Capture, playback and sample rate changes all work as usual.

This was seen with ASIO4ALL 2.22.

## Configuration of devices

Set the device `type` to `Asio` for both capture and playback.

### Device names
The `device` parameter should be set to the name of the ASIO driver to use.
Available ASIO drivers are listed in the log output at startup (at debug level).
Note that ASIO exposes drivers rather than actual device availability,
so drivers for disconnected or powered‑off devices are still included
in the listing.

### Channels
Set the `channels` property to the number of channels you want to use.
The value may be lower than the number of channels the device provides,
any channels above the specified count are simply ignored.

### Sample format
The supported sample formats are:
- `S16_LE` - 16-bit signed integer
- `S24_4_LE` - 24-bit signed integer (in 32-bit container)
- `S24_3_LE` - 24-bit signed integer (packed 3-byte)
- `S32_LE` - 32-bit signed integer
- `F32_LE` - 32-bit float
- `F64_LE` - 64-bit float

If the `format` parameter is omitted, CamillaDSP will query the device
for its native sample format and use it automatically.
ASIO drivers do not perform sample format conversion,
so if a format is specified it must match the device's native format.
A mismatch will result in an error at startup.

## Using one or two devices
Capture and playback may use the same ASIO device, or two different ones.
The behaviour differs in an important way, because each ASIO device has its own clock.

### Same device, full duplex
When capture and playback name the same driver,
CamillaDSP operates them in full-duplex mode through a single shared driver instance.
Both directions then run on one hardware clock, which means:
- Resampling is not supported. `capture_samplerate` must equal `samplerate`,
  and no `resampler` may be configured.
- The two directions are inherently in sync, so no rate adjustment is needed.

```yaml
capture:
  type: Asio
  channels: 2
  device: "My ASIO Driver"
  format: S32_LE
playback:
  type: Asio
  channels: 2
  device: "My ASIO Driver"
  format: S32_LE
```

### Two different devices
Capture and playback may also name different drivers.
Each side then gets its own driver instance, and each device runs on its own clock.
Since two independent clocks always drift apart,
this requires asynchronous resampling to avoid a slow build-up of
buffer underruns or overruns:
- Set `enable_rate_adjust: true` and configure an asynchronous `resampler`.
- The sample formats do not have to match.
  Each device is queried separately and its native format is used.

```yaml
capture:
  type: Asio
  channels: 2
  device: "My Recording Interface"
playback:
  type: Asio
  channels: 2
  device: "My Playback Interface"
```
