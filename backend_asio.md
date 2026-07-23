# ASIO (Windows)

## Introduction

The ASIO backend is an optional alternative audio backend for Windows.
It provides low-latency access to audio devices via ASIO drivers.
To use it, CamillaDSP must be compiled with the `asio-backend` feature enabled.
See [Building with ASIO backend (Windows)](./README.md#building-with-asio-backend-windows).

Note that the ASIO backend is licensed under GPLv3 only,
due to the ASIO SDK license requirements.
See the [ASIO backend and license implications](./README.md#asio-backend-and-license-implications)
section for details.

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

## Full-duplex limitations
When both capture and playback use the ASIO backend,
CamillaDSP operates them in full-duplex mode
through a single shared driver instance. This implies:
- **Same device:** Capture and playback must specify the same ASIO driver name.
  ASIO only supports one driver loaded at a time.
- **Same sample rate:** Resampling is not supported in full-duplex ASIO mode.
  Both directions share the same hardware clock,
  so `capture_samplerate` must equal `samplerate`
  and no `resampler` should be configured.

Example configuration:
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
