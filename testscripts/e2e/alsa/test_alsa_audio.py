"""Audio through the ALSA backend on both ends: formats, rates, and what comes out.

Everything else in the suite reaches the engine through the Dummy or the software
devices, so this is the only place the ALSA capture and playback code runs at all: the
hw params negotiation, the sample format conversion into and out of the device buffers,
and the reads and writes against a device with a real clock behind it.

The loopback is bit exact, so with an empty pipeline what CamillaDSP plays should be
exactly what the feeder played. That is a stronger check than a level, since it catches
a byte order, a sign extension or a scale that is off by one bit, none of which moves a
peak meter.
"""

import time

import pytest

from .loopback import (
    DUMMY,
    LOOPBACK_FORMATS,
    decode,
    encode,
    find_in_loop,
    noise,
    record,
)

pytestmark = pytest.mark.alsa

EXIT_OK = 0
RATE = 48000
LEVEL_DB = -6.0


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    """Wait for both channels of a peak meter to read `level`, and return them."""
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.2 for peak in peaks),
        timeout=timeout,
    )


def wait_for_failure(cdsp):
    """Wait for a device that could not be opened to have stopped the engine.

    Same gate as test_failures.py: the stop reason is what the tests assert on, so it is
    what gets polled, and the state is only checked after it.
    """
    reason = cdsp.poll_until_true("GetStopReason", lambda value: value != "None")
    cdsp.poll_until("GetState", "Inactive")
    return reason


@pytest.mark.parametrize("fmt", LOOPBACK_FORMATS)
def test_audio_passes_through_bit_exact(start_cdsp, alsa_config, feeder, fmt):
    """A second of noise in a loop, and the tail of the output has to match it exactly.

    Every value in the input survives the round trip through the 64 bit pipeline, the
    integer formats because their full range fits in a double's mantissa and F32 because
    widening a float is exact, so any difference at all is a bug in the conversion.
    """
    block = decode(fmt, encode(fmt, noise(fmt, RATE)))
    feeder(block=encode(fmt, block), fmt=fmt)
    start_cdsp(config=alsa_config(capture_format=fmt, playback_format=fmt))
    recorded = decode(fmt, record(RATE * 3 // 2, fmt=fmt))
    # The tail, well clear of whatever the start of the recording caught.
    window = recorded[-RATE // 2 :]
    assert find_in_loop(block, window) is not None, "the output is not the input"


@pytest.mark.parametrize("fmt", LOOPBACK_FORMATS)
def test_the_capture_format_is_taken_from_the_device(start_cdsp, alsa_config, feeder, fmt):
    """With no format in the config, the backend has to find the one the device runs.

    The feeder has already locked the cable to its format, so exactly one format opens,
    and a wrong pick is a failure to start rather than a wrong level.
    """
    feeder(fmt=fmt)
    cdsp = start_cdsp(config=alsa_config(capture_format=None))
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_format_the_device_lacks_is_refused(start_cdsp, alsa_config, feeder, side):
    """The loopback has no FLOAT64, so asking for F64_LE has to fail to open.

    Validation cannot catch this, the config is fine. It is the device saying no, and
    the engine has to report it against the right side and keep running under --wait.
    """
    feeder()
    config = alsa_config(**{f"{side}_format": "F64_LE"})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_failure(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


def test_a_rate_the_source_is_not_running_is_refused(start_cdsp, alsa_config, feeder):
    """A source already playing at 44.1 kHz cannot be captured at 48 kHz.

    The same thing happens with a real loopback when the player starts first, so this is
    what a user sees when the config and the player disagree.
    """
    feeder(rate=44100)
    cdsp = start_cdsp(config=alsa_config(), extra_args=["--wait"], wait_for_running=False)
    assert list(wait_for_failure(cdsp)) == ["CaptureError"]


@pytest.mark.parametrize("rate", [44100, 96000, 192000])
def test_other_sample_rates(start_cdsp, alsa_config, feeder, rate):
    """The rate reaches hw params on both ends, and the measured rate agrees with it.

    The chunk size scales with the rate, so every case has the same chunk duration and
    the same headroom on a VM with no real time priority.
    """
    feeder(rate=rate)
    cdsp = start_cdsp(config=alsa_config(samplerate=rate, chunksize=1024 * rate // RATE))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.poll_until_true(
        "GetCaptureRate", lambda measured: abs(measured - rate) < 0.005 * rate, timeout=10.0
    )


@pytest.mark.parametrize("fmt", ["S16_LE", None])
def test_playback_to_snd_dummy(start_cdsp, alsa_config, feeder, fmt):
    """snd-dummy is a sink with its own clock, the shape of a real sound card.

    It only takes S16_LE, so with no format given the backend has to find that one.
    """
    feeder()
    cdsp = start_cdsp(config=alsa_config(playback_device=DUMMY, playback_format=fmt))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    # And it keeps going, rather than starting and falling over on the first period.
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


def test_snd_dummy_refuses_a_float_format(start_cdsp, alsa_config, feeder):
    feeder()
    config = alsa_config(playback_device=DUMMY, playback_format="F32_LE")
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    assert list(wait_for_failure(cdsp)) == ["PlaybackError"]
