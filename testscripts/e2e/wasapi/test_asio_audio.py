"""Audio through the ASIO backend, on the Steinberg built-in ASIO Driver over VB-Cable.

The driver opens the Windows default devices, which on the runner are the cable's, so the
same one way runs as the WASAPI tests work through ASIO. It is float only, so this covers
the backend's F32 path and its per channel (non interleaved) buffers. The integer sample
types go through the same shared converters as every other backend, which have unit tests
and run bit exact in the software backend suite, so nothing is lost by the driver lacking
them.
"""

import pytest

from .cable import (
    EXIT_OK,
    RATE,
    asio_block,
    assert_tone,
    capture_to_stdout,
    record,
    wait_for_peak,
    wait_for_stop,
)

pytestmark = [pytest.mark.wasapi, pytest.mark.usefixtures("steinberg")]

# With no format the backend takes the driver's own, which is F32_LE.
FORMATS = [pytest.param(None, id="native"), pytest.param("F32_LE", id="F32_LE")]


@pytest.mark.parametrize("fmt", FORMATS)
def test_playback(start_cdsp, win_config, fmt):
    """A generated tone played through the driver comes out of the cable."""
    config = win_config(direction="playback", playback=asio_block("playback", fmt=fmt))
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert_tone(record(2 * RATE))
    assert cdsp.send("GetStopReason") == "None"


@pytest.mark.parametrize("fmt", FORMATS)
def test_capture(start_cdsp, win_config, feeder, fmt):
    """A tone played into the cable is what CamillaDSP captures through the driver."""
    feeder()
    config = win_config(direction="capture", capture=asio_block("capture", fmt=fmt))
    assert_tone(capture_to_stdout(start_cdsp, config, 2.5))


@pytest.mark.parametrize("fmt", ["S16_LE", "S24_3_LE", "S24_4_LE", "S32_LE"])
@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_format_the_driver_lacks_is_refused(start_cdsp, win_config, side, fmt):
    """The driver is float only, so an integer format has to fail to open, against the
    right side, with the process standing by under --wait."""
    config = win_config(direction=side, **{side: asio_block(side, fmt=fmt)})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK
