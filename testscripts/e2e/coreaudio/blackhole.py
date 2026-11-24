"""The BlackHole devices the CoreAudio suite runs on, and the tools that drive them.

Two BlackHole devices, installed from the latest casks, each a loopback: whatever any
client plays into a device's output comes out of the same device's input.

- BlackHole 2ch is the capture side. The test feeds its output with a `Feeder`,
  CamillaDSP captures its input.
- BlackHole 16ch is the playback side. CamillaDSP plays into the first two of its
  sixteen output channels, and the test records them from its input with `record` when
  it wants to see the output.

Four properties of BlackHole decide how the tests are written:

- Its only physical format is 32 bit float, at every rate it has. So a `format` other
  than F32 in the config has to be refused, and since CamillaDSP always talks float32 to
  CoreAudio, F32 all the way through is bit exact.
- A device has one nominal sample rate, shared by every client and outliving all of
  them. CamillaDSP sets it when it opens a device, and a test can change it underneath
  a running CamillaDSP, which is what fires the backend's rate listener.
- It has two clock sources, "Internal Fixed" and "Internal Adjustable", and on the
  adjustable one the output stereo pan is a pitch control, `1 + 0.02 * (pan - 0.5)`.
  The CoreAudio backend switches its capture device to the adjustable clock and writes
  the pan to do rate adjust. The tests do the same to the sink to skew it.
- Both devices run on the host clock, so with no pitch set the two ends of CamillaDSP
  run at the same rate and nothing drifts.

The device properties are read and written through the CoreAudio C API with ctypes,
since no Python package exposes them. The audio goes through sounddevice, which hands
float32 through untouched, where sox would pass it through 32 bit integers.
"""

import ctypes
import ctypes.util
import threading
import time

import numpy as np

FEED_DEVICE = "BlackHole 2ch"
SINK_DEVICE = "BlackHole 16ch"
SINK_CHANNELS = 16

NOMINAL_RATE = 48000
FIXED_CLOCK = "Internal Fixed"
ADJUSTABLE_CLOCK = "Internal Adjustable"


def _fourcc(code):
    return int.from_bytes(code.encode("ascii"), "big")


_SYSTEM_OBJECT = 1
_SCOPE_GLOBAL = _fourcc("glob")
_SCOPE_OUTPUT = _fourcc("outp")
_ELEMENT_MAIN = 0

_DEVICES = _fourcc("dev#")
_NAME = _fourcc("lnam")
_NOMINAL_RATE = _fourcc("nsrt")
_CLOCK_SOURCE = _fourcc("csrc")
_CLOCK_SOURCES = _fourcc("csc#")
_CLOCK_SOURCE_NAME = _fourcc("lcsn")
_STEREO_PAN = _fourcc("span")
_HOG_MODE = _fourcc("oink")
_RUN_LOOP = _fourcc("rnlp")

_UTF8 = 0x08000100


class _Address(ctypes.Structure):
    _fields_ = [
        ("selector", ctypes.c_uint32),
        ("scope", ctypes.c_uint32),
        ("element", ctypes.c_uint32),
    ]


class _Translation(ctypes.Structure):
    _fields_ = [
        ("input", ctypes.c_void_p),
        ("input_size", ctypes.c_uint32),
        ("output", ctypes.c_void_p),
        ("output_size", ctypes.c_uint32),
    ]


_ca = None
_cf = None


def _libs():
    """Load the two frameworks on first use, so importing this module works anywhere."""
    global _ca, _cf
    if _ca is None:
        _ca = ctypes.CDLL(ctypes.util.find_library("CoreAudio"))
        _cf = ctypes.CDLL(ctypes.util.find_library("CoreFoundation"))
        _ca.AudioObjectGetPropertyDataSize.argtypes = [
            ctypes.c_uint32, ctypes.POINTER(_Address), ctypes.c_uint32, ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32),
        ]
        _ca.AudioObjectGetPropertyData.argtypes = [
            ctypes.c_uint32, ctypes.POINTER(_Address), ctypes.c_uint32, ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p,
        ]
        _ca.AudioObjectSetPropertyData.argtypes = [
            ctypes.c_uint32, ctypes.POINTER(_Address), ctypes.c_uint32, ctypes.c_void_p,
            ctypes.c_uint32, ctypes.c_void_p,
        ]
        _cf.CFStringGetCString.argtypes = [
            ctypes.c_void_p, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32,
        ]
        _cf.CFRelease.argtypes = [ctypes.c_void_p]
        # The HAL caches property values in the client and only hears about changes
        # through notifications, which it delivers on the run loop it is told about.
        # Python runs none, so without this a value read back after a change can be
        # stale. NULL has the HAL run its own thread for them.
        null = ctypes.c_void_p(None)
        address = _Address(_RUN_LOOP, _SCOPE_GLOBAL, _ELEMENT_MAIN)
        _ca.AudioObjectSetPropertyData(
            _SYSTEM_OBJECT, ctypes.byref(address), 0, None, ctypes.sizeof(null), ctypes.byref(null)
        )
    return _ca, _cf


