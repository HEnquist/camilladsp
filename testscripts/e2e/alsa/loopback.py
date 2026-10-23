"""The snd-aloop and snd-dummy devices the ALSA suite runs on, and the tools that drive them.

snd-aloop is loaded with `pcm_substreams=2`, which gives two cables. Whatever is played
into device 0 of a cable comes out of device 1 of the same cable, and the subdevice
number picks the cable:

- cable 0 is the capture side. The test feeds `FEED` with aplay, CamillaDSP captures
  from `CAPTURE`.
- cable 1 is the playback side. CamillaDSP plays into `PLAYBACK`, and the test records
  `RECORD` with arecord when it wants to see the output, or leaves it unopened when the
  cable is only there as a clock.

snd-dummy is the other playback, a sink with no far end. It only takes S16_LE and U8, at
most two channels and at most 48 kHz.

Three properties of snd-aloop decide how the tests are written:

- Once one end of a cable is open, the other end can only open with the same format,
  rate and channel count. That is what makes format autodetection testable, and a
  mismatch a way to make a device fail to open.
- Each cable has a `PCM Rate Shift 100000` control that runs it at
  `nominal * 100000 / value`, so above 100000 is slower. The control is per cable and
  named after the cable's capture end, device 1, whichever end is being asked about.
  The ALSA backend writes the one on its own capture cable to do rate adjust, and the
  tests write the one on the playback cable to skew the sink it is adjusting against.
- Both cables run on the kernel's system timer, so with no shift set the two ends of
  CamillaDSP run on the same clock and nothing drifts.

A second loopback card, `STALL_CARD`, is there for one thing the first cannot do: stop.
It is loaded with `timer_source` set to snd-dummy's first playback substream, so its
cables are clocked by that PCM's period interrupts instead of the system timer. With
`DummyClock` running on `DUMMY_CLOCK` the card runs, and when it is killed every stream
on the card stops moving, without an error and without leaving the running state. That
is what a UAC2 gadget looks like when the host closes its stream. CamillaDSP plays into
`STALL_PLAYBACK` on cable 0 to stall a playback, and the test records `STALL_RECORD` on
the far end of that cable when it wants to see the output. CamillaDSP captures from
`STALL_CAPTURE` on cable 1 to stall a capture. Nothing plays into cable 1, so the capture
records silence, at the pace of the clock. Three traps come with it:

- The loopback's period has to take exactly as long as the Dummy's, or every tick is
  rejected as the wrong resolution and the stream is stopped with an xrun. So the
  Dummy runs with the period size CamillaDSP is going to pick, see `DummyClock`.
- aplay on a stalled cable fails with EIO after its 100 ms write timeout, so it is no
  use as the far end there. CamillaDSP waits with a shorter timeout and sees the stall.
- The ticks reach the loopback through kernel work, which runs at normal priority, and
  the loopback moves one period per callback however many ticks it was late for. A
  callback that runs late loses periods for good, and the card then runs slow. A real
  time thread that keeps a core busy can hold that work back, and once stopped the card
  completely, which is how the threaded backend's busy wait was found.
"""

import os
import re
import subprocess
import threading
import time

import numpy as np

CARD = "Loopback"
FEED = "hw:Loopback,0,0"
CAPTURE = "hw:Loopback,1,0"
PLAYBACK = "hw:Loopback,0,1"
RECORD = "hw:Loopback,1,1"
DUMMY = "hw:Dummy"

STALL_CARD = "Loopback_1"
STALL_PLAYBACK = "hw:Loopback_1,0,0"
STALL_RECORD = "hw:Loopback_1,1,0"
STALL_CAPTURE = "hw:Loopback_1,1,1"
DUMMY_CLOCK = "hw:Dummy,0,0"

CAPTURE_CABLE = 0
PLAYBACK_CABLE = 1
NOMINAL_SHIFT = 100000

# The CamillaDSP name of a format, and what aplay and arecord call it.
ALSA_FORMAT = {
    "S16_LE": "S16_LE",
    "S24_3_LE": "S24_3LE",
    "S24_4_LE": "S24_LE",
    "S32_LE": "S32_LE",
    "F32_LE": "FLOAT_LE",
    "F64_LE": "FLOAT64_LE",
}
# Every format snd-aloop takes that CamillaDSP also has. The loopback has no FLOAT64.
LOOPBACK_FORMATS = ("S16_LE", "S24_3_LE", "S24_4_LE", "S32_LE", "F32_LE")
INT_BITS = {"S16_LE": 16, "S24_3_LE": 24, "S24_4_LE": 24, "S32_LE": 32}


def devices_present():
    """Whether both cards are loaded, which is what every test here needs."""
    return os.path.exists(f"/proc/asound/{CARD}") and os.path.exists("/proc/asound/Dummy")


