"""Audio through the CoreAudio backend on both ends: formats, rates, and what comes out.

Everything else in the suite reaches the engine through the Dummy or the software
devices, so this is the only place the CoreAudio capture and playback code runs at all:
the AudioUnit setup, the physical format and rate negotiation, the ring buffers between
the callbacks and the device threads.

CamillaDSP always talks float32 to CoreAudio and BlackHole's only physical format is
float32, so with an empty pipeline what CamillaDSP plays should be exactly what the
feeder played. That is a stronger check than a level, since it catches a channel
swapped or a frame repeated at a callback boundary, neither of which moves a peak meter.
"""

import time

import pytest

from .blackhole import (
    FEED_DEVICE,
    NOMINAL_RATE,
    SINK_DEVICE,
    exact_fraction,
    noise,
    nominal_rate,
    record,
    set_nominal_rate,
    wait_for_nominal_rate,
)

pytestmark = pytest.mark.coreaudio

EXIT_OK = 0
RATE = NOMINAL_RATE
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


@pytest.mark.parametrize("fmt", [None, "F32"])
def test_audio_passes_through_bit_exact(start_cdsp, ca_config, feeder, fmt):
    """A second of noise in a loop, and the output has to match it exactly.

    With no format the backend leaves the physical format alone, with F32 it looks the
    format up and sets it, and both end up at float32 on a float32 device.

    Matched in 50 ms pieces rather than as one stretch, see `exact_fraction`: a dropout
    on a busy runner is a gap between pieces, not a wrong sample, and is not what this
    is about. A wrong conversion fails every piece, so most of them matching is the
    whole claim.
    """
    block = noise(RATE)
    feeder(block=block)
    start_cdsp(config=ca_config(capture_format=fmt, playback_format=fmt))
    recorded = record(RATE * 2)
    # The last second, well clear of whatever the start of the recording caught.
    window = recorded[-RATE:]
    assert exact_fraction(block, window, RATE // 20) >= 0.75


@pytest.mark.parametrize("fmt", ["S16", "S24", "S32"])
@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_format_the_device_lacks_is_refused(start_cdsp, ca_config, feeder, side, fmt):
    """BlackHole is float only, so an integer physical format has to fail to open.

    Validation cannot catch this, the config is fine. It is the device's format list
    saying no, and the engine has to report it against the right side and keep running
    under --wait.
    """
    feeder()
    config = ca_config(**{f"{side}_format": fmt})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_failure(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


def test_the_devices_are_switched_to_the_config_rate(start_cdsp, ca_config):
    """Both devices start at another rate, and opening them has to set the config's.

    The rate is the one device setting CamillaDSP always writes, format or not, and
    nothing else in the suite can see whether it reached the device.
    """
    for device in (FEED_DEVICE, SINK_DEVICE):
        set_nominal_rate(device, 44100)
        wait_for_nominal_rate(device, 44100)
    cdsp = start_cdsp(config=ca_config())
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0, timeout=10.0)
    assert nominal_rate(FEED_DEVICE) == RATE
    assert nominal_rate(SINK_DEVICE) == RATE


@pytest.mark.parametrize("rate", [44100, 96000, 192000])
def test_other_sample_rates(start_cdsp, ca_config, feeder, rate):
    """The rate reaches both devices, and the measured rate agrees with it.

    The devices are put at the rate first so the feeder opens at the device's own rate,
    see `Feeder`. The chunk size scales with the rate, so every case has the same chunk
    duration and the same headroom.
    """
    for device in (FEED_DEVICE, SINK_DEVICE):
        set_nominal_rate(device, rate)
        wait_for_nominal_rate(device, rate)
    feeder(rate=rate)
    cdsp = start_cdsp(config=ca_config(samplerate=rate, chunksize=1024 * rate // RATE))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.poll_until_true(
        "GetCaptureRate", lambda measured: abs(measured - rate) < 0.005 * rate, timeout=10.0
    )


def test_the_audio_keeps_flowing(start_cdsp, ca_config, feeder):
    """Started and still going, rather than falling over after the first few callbacks."""
    feeder()
    cdsp = start_cdsp(config=ca_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    time.sleep(3.0)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


def test_a_silent_source_pauses_and_resumes(start_cdsp, ca_config, feeder):
    """With nothing playing into it BlackHole delivers zeros, which is silence.

    The silence detection is in each backend, next to the source, so that a paused
    session skips the resampler too, and this is the CoreAudio copy of it. A feeder
    starting again has to bring the session back without anything being restarted.
    """
    config = ca_config(devices={"silence_threshold": -60.0, "silence_timeout_s": 0.5})
    cdsp = start_cdsp(config=config, wait_for_running=False)
    cdsp.poll_until("GetState", "Paused", timeout=10.0)
    feeder()
    cdsp.poll_until("GetState", "Running", timeout=10.0)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
