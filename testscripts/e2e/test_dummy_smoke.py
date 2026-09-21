"""Smoke tests for the dummy backend.

Start the real binary on a dummy to dummy config, let it run for a few seconds, check
what it reports over the websocket, and shut it down. This is the cheapest end-to-end
signal there is: it covers process startup, the supervisor, the engine loop, both
devices, the pipeline and the whole control plane in one go.

Lifecycle and shutdown live in test_lifecycle.py; what is left here is the devices
themselves.
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


@pytest.mark.parametrize("samplerate", [44100, 48000, 96000, 192000])
def test_capture_rate_is_measured_at_every_rate(start_cdsp, config_file, samplerate):
    """The measured rate should follow the configured one, not just at 48 kHz."""
    cdsp = start_cdsp(config=config_file({"samplerate: 48000": f"samplerate: {samplerate}"}))
    # The first value published can come from a partial measurement window and read
    # low, so this polls for the rate to arrive rather than for it to be nonzero. A
    # rate that never gets there fails as a timeout naming the value it was stuck at.
    cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: rate == pytest.approx(samplerate, rel=0.05), timeout=8.0
    )


@pytest.mark.parametrize("chunksize", [128, 4096])
def test_other_chunksizes_run(start_cdsp, config_file, chunksize):
    """Chunk size changes the pacing granularity, so run the extremes of it."""
    cdsp = start_cdsp(config=config_file({"chunksize: 1024": f"chunksize: {chunksize}"}))
    cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: rate == pytest.approx(SAMPLERATE, rel=0.05), timeout=8.0
    )
    assert cdsp.send("GetState") == "Running"


def test_eight_channels_through_a_mixer(start_cdsp):
    """Two channels in, eight out, to prove the pipeline is not hardwired to stereo."""
    cdsp = start_cdsp(config="dummy_mixer.yml")
    assert cdsp.send("GetState") == "Running"
    levels = cdsp.poll_until_true(
        "GetPlaybackSignalPeak", lambda peaks: peaks and max(peaks) > -100.0
    )
    assert len(levels) == 8
