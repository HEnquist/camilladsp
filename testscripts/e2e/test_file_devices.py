"""The file devices, and what happens when a paced end meets a free-running one.

The dummy devices are paced and the file devices are not, so between them they give all
four combinations of a free-running and a paced end of the pipeline. That matrix is the
thing here: each combination has its own way of going wrong, and only one of the four
has ever been exercised by the rest of the suite.

The two interesting ones are the mixed pairs. A free-running capture feeding a paced
playback produces faster than the far end consumes, so the queue fills and stays full
and the capture blocks in `send` for the rest of the run. That is the only way to make
`queuelimit` do anything, which the suite validates as a config value and otherwise
never reaches. The other way round, a paced capture feeding a file, runs at the capture
clock with the queue empty.

The matrix comes from cdsp's `FileFile_Realtime_FF/FT/TF/TT`, the overflow case from its
`RealtimeQueueDrop_DataIntegrity`, and the startup failure from `StartupFailure_Abort`.

These tests need numpy, and the file device cases would run against a stock build too.
They are here rather than in a software backend suite of their own because three of the
four combinations need a dummy device at one end.
"""

import os
import time

import numpy as np
import pytest

from conftest import DUMMY_PLAYBACK, FILE_PLAYBACK, SAMPLERATE, SINE_FREQ, SINE_LEVEL_DB, read_raw

EXIT_OK = 0
EXIT_BAD_CONFIG = 101

CHUNKSIZE = 1024
# A -6 dBFS sine, so the peak is exact and the RMS is 3.01 dB below it.
EXPECTED_PEAK = 10 ** (SINE_LEVEL_DB / 20)
EXPECTED_RMS_DB = SINE_LEVEL_DB - 3.0103
TOLERANCE_DB = 0.1
# Enough tone for the spectrum below to place the peak within a couple of percent, and
# a quarter second of real time for a paced capture to produce it.
MIN_FRAMES = SAMPLERATE // 4

def wait_for_frames(path, frames, timeout=10.0):
    """Wait until a playback file holds at least `frames` frames, and return the wait.

    A paced capture writes in real time, so a test that stops as soon as the meters
    read the tone stops after a handful of chunks. Waiting on the file rather than
    sleeping a fixed time keeps that explicit and keeps the run no longer than it needs
    to be.
    """
    needed = frames * 2 * 8
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if os.path.exists(path) and os.path.getsize(path) >= needed:
            return
        time.sleep(0.02)
    got = os.path.getsize(path) if os.path.exists(path) else 0
    raise TimeoutError(f"{path} held {got // 16} frames after {timeout} s, wanted {frames}")


def assert_tone(written):
    """Assert a written file holds the tone that went in, by level and by frequency.

    Level alone would accept a constant, and frequency alone would accept a tone at the
    wrong gain, so both are checked. Neither depends on where the file starts, which is
    what makes this usable on output whose length depends on when the run was stopped.
    """
    assert len(written) >= MIN_FRAMES, f"only {len(written)} frames written"
    for channel in range(written.shape[1]):
        column = written[:, channel]
        assert np.abs(column).max() == pytest.approx(EXPECTED_PEAK, rel=1e-3)
        rms_db = 20 * np.log10(np.sqrt((column**2).mean()))
        assert rms_db == pytest.approx(EXPECTED_RMS_DB, abs=TOLERANCE_DB)
        spectrum = np.abs(np.fft.rfft(column * np.hanning(len(column))))
        peak_hz = np.fft.rfftfreq(len(column), 1 / SAMPLERATE)[spectrum.argmax()]
        assert peak_hz == pytest.approx(SINE_FREQ, rel=0.02)


def assert_playback_meters(cdsp):
    """Assert the dummy playback saw the tone, since it writes nothing to compare."""
    levels = cdsp.poll_until_true(
        "GetPlaybackSignalRms",
        lambda values: len(values) == 2
        and all(level == pytest.approx(EXPECTED_RMS_DB, abs=TOLERANCE_DB) for level in values),
    )
    assert len(levels) == 2


@pytest.mark.parametrize("capture", ["file", "dummy"])
@pytest.mark.parametrize("playback", ["file", "dummy"])
def test_the_audio_survives_every_pacing_combination(
    start_cdsp, device_config, capture, playback
):
    """All four pairings of a free-running and a paced end have to pass the tone through.

    What differs between them is which end sets the rate and whether the queue between
    them runs full or empty, and none of that should reach the audio. A combination that
    deadlocked would fail on the global timeout instead.
    """
    config, _, destination = device_config(capture, playback, seconds=2.0)
    # A file capture ends by itself, so there is nothing to wait for in the Running
    # state: a short input can be finished before the first poll. The audio is what is
    # waited on in that case.
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)

    if playback == "dummy":
        assert_playback_meters(cdsp)
    if destination is not None and capture == "dummy":
        wait_for_frames(destination, MIN_FRAMES)
    if capture == "file":
        # Runs out of input and stops on its own, which is the natural end.
        assert cdsp.poll_until_true("GetStopReason", lambda reason: reason != "None") == "Done"
    assert cdsp.exit() == EXIT_OK

    if destination is not None:
        assert_tone(read_raw(destination))


def test_a_file_to_file_run_is_bit_exact(start_cdsp, device_config, tmp_path):
    """With both ends free-running and an empty pipeline, nothing may change a sample.

    Stronger than the matrix test above and only possible in this corner: the input is a
    file, so there is something to compare against, and neither end resamples or
    converts, so the comparison can be exact rather than a level.
    """
    config, samples, destination = device_config("file", "file", seconds=1.0)
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    assert cdsp.poll_until_true("GetStopReason", lambda reason: reason != "None") == "Done"
    assert cdsp.exit() == EXIT_OK

    written = read_raw(destination)
    assert written.shape == samples.shape
    assert np.array_equal(written, samples)


def test_a_paced_capture_writes_a_file_that_matches_the_signal(
    start_cdsp, device_config, tmp_path
):
    """The loopback case: a real-time capture recorded to disk and checked afterwards.

    From cdsp's `ALSALoopbackSignalMatch`. The dummy playback discards its audio, so a
    file on the far end is the only way to see what a paced capture actually produced
    without a real device.
    """
    config, _, destination = device_config("dummy", "file")
    cdsp = start_cdsp(config=config)
    wait_for_frames(destination, MIN_FRAMES)
    assert cdsp.exit() == EXIT_OK
    assert_tone(read_raw(destination))


@pytest.mark.parametrize("queuelimit", [2, 16])
def test_a_free_running_capture_fills_the_queue(
    start_cdsp, device_config, tmp_path, queuelimit
):
    """`queuelimit` is validated as a config value and otherwise never reached.

    A file capture produces as fast as it can read, so against a paced playback the
    queue goes full immediately and stays there. The playback counts what is waiting
    behind the chunk it is writing, `src/alsa_backend/device.rs:662`, so the level it
    reports carries the queue, and a full queue of `queuelimit` chunks is a floor on it.

    A lower bound rather than a band, because the rest of the reading is the playback's
    own buffer and that does move around.
    """
    config, _, _ = device_config(
        "file", "dummy", seconds=30.0,
        edits={"  queuelimit: 4": f"  queuelimit: {queuelimit}"},
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    level = cdsp.poll_until_true(
        "GetBufferLevel", lambda value: value >= queuelimit * CHUNKSIZE
    )
    assert level >= queuelimit * CHUNKSIZE
    # The audio is unharmed by the backpressure, which is the other half of the claim.
    assert_playback_meters(cdsp)
    assert cdsp.exit() == EXIT_OK


def test_a_bigger_queue_holds_more(start_cdsp, device_config, tmp_path):
    """And the level has to follow `queuelimit`, not merely clear it.

    Otherwise the floors above would both pass on a playback that reported one large
    constant. The difference cancels the playback's own buffer, so the expected gap is
    just the extra chunks, and the tolerance is two of them.
    """
    levels = {}
    for queuelimit in (2, 16):
        config, _, _ = device_config(
            "file", "dummy", seconds=30.0,
            edits={"  queuelimit: 4": f"  queuelimit: {queuelimit}"},
        )
        cdsp = start_cdsp(config=config, extra_args=["--wait"])
        levels[queuelimit] = cdsp.poll_until_true(
            "GetBufferLevel", lambda value: value >= queuelimit * CHUNKSIZE
        )
        assert cdsp.exit() == EXIT_OK

    gap = levels[16] - levels[2]
    assert gap == pytest.approx(14 * CHUNKSIZE, abs=2 * CHUNKSIZE), f"levels were {levels}"


def test_a_playback_device_that_cannot_be_opened_reports_it(
    start_cdsp, device_config, tmp_path
):
    """A device can pass validation and still fail to open, and that is not a crash.

    From cdsp's `StartupFailure_Abort`. The suite covers a config rejected by
    validation; this is the other kind, where the config is fine and the machine says
    no. A playback file in a directory that does not exist does it, and the engine has
    to report it the same way it reports a device failing later on.
    """
    config, _, _ = device_config(
        "dummy", "file", edits={"PLAYBACK_FILE": str(tmp_path / "nodir" / "out.raw")}
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = cdsp.poll_until_true("GetStopReason", lambda value: value != "None")
    assert list(reason) == ["PlaybackError"]
    assert cdsp.poll_until("GetState", "Inactive")
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


def test_a_missing_capture_file_is_rejected_before_startup(spawn_cdsp, config_file, tmp_path):
    """The other half of the pair, and it fails earlier than it looks like it should.

    Validation opens the capture file rather than leaving it to the device, so a missing
    input never reaches the device layer at all: the process exits with EXIT_BAD_CONFIG
    and the websocket never comes up. Worth pinning down because it is the opposite of
    the playback case above, where the same mistake is a runtime error on a live engine.
    """
    config = config_file(
        {
            "CAPTURE_FILE": str(tmp_path / "nosuch.raw"),
            FILE_PLAYBACK: DUMMY_PLAYBACK,
        },
        base="file_devices.yml",
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=20) == EXIT_BAD_CONFIG