class CoreAudioError(RuntimeError):
    pass


def _check(status, what):
    if status != 0:
        code = status.to_bytes(4, "big", signed=True)
        text = code.decode("ascii") if all(32 <= b < 127 for b in code) else str(status)
        raise CoreAudioError(f"{what} failed with {text}")


def _get(obj, selector, ctype, scope=_SCOPE_GLOBAL):
    ca, _ = _libs()
    value = ctype()
    size = ctypes.c_uint32(ctypes.sizeof(value))
    address = _Address(selector, scope, _ELEMENT_MAIN)
    status = ca.AudioObjectGetPropertyData(
        obj, ctypes.byref(address), 0, None, ctypes.byref(size), ctypes.byref(value)
    )
    _check(status, f"reading {selector:#x} of {obj}")
    return value.value


def _set(obj, selector, ctype, value, scope=_SCOPE_GLOBAL):
    ca, _ = _libs()
    data = ctype(value)
    address = _Address(selector, scope, _ELEMENT_MAIN)
    status = ca.AudioObjectSetPropertyData(
        obj, ctypes.byref(address), 0, None, ctypes.sizeof(data), ctypes.byref(data)
    )
    _check(status, f"writing {selector:#x} of {obj}")


def _get_array(obj, selector, scope=_SCOPE_GLOBAL):
    ca, _ = _libs()
    address = _Address(selector, scope, _ELEMENT_MAIN)
    size = ctypes.c_uint32(0)
    status = ca.AudioObjectGetPropertyDataSize(
        obj, ctypes.byref(address), 0, None, ctypes.byref(size)
    )
    if status != 0:
        return []
    values = (ctypes.c_uint32 * (size.value // 4))()
    status = ca.AudioObjectGetPropertyData(
        obj, ctypes.byref(address), 0, None, ctypes.byref(size), values
    )
    _check(status, f"reading {selector:#x} of {obj}")
    return list(values)


def _cfstring(ref):
    _, cf = _libs()
    if not ref:
        return ""
    buffer = ctypes.create_string_buffer(512)
    cf.CFStringGetCString(ref, buffer, len(buffer), _UTF8)
    cf.CFRelease(ref)
    return buffer.value.decode("utf-8")


def device_names():
    """Every CoreAudio device's name, input or output."""
    return [_cfstring(_get(dev, _NAME, ctypes.c_void_p)) for dev in _get_array(_SYSTEM_OBJECT, _DEVICES)]


def device_id(name):
    """The id of a device by name, or None. BlackHole is one device for both directions."""
    for dev in _get_array(_SYSTEM_OBJECT, _DEVICES):
        if _cfstring(_get(dev, _NAME, ctypes.c_void_p)) == name:
            return dev
    return None


def _require(name):
    dev = device_id(name)
    if dev is None:
        raise CoreAudioError(f"no device named {name!r}")
    return dev


def devices_present():
    """Whether both BlackHole devices are installed, which is what every test here needs."""
    try:
        names = device_names()
    except (OSError, TypeError, CoreAudioError):
        return False
    return FEED_DEVICE in names and SINK_DEVICE in names


def nominal_rate(name):
    return _get(_require(name), _NOMINAL_RATE, ctypes.c_double)


def set_nominal_rate(name, rate):
    _set(_require(name), _NOMINAL_RATE, ctypes.c_double, float(rate))


def wait_for_nominal_rate(name, rate, timeout=5.0):
    """Wait for a rate change to have landed, since the HAL applies it asynchronously."""
    deadline = time.monotonic() + timeout
    while nominal_rate(name) != rate:
        if time.monotonic() > deadline:
            raise TimeoutError(f"{name} is at {nominal_rate(name)} Hz, not {rate} Hz")
        time.sleep(0.02)


def clock_sources(name):
    """The clock sources of a device, as a name to id mapping."""
    dev = _require(name)
    ca, _ = _libs()
    sources = {}
    for source in _get_array(dev, _CLOCK_SOURCES):
        source_in = ctypes.c_uint32(source)
        name_out = ctypes.c_void_p()
        translation = _Translation(
            ctypes.cast(ctypes.byref(source_in), ctypes.c_void_p), 4,
            ctypes.cast(ctypes.byref(name_out), ctypes.c_void_p), ctypes.sizeof(name_out),
        )
        size = ctypes.c_uint32(ctypes.sizeof(translation))
        address = _Address(_CLOCK_SOURCE_NAME, _SCOPE_GLOBAL, _ELEMENT_MAIN)
        status = ca.AudioObjectGetPropertyData(
            dev, ctypes.byref(address), 0, None, ctypes.byref(size), ctypes.byref(translation)
        )
        _check(status, f"reading the name of clock source {source}")
        sources[_cfstring(name_out.value)] = source
    return sources


def clock_source(name):
    """The name of the clock source a device is on."""
    current = _get(_require(name), _CLOCK_SOURCE, ctypes.c_uint32)
    for source_name, source in clock_sources(name).items():
        if source == current:
            return source_name
    return None


def set_clock_source(name, source_name, timeout=5.0):
    """Switch clock source and wait for it to land.

    Written again on every poll, since the settings the HAL restores after a rate change
    can land just after a write and undo it. On the adjustable clock the pan control
    only appears a couple of hundred milliseconds after the switch, so that is waited
    for too, and a pan write straight after this cannot miss it.
    """
    dev = _require(name)
    source = clock_sources(name)[source_name]
    deadline = time.monotonic() + timeout
    while True:
        if _get(dev, _CLOCK_SOURCE, ctypes.c_uint32) != source:
            _set(dev, _CLOCK_SOURCE, ctypes.c_uint32, source)
        time.sleep(0.02)
        if _get(dev, _CLOCK_SOURCE, ctypes.c_uint32) == source:
            if source_name != ADJUSTABLE_CLOCK:
                return
            try:
                _get(dev, _STEREO_PAN, ctypes.c_float, scope=_SCOPE_OUTPUT)
                return
            except CoreAudioError:
                pass
        if time.monotonic() > deadline:
            raise TimeoutError(f"{name} did not switch to {source_name} in {timeout} s")


def pitch(name):
    """A device's pitch, from its output stereo pan the way the backend writes it.

    The pan only exists on the adjustable clock, and the fixed one runs at nominal.
    """
    if clock_source(name) != ADJUSTABLE_CLOCK:
        return 1.0
    pan = _get(_require(name), _STEREO_PAN, ctypes.c_float, scope=_SCOPE_OUTPUT)
    return 1.0 + 0.02 * (pan - 0.5)


def set_pitch(name, value, timeout=5.0):
    """Put a device on its adjustable clock and run it at `value` times nominal.

    Written until it reads back. A pan written just after the switch to the adjustable
    clock is accepted without an error and then lost, a second write sticks.

    The clock can also be switched back from under it, by the settings the HAL restores
    after a rate change, which takes the pan control away. That goes round again from
    the switch.
    """
    dev = _require(name)
    pan = min(max((value - 1.0) * 50.0 + 0.5, 0.0), 1.0)
    deadline = time.monotonic() + timeout
    while True:
        try:
            set_clock_source(name, ADJUSTABLE_CLOCK)
            _set(dev, _STEREO_PAN, ctypes.c_float, pan, scope=_SCOPE_OUTPUT)
            time.sleep(0.05)
            if abs(_get(dev, _STEREO_PAN, ctypes.c_float, scope=_SCOPE_OUTPUT) - pan) < 1e-6:
                return
        except CoreAudioError:
            pass
        if time.monotonic() > deadline:
            raise TimeoutError(f"the pan of {name} would not stay at {pan}")


def hog_owner(name):
    """The pid holding a device in hog mode, or -1 when nobody does."""
    return _get(_require(name), _HOG_MODE, ctypes.c_int32)


def reset(name, attempts=5):
    """Put a device back where a fresh install has it: nominal rate, fixed clock, pitch 1.

    All three belong to the device and outlive the process that set them, so a test that
    leaves one behind would change every test after it, and the user's own setup too.

    The rate goes first. A rate change has the HAL put back the clock source and pan it
    last saw, and that can land a little after the new rate reads back, so the end
    state is checked again after a pause and redone if it was reverted.
    """
    if nominal_rate(name) != NOMINAL_RATE:
        set_nominal_rate(name, NOMINAL_RATE)
        wait_for_nominal_rate(name, NOMINAL_RATE)
    sources = clock_sources(name)
    for _ in range(attempts):
        if ADJUSTABLE_CLOCK in sources:
            set_pitch(name, 1.0)
        if FIXED_CLOCK in sources:
            set_clock_source(name, FIXED_CLOCK)
        time.sleep(0.3)
        if FIXED_CLOCK not in sources or clock_source(name) == FIXED_CLOCK:
            return
    raise RuntimeError(f"{name} keeps going back to {clock_source(name)}")


def noise(frames, channels=2, seed=1):
    """Random float32 samples inside +/-0.9, so nothing clips.

    Random rather than a tone because a window of noise matches the input at exactly one
    offset, which is what lets a test find where a recording sits in the loop the feeder
    plays.
    """
    rng = np.random.default_rng(seed)
    return rng.uniform(-0.9, 0.9, (frames, channels)).astype(np.float32)


def sine(frames, level_db=-6.0, freq=1000.0, rate=NOMINAL_RATE, channels=2):
    """A float32 sine at `level_db` peak. `frames` should hold a whole number of periods."""
    amplitude = 10 ** (level_db / 20)
    wave = amplitude * np.sin(2 * np.pi * freq * np.arange(frames) / rate)
    return np.repeat(wave[:, None], channels, axis=1).astype(np.float32)


def find_in_loop(block, window):
    """Where `window` starts in `block` played in a loop, or None if it is not in it.

    The window has to match exactly and in full, so this is the bit exactness check as
    well as the alignment.
    """
    doubled = np.concatenate([block, block])
    candidates = np.flatnonzero((block == window[0]).all(axis=1))
    for start in candidates:
        if np.array_equal(doubled[start : start + len(window)], window):
            return int(start)
    return None


def exact_fraction(block, window, piece):
    """The fraction of `piece` frame pieces of `window` found exactly in `block` looped.

    A dropout leaves a gap in the output, and one can land anywhere on a busy runner, so
    a window that has to match in one piece would fail on timing rather than on the
    audio. Cut into pieces, a conversion bug still fails every piece, while a gap only
    breaks the piece it falls in.
    """
    pieces = [window[start : start + piece] for start in range(0, len(window) - piece + 1, piece)]
    found = sum(1 for part in pieces if find_in_loop(block, part) is not None)
    return found / len(pieces)


class Feeder:
    """A block played into BlackHole 2ch in a loop, until stopped.

    Opened at whatever rate the device is at, so the HAL has nothing to convert and the
    samples arrive as written. Tests that want another rate set the device first.
    """

    def __init__(self, block, rate=NOMINAL_RATE, device=FEED_DEVICE):
        import sounddevice

        self._block = np.ascontiguousarray(block, dtype=np.float32)
        self._position = 0
        self._lock = threading.Lock()
        self.stream = sounddevice.OutputStream(
            device=device,
            samplerate=rate,
            channels=self._block.shape[1],
            dtype="float32",
            callback=self._callback,
        )
        self.stream.start()

    def _callback(self, outdata, frames, _time, _status):
        with self._lock:
            filled = 0
            while filled < frames:
                take = min(frames - filled, len(self._block) - self._position)
                outdata[filled : filled + take] = self._block[self._position : self._position + take]
                filled += take
                self._position = (self._position + take) % len(self._block)

    def stop(self):
        self.stream.abort()
        self.stream.close()


def record(frames, rate=NOMINAL_RATE, device=SINK_DEVICE, channels=2):
    """Record `frames` of the first `channels` channels of a device, as float32."""
    import sounddevice

    width = SINK_CHANNELS if device == SINK_DEVICE else channels
    data = sounddevice.rec(
        frames, samplerate=rate, channels=width, dtype="float32", device=device, blocking=True
    )
    return np.ascontiguousarray(data[:, :channels])

