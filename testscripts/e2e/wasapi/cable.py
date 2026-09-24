"""VB-Cable, the device the WASAPI and ASIO suite runs on, and the tools that drive it.

The free VB-Cable is one cable. Its render side has two endpoints, a 2 channel one and
"CABLE In 16 Ch", and whatever any client plays into either comes out of the capture
endpoint "CABLE Output". A second instance of the driver installs but brings no new
endpoints, so every test runs one way:

- Playback: CamillaDSP plays into the 2 channel render endpoint, and the test records
  CABLE Output with `record`.
- Capture: the test plays a tone into the render endpoint with a `Feeder`, and
  CamillaDSP captures CABLE Output.

Installed without the vendor's setup, see install_vbcable.ps1, the 2 channel render
endpoint is called "Speakers (VB-Audio Virtual Cable)" rather than "CABLE Input". So it is
found by the driver name, and by not being the 16 channel one.

Three properties of the cable decide how the tests are written:

- Nothing through it is bit exact, in shared or exclusive mode, since it processes
  internally. Tests assert level and frequency, never samples.
- It keeps its own rate, 48 kHz, whatever rate an exclusive client asks for, and converts.
- Its measured capture rate swings between about 47000 and 48600 Hz per 1 s reading, so a
  rate is only checked with a wide band or an average.

The audio goes through sounddevice on PortAudio's WASAPI host API, in shared mode with
auto conversion, so the test side opens at whatever rate it likes.
"""

import sys
import threading

import numpy as np

RATE = 48000
CHANNELS = 2
DRIVER = "(VB-Audio Virtual Cable)"
CAPTURE_NAME = "CABLE Output"
SIXTEEN = "16 Ch"

LEVEL_DB = -6.0
# On an FFT bin centre at 48 kHz, as in the rest of the suite. See test_spectrum.py.
TONE_HZ = 984.375


def _sounddevice():
    import sounddevice

    return sounddevice


def _wasapi_devices(output):
    """(index, name) of every WASAPI device of the cable facing the given way."""
    sd = _sounddevice()
    hostapi = next(i for i, api in enumerate(sd.query_hostapis()) if "WASAPI" in api["name"])
    key = "max_output_channels" if output else "max_input_channels"
    return [
        (index, dev["name"])
        for index, dev in enumerate(sd.query_devices())
        if dev["hostapi"] == hostapi and DRIVER in dev["name"] and dev[key] > 0
    ]


def render_endpoint():
    """(index, name) of the cable's 2 channel render endpoint."""
    return next(dev for dev in _wasapi_devices(output=True) if SIXTEEN not in dev[1])


def capture_endpoint():
    """(index, name) of CABLE Output."""
    return next(dev for dev in _wasapi_devices(output=False) if CAPTURE_NAME in dev[1])


def devices_present():
    """Whether the cable is installed, which is what every test here needs."""
    if sys.platform != "win32":
        return False
    try:
        render_endpoint()
        capture_endpoint()
    except (ImportError, OSError, StopIteration):
        return False
    return True


def _wasapi_settings():
    return _sounddevice().WasapiSettings(auto_convert=True)


class Feeder:
    """A sine played into the cable's render endpoint, until stopped.

    Generated in the callback from a running frame count, so it is continuous for as long
    as it plays, at any rate.
    """

    def __init__(self, level_db=LEVEL_DB, freq=TONE_HZ, rate=RATE):
        sd = _sounddevice()
        self._amplitude = 10 ** (level_db / 20)
        self._step = 2 * np.pi * freq / rate
        self._position = 0
        self._lock = threading.Lock()
        self.stream = sd.OutputStream(
            device=render_endpoint()[0],
            samplerate=rate,
            channels=CHANNELS,
            dtype="float32",
            callback=self._callback,
            extra_settings=_wasapi_settings(),
        )
        self.stream.start()

    def _callback(self, outdata, frames, _time, _status):
        with self._lock:
            phase = self._step * (self._position + np.arange(frames))
            outdata[:] = (self._amplitude * np.sin(phase)).astype(np.float32)[:, None]
            self._position += frames

    def stop(self):
        self.stream.abort()
        self.stream.close()


def record(frames, rate=RATE):
    """Record `frames` from CABLE Output, as float32."""
    sd = _sounddevice()
    with sd.InputStream(
        device=capture_endpoint()[0],
        samplerate=rate,
        channels=CHANNELS,
        dtype="float32",
        extra_settings=_wasapi_settings(),
    ) as stream:
        data, _overflowed = stream.read(frames)
    return np.ascontiguousarray(data)


def peak_frequency(column, rate=RATE):
    spectrum = np.abs(np.fft.rfft(column * np.hanning(len(column))))
    return np.fft.rfftfreq(len(column), 1 / rate)[spectrum.argmax()]


def sine_level_db(column):
    """The peak level of a sine in dB, from its RMS, so a stray sample does not move it."""
    rms = np.sqrt((column.astype(np.float64) ** 2).mean())
    return 20 * np.log10(rms * np.sqrt(2))


def assert_tone(data, rate=RATE, level_db=LEVEL_DB, freq=TONE_HZ, skip=0.5):
    """Every channel of `data` carries the tone, at its level and frequency.

    The first `skip` seconds are left out, since the two ends start at different times.
    One FFT bin of frequency error is allowed, and 1 dB of level, since the cable is not
    bit exact and a dropout on a busy runner takes a little off the RMS.
    """
    body = data[int(skip * rate) :]
    assert len(body) >= rate // 2, f"only {len(body)} frames after the first {skip} s"
    bin_width = rate / len(body)
    for channel in range(body.shape[1]):
        column = body[:, channel]
        level = sine_level_db(column)
        assert abs(level - level_db) < 1.0, f"channel {channel} at {level:.2f} dB"
        found = peak_frequency(column, rate)
        assert abs(found - freq) <= bin_width, f"channel {channel} peaks at {found:.1f} Hz"


# Config blocks, as text like the rest of the suite's device configs. Each returns the
# lines of one side, to go under `devices:`.


def wasapi_block(side, device, exclusive=False, fmt=None, extra=None):
    lines = [
        f"  {side}:",
        "    type: Wasapi",
        f"    channels: {CHANNELS}",
        f'    device: "{device}"',
        f"    exclusive: {str(exclusive).lower()}",
    ]
    if fmt is not None:
        lines.append(f"    format: {fmt}")
    lines += [f"    {key}: {_yaml(value)}" for key, value in (extra or {}).items()]
    return lines


def generator_block(level_db=LEVEL_DB, freq=TONE_HZ):
    return [
        "  capture:",
        "    type: SignalGenerator",
        f"    channels: {CHANNELS}",
        f"    signal: {{type: Sine, freq: {freq}, level: {level_db}}}",
    ]


def stdout_block(fmt="F32_LE"):
    return ["  playback:", "    type: Stdout", f"    channels: {CHANNELS}", f"    format: {fmt}"]


def _yaml(value):
    if isinstance(value, bool):
        return str(value).lower()
    return str(value)
