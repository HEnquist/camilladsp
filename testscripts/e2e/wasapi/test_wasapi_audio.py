"""Audio through the WASAPI backend on VB-Cable, one way at a time.

Everything else in the suite reaches the engine through the Dummy or the software
devices, so this is the only place the WASAPI capture and playback code runs at all: the
endpoint lookup, the format negotiation in shared and exclusive mode, the event driven
loops and the buffers between them and the engine.

Nothing through the cable is bit exact, so every test asserts the level and the
frequency of a tone, see cable.py.
"""

import numpy as np
import pytest
from swdevices import read_exactly

from .cable import (
    CHANNELS,
    LEVEL_DB,
    RATE,
    assert_tone,
    capture_endpoint,
    record,
    render_endpoint,
    wasapi_block,
)

pytestmark = pytest.mark.wasapi

EXIT_OK = 0
# F32_LE on stdout.
FRAME_BYTES = CHANNELS * 4

# Shared mode is always the mix format, which is float. Exclusive mode on the cable has
# only the integer formats.
MODES = [
    pytest.param(False, None, id="shared"),
    pytest.param(False, "F32", id="shared-F32"),
    pytest.param(True, "S16", id="exclusive-S16"),
    pytest.param(True, "S24", id="exclusive-S24"),
]


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    """Wait for both channels of a peak meter to read `level`, and return them."""
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.5 for peak in peaks),
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


def capture_to_stdout(start_cdsp, config, seconds, rate=RATE):
    """Run a config that captures into stdout, take `seconds` of it, and stop.

    Not waited on for Running before the read. The capture runs in real time, so until
    something drains the pipe the Stdout device blocks on a full one, the chain backs up
    to the capture, and the state never gets there. The read is the gate instead, and
    the state is checked once the audio is in.
    """
    cdsp = start_cdsp(
        config=config, extra_args=["-v"], pipe_stdout=True, wait_for_running=False
    )
    data = read_exactly(cdsp.process.stdout, int(seconds * rate) * FRAME_BYTES)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.exit() == EXIT_OK
    return np.frombuffer(data, dtype="<f4").reshape(-1, CHANNELS)


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
    reason = wait_for_failure(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


def test_loopback_capture(start_cdsp, win_config, feeder):
    """Capturing the render endpoint itself, in loopback, gets what is played into it.

    The one capture path that opens a render endpoint, and no other suite reaches it.
    """
    feeder()
    loopback = wasapi_block("capture", render_endpoint()[1], extra={"loopback": True})
    config = win_config(direction="capture", capture=loopback)
    assert_tone(capture_to_stdout(start_cdsp, config, 2.5))