def stall_card_present():
    """Whether the Dummy clocked loopback card is loaded, see the module docstring."""
    return os.path.exists(f"/proc/asound/{STALL_CARD}")


def _pcm_status(device, stream, subdevice, card):
    path = f"/proc/asound/{card}/pcm{device}{stream}/sub{subdevice}/status"
    with open(path) as status:
        return status.read()


def pcm_state(device, stream, subdevice, card=CARD):
    """The state of one substream, "RUNNING", "PREPARED", "closed" and so on.

    Read from procfs rather than asked of alsa-lib, so it can be polled while something
    else holds the device.
    """
    text = _pcm_status(device, stream, subdevice, card)
    if text.strip() == "closed":
        return "closed"
    match = re.search(r"state:\s*(\S+)", text)
    return match.group(1) if match else text.strip()


def hw_ptr(device, stream, subdevice, card=CARD):
    """How many frames the hardware side of a substream has moved since it started.

    This is what shows a stalled stream, since a stalled one stays in the running state.
    None when the substream is closed.
    """
    match = re.search(r"hw_ptr\s*:\s*(\d+)", _pcm_status(device, stream, subdevice, card))
    return int(match.group(1)) if match else None


def wait_for_pcm_state(device, stream, subdevice, state, timeout=5.0):
    deadline = time.monotonic() + timeout
    while True:
        current = pcm_state(device, stream, subdevice)
        if current == state:
            return
        if time.monotonic() > deadline:
            raise TimeoutError(
                f"pcm{device}{stream} sub{subdevice} is {current}, not {state} after {timeout} s"
            )
        time.sleep(0.02)


def _shift_control(cable):
    return f"iface=PCM,name='PCM Rate Shift 100000',device=1,subdevice={cable}"


