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

from .cable import CHANNELS, LEVEL_DB, RATE, assert_tone, record

pytestmark = pytest.mark.wasapi

EXIT_OK = 0
# F32_LE on stdout.
FRAME_BYTES = CHANNELS * 4


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    """Wait for both channels of a peak meter to read `level`, and return them."""
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.5 for peak in peaks),
        timeout=timeout,
    )


def capture_to_stdout(start_cdsp, config, seconds):
    """Run a config that captures into stdout, take `seconds` of it, and stop.

    Not waited on for Running before the read. The capture runs in real time, so until
    something drains the pipe the Stdout device blocks on a full one, the chain backs up
    to the capture, and the state never gets there. The read is the gate instead, and
    the state is checked once the audio is in.
    """
    cdsp = start_cdsp(config=config, pipe_stdout=True, wait_for_running=False)
    data = read_exactly(cdsp.process.stdout, int(seconds * RATE) * FRAME_BYTES)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.exit() == EXIT_OK
    return np.frombuffer(data, dtype="<f4").reshape(-1, CHANNELS)


def test_playback_shared(start_cdsp, win_config):
    """A generated tone played into the cable in shared mode comes out of it."""
    cdsp = start_cdsp(config=win_config(direction="playback"))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert_tone(record(2 * RATE))
    assert cdsp.send("GetStopReason") == "None"


def test_capture_shared(start_cdsp, win_config, feeder):
    """A tone played into the cable is what CamillaDSP captures from it in shared mode."""
    feeder()
    assert_tone(capture_to_stdout(start_cdsp, win_config(direction="capture"), 2.5))
