"""Audio through the WASAPI backend on VB-Cable, one way at a time.

Everything else in the suite reaches the engine through the Dummy or the software
devices, so this is the only place the WASAPI capture and playback code runs at all: the
endpoint lookup, the format negotiation in shared and exclusive mode, the event driven
loops and the buffers between them and the engine.

Nothing through the cable is bit exact, so every test asserts the level and the
frequency of a tone, see cable.py.

Loopback capture is not here. A loopback of either render endpoint of the cable gets
only zeros on the runner, through the soundcard package as well as through CamillaDSP,
so it says nothing about the backend.
"""

import pytest

from .cable import (
    EXIT_OK,
    RATE,
    assert_tone,
    capture_endpoint,
    capture_to_stdout,
    record,
    render_endpoint,
    wait_for_peak,
    wait_for_stop,
    wasapi_block,
)

pytestmark = pytest.mark.wasapi

# Shared mode is always the mix format, which is float. Exclusive mode on the cable has
# only the integer formats.
MODES = [
    pytest.param(False, None, id="shared"),
    pytest.param(False, "F32", id="shared-F32"),
    pytest.param(True, "S16", id="exclusive-S16"),
    pytest.param(True, "S24", id="exclusive-S24"),
]


def playback(exclusive=False, fmt=None, extra=None):
    return wasapi_block("playback", render_endpoint()[1], exclusive, fmt, extra)


def capture(exclusive=False, fmt=None, extra=None):
    return wasapi_block("capture", capture_endpoint()[1], exclusive, fmt, extra)


@pytest.mark.parametrize("exclusive,fmt", MODES)
def test_playback(start_cdsp, win_config, exclusive, fmt):
    """A generated tone played into the cable comes out of it."""
    config = win_config(direction="playback", playback=playback(exclusive, fmt))
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert_tone(record(2 * RATE))
    assert cdsp.send("GetStopReason") == "None"


@pytest.mark.parametrize("exclusive,fmt", MODES)
def test_capture(start_cdsp, win_config, feeder, exclusive, fmt):
    """A tone played into the cable is what CamillaDSP captures from it."""
    feeder()
    config = win_config(direction="capture", capture=capture(exclusive, fmt))
    assert_tone(capture_to_stdout(start_cdsp, config, 2.5))


@pytest.mark.parametrize("rate", [44100, 96000, 192000])
def test_exclusive_playback_at_other_rates(start_cdsp, win_config, rate):
    """The stream opens at the config's rate, and the tone arrives at its frequency.

    The cable does not follow the rate an exclusive client sets, it keeps its own and
    converts, so the recorder stays at 48 kHz and only the frequency says anything. The
    chunk size scales with the rate, so every case has the same chunk duration.
    """
    config = win_config(
        direction="playback",
        playback=playback(True, "S16"),
        samplerate=rate,
        chunksize=1024 * rate // RATE,
    )
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert_tone(record(2 * RATE), level_db=None)


@pytest.mark.parametrize("rate", [44100, 96000, 192000])
def test_exclusive_capture_at_other_rates(start_cdsp, win_config, feeder, rate):
    """Captured at the config's rate, the feeder's tone keeps its frequency."""
    feeder()
    config = win_config(
        direction="capture",
        capture=capture(True, "S16"),
        samplerate=rate,
        chunksize=1024 * rate // RATE,
    )
    assert_tone(capture_to_stdout(start_cdsp, config, 2.5, rate), rate=rate, level_db=None)


@pytest.mark.parametrize("fmt", ["F32", "S32"])
@pytest.mark.parametrize("side", ["capture", "playback"])
def test_an_exclusive_format_the_device_lacks_is_refused(start_cdsp, win_config, side, fmt):
    """The cable has only S16 and S24 in exclusive mode, so these have to fail to open.

    Validation cannot catch this, the config is fine. It is the device saying no, and
    the engine has to report it against the right side and keep running under --wait.
    """
    if side == "capture":
        config = win_config(direction="capture", capture=capture(True, fmt))
    else:
        config = win_config(direction="playback", playback=playback(True, fmt))
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK

