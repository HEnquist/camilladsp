"""Smoke tests for the dummy backend.

Start the real binary on a dummy to dummy config, let it run for a few seconds, check
what it reports over the websocket, and shut it down. This is the cheapest end-to-end
signal there is: it covers process startup, the supervisor, the engine loop, both
devices, the pipeline and the whole control plane in one go.
"""

import time

import pytest

SAMPLERATE = 48000
CHUNKSIZE = 1024
# Long enough for several rate measurement windows, short enough to stay cheap in CI.
RUN_SECONDS = 3.0


def test_runs_and_paces_audio(cdsp):
    """Audio should keep flowing at roughly the nominal rate for the whole run."""
    time.sleep(RUN_SECONDS)
    assert cdsp.send("GetState") == "Running"
    # The dummy devices derive their frame position from the clock, so a measured rate
    # this close to nominal means the pacing really did track real time.
    assert cdsp.send("GetCaptureRate") == pytest.approx(SAMPLERATE, rel=0.05)
    assert cdsp.is_running()


def test_status_getters(cdsp):
    """Every getter the dummy devices feed should answer with a plausible value."""
    # The status snapshot is only refreshed once per update interval, so it reads back
    # as zero for the first fraction of a second after the devices start.
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    assert cdsp.send("GetVersion")
    assert cdsp.send("GetStopReason") == "None"
    assert 0.0 <= cdsp.send("GetProcessingLoad") < 1.0
    # A sine at -6 dBFS swings between -0.5 and +0.5, so the range is 1.0.
    assert cdsp.send("GetSignalRange") == pytest.approx(1.0, abs=0.05)
    # The dummy playback device prefills to target_level, which defaults to chunksize.
    assert 0 < cdsp.send("GetBufferLevel") <= 4 * CHUNKSIZE
    assert cdsp.send("GetClippedSamples") == 0


def test_dummy_devices_are_supported(cdsp):
    """A build with the dummy-backend feature should advertise the devices."""
    playback_types, capture_types = cdsp.send("GetSupportedDeviceTypes")
    assert "Dummy" in playback_types
    assert "Dummy" in capture_types


def test_exit_is_clean(cdsp):
    """Exit should shut the process down with the clean exit code."""
    assert cdsp.exit() == 0


def test_stop_returns_to_inactive(start_cdsp):
    """With --wait, Stop should close the devices but leave the process alive.

    Without --wait the supervisor exits once Stop clears the active config, so the
    waiting behaviour only exists in this mode.
    """
    cdsp = start_cdsp(extra_args=["--wait"])
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    assert cdsp.is_running()
    assert cdsp.exit() == 0