def rate_shift(cable):
    out = subprocess.run(
        ["amixer", "-c", CARD, "cget", _shift_control(cable)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return int(re.search(r": values=(\d+)", out).group(1))


def set_rate_shift(cable, value, card=CARD):
    subprocess.run(
        ["amixer", "-q", "-c", card, "cset", _shift_control(cable), str(value)],
        check=True,
    )


def shifted_rate(nominal, shift):
    """The rate a cable runs at with a given shift, see the module docstring."""
    return nominal * NOMINAL_SHIFT / shift


def encode(fmt, values):
    """Samples as the bytes an ALSA device of `fmt` carries.

    Integer formats take integer values at their own scale, F32_LE takes floats, so a
    test can generate values that survive a round trip through CamillaDSP exactly and
    compare them without a tolerance.
    """
    values = np.asarray(values)
    if fmt == "S16_LE":
        return values.astype("<i2").tobytes()
    if fmt in ("S32_LE", "S24_4_LE"):
        return values.astype("<i4").tobytes()
    if fmt == "S24_3_LE":
        return values.astype("<i4").view("u1").reshape(-1, 4)[:, :3].tobytes()
    if fmt == "F32_LE":
        return values.astype("<f4").tobytes()
    raise ValueError(f"no encoder for {fmt}")


def decode(fmt, data, channels=2):
    """The inverse of `encode`, one column per channel.

    S24_LE carries 24 bits in a 32 bit container and the top byte is not part of the
    sample, so it is sign extended from bit 23 rather than read, whichever convention
    the writer used for it.
    """
    if fmt == "S16_LE":
        values = np.frombuffer(data, dtype="<i2").astype(np.int64)
    elif fmt == "S32_LE":
        values = np.frombuffer(data, dtype="<i4").astype(np.int64)
    elif fmt == "S24_4_LE":
        values = (np.frombuffer(data, dtype="<i4") << 8) >> 8
        values = values.astype(np.int64)
    elif fmt == "S24_3_LE":
        raw = np.frombuffer(data, dtype="u1").reshape(-1, 3)
        wide = np.zeros((len(raw), 4), dtype="u1")
        wide[:, 1:] = raw
        values = (wide.view("<i4").ravel() >> 8).astype(np.int64)
    elif fmt == "F32_LE":
        values = np.frombuffer(data, dtype="<f4").astype(np.float64)
    else:
        raise ValueError(f"no decoder for {fmt}")
    return values.reshape(-1, channels)


def noise(fmt, frames, channels=2, seed=1):
    """Random samples over the whole range of `fmt`, as values for `encode`.

    Random rather than a tone because a window of noise matches the input at exactly one
    offset, which is what lets a test find where a recording sits in the loop the
    feeder plays. Floats stay inside +/-0.9 so nothing clips.
    """
    rng = np.random.default_rng(seed)
    if fmt == "F32_LE":
        return rng.uniform(-0.9, 0.9, (frames, channels)).astype(np.float32)
    bits = INT_BITS[fmt]
    return rng.integers(-(2 ** (bits - 1)), 2 ** (bits - 1), (frames, channels), dtype=np.int64)


def sine(fmt, frames, level_db=-6.0, freq=1000.0, rate=48000, channels=2):
    """A sine at `level_db` peak, as values for `encode`.

    `frames` should hold a whole number of periods, so the feeder can loop it without a
    discontinuity that would show up as a peak above the level.
    """
    amplitude = 10 ** (level_db / 20)
    wave = amplitude * np.sin(2 * np.pi * freq * np.arange(frames) / rate)
    wave = np.repeat(wave[:, None], channels, axis=1)
    if fmt == "F32_LE":
        return wave.astype(np.float32)
    return np.round(wave * 2 ** (INT_BITS[fmt] - 1)).astype(np.int64)


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

    An xrun leaves a gap in the output, and on a jittery VM one can land anywhere, so a
    window that has to match in one piece fails on timing rather than on the audio. Cut
    into pieces, a conversion bug still fails every piece, since no sample survives it,
    while a gap only breaks the piece it falls in.
    """
    pieces = [window[start : start + piece] for start in range(0, len(window) - piece + 1, piece)]
    found = sum(1 for part in pieces if find_in_loop(block, part) is not None)
    return found / len(pieces)


class Feeder:
    """aplay on the far end of a loopback cable, playing a block in a loop until stopped.

    The block is written from a thread, since aplay only takes what fits in its buffer
    and blocks for the rest. Stopping kills aplay rather than closing its stdin, which
    would have it drain the buffer first, and the kill is what closes the cable end at
    once the way an application quitting does.
    """

    def __init__(self, block, fmt="S16_LE", rate=48000, channels=2, device=FEED):
        self.device = device
        self.process = subprocess.Popen(
            [
                "aplay", "-q", "-D", device, "-t", "raw",
                "-f", ALSA_FORMAT[fmt], "-r", str(rate), "-c", str(channels),
            ],
            stdin=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._write, args=(block,), daemon=True)
        self._thread.start()

    def _write(self, block):
        try:
            while not self._stop.is_set():
                self.process.stdin.write(block)
                self.process.stdin.flush()
        except (BrokenPipeError, OSError, ValueError):
            pass

    def wait_until_running(self, timeout=5.0):
        """Wait for the cable end to be running, and fail with aplay's own error if it died."""
        card, dev, sub = _card_device_and_subdevice(self.device)
        deadline = time.monotonic() + timeout
        while pcm_state(dev, "p", sub, card=card) != "RUNNING":
            if self.process.poll() is not None:
                raise RuntimeError(f"aplay exited: {self.process.stderr.read().decode().strip()}")
            if time.monotonic() > deadline:
                raise TimeoutError(f"{self.device} was not running after {timeout} s")
            time.sleep(0.02)
        return self

    def stop(self):
        self._stop.set()
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)
        self._thread.join(timeout=5)


class DummyClock:
    """aplay playing silence into `DUMMY_CLOCK`, which is what drives `STALL_CARD`.

    The period has to match the one CamillaDSP gets on the stall card, see the module
    docstring. The playback backend asks for a buffer of three chunks rounded up to a
    power of two and a period of an eighth of that, so a chunk size of 1024 gives 512,
    and one of 1536 gives 1024.
    """

    def __init__(self, period, rate=48000):
        self.process = subprocess.Popen(
            [
                "aplay", "-q", "-D", DUMMY_CLOCK, "-t", "raw", "-f", "S16_LE",
                "-r", str(rate), "-c", "2",
                f"--period-size={period}", f"--buffer-size={8 * period}", "/dev/zero",
            ],
            stderr=subprocess.PIPE,
        )
        deadline = time.monotonic() + 5.0
        while pcm_state(0, "p", 0, card="Dummy") != "RUNNING":
            if self.process.poll() is not None:
                raise RuntimeError(f"aplay exited: {self.process.stderr.read().decode().strip()}")
            if time.monotonic() > deadline:
                raise TimeoutError(f"{DUMMY_CLOCK} was not running after 5 s")
            time.sleep(0.02)

    def stop(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)


def record(frames, fmt="S16_LE", rate=48000, channels=2, device=RECORD):
    """Record `frames` from a loopback cable with arecord and return the raw bytes."""
    result = subprocess.run(
        [
            "arecord", "-q", "-D", device, "-t", "raw",
            "-f", ALSA_FORMAT[fmt], "-r", str(rate), "-c", str(channels),
            "-s", str(frames),
        ],
        capture_output=True,
        timeout=frames / rate + 10,
    )
    if result.returncode != 0:
        raise RuntimeError(f"arecord failed: {result.stderr.decode().strip()}")
    return result.stdout


def _card_device_and_subdevice(name):
    """The card name and the device and subdevice numbers of a "hw:Card,D,S" name."""
    card, dev, sub = name.split(",")
    return card.removeprefix("hw:"), int(dev), int(sub)
