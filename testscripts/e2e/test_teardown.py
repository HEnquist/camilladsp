"""Stopping a run, in the states where stopping is hard.

`Exit` is already covered for its exit code, and while a device is stalled for its
promptness. What was not covered is the two places where the capture is busy handing
audio over and cannot look at its command channel: a queue that is full because the far
end is slower, and the tail of silence a file capture adds after its input ends.

Both are a question of latency rather than of outcome, so these tests measure how long
the exit took. The margins are wide on purpose. What they are meant to catch is a stop
that waits for the audio to finish rather than cutting it short, which is a difference
of seconds, not the tens of milliseconds a loaded runner adds.

From cdsp's `ImmediateAbort_PlaybackDrainingBug`, `NonRealtimeImmediateAbort_ExitsImmediately`,
`UserStopDuringEOFDrain_UnblocksPlayback` and `GracefulTeardown_Sequence`.
"""

import os
import sys
import time

import pytest

from conftest import SAMPLERATE, read_raw

EXIT_OK = 0

# Long enough that waiting it out is unmistakable in the numbers below, and short enough
# that a test which does wait it out still finishes inside the global timeout.
DRAIN_SECONDS = 8
# What counts as prompt. Measured at about 0.3 s on a laptop for every case here, so
# this is a wide margin that still fails loudly against a drain that is waited out.
PROMPT = 3.0


def extra_samples(seconds):
    """Config edit adding a tail of silence to the raw file capture."""
    # Keyed on the device type rather than the filename, because device_config has
    # already substituted the filename by the time these edits are applied.
    return {
        "    type: RawFile": f"    type: RawFile\n    extra_samples: {int(seconds * SAMPLERATE)}"
    }


def timed_exit(cdsp):
    """Ask CamillaDSP to exit, and return the exit code and how long it took."""
    started = time.monotonic()
    code = cdsp.exit(timeout=60)
    return code, time.monotonic() - started


def test_exit_during_the_extra_samples_is_prompt(start_cdsp, device_config):
    """A stop must cut the tail short rather than play it out.

    The capture hands the whole tail over in one go, and against a paced playback the
    queue is bounded, so every chunk of it blocks until the far end has room. Without a
    command check inside that loop the capture never gets back to the one at the top,
    and a user asking to stop waits for the entire tail first. This measured 7.05 s
    against an 8 s tail before `send_silence` learned to look.
    """
    config, _, _ = device_config(
        "file", "dummy", seconds=0.1, edits=extra_samples(DRAIN_SECONDS)
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    # The input is a tenth of a second, so by now the capture is certainly in the tail.
    time.sleep(1.0)

    code, elapsed = timed_exit(cdsp)
    assert code == EXIT_OK
    assert elapsed < PROMPT, f"exit took {elapsed:.2f} s of a {DRAIN_SECONDS} s tail"


def test_an_uninterrupted_tail_is_written_in_full(start_cdsp, device_config):
    """And cutting it short on request must not make it lossy when nobody asks.

    The pair to the test above, and the one that would fail if the command check in
    `send_silence` ever started swallowing something it should not. `extra_samples` is
    exact, so this is a frame count rather than a duration.
    """
    tail = SAMPLERATE // 2
    config, samples, destination = device_config(
        "file", "file", seconds=0.1, edits=extra_samples(0.5)
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    assert cdsp.poll_until_true("GetStopReason", lambda reason: reason != "None") == "Done"
    assert cdsp.exit() == EXIT_OK

    assert len(read_raw(destination)) == len(samples) + tail


def test_exit_with_a_full_queue_is_prompt(start_cdsp, device_config):
    """Stopping while the capture is blocked handing over a chunk.

    A free-running capture against a paced playback pins the queue full for the whole
    run, so the capture spends nearly all its time blocked in `send`. It still has to
    see the command within a chunk or two, which it does because the playback keeps
    draining while the teardown runs.
    """
    config, _, _ = device_config("file", "dummy", seconds=60.0)
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    cdsp.poll_until_true("GetBufferLevel", lambda level: level > 0)

    code, elapsed = timed_exit(cdsp)
    assert code == EXIT_OK
    assert elapsed < PROMPT, f"exit took {elapsed:.2f} s"


@pytest.mark.skipif(sys.platform == "win32", reason="needs /dev/zero and /dev/null")
def test_aborting_an_endless_non_realtime_run_is_immediate(start_cdsp, config_file):
    """A run with nothing pacing it and no end in sight still has to stop when asked.

    From cdsp's `NonRealtimeImmediateAbort_ExitsImmediately`. The difficulty is
    arranging for there to be anything to abort: file to file goes at a few hundred
    times real time, measured here at over two thousand, so any input small enough to
    generate is processed before a stop can be sent, and the test would pass on a run
    that had already finished.

    `/dev/zero` solves it by never ending, which turns the assertion from a measurement
    into a certainty: the job cannot have completed, so a process that exited was cut
    short. `/dev/null` on the far end keeps a run at that speed from filling the disk,
    which it would otherwise do at a few hundred megabytes a second.

    The measured capture rate is what says the run was really under way rather than
    stuck somewhere. It reads thousands of times nominal, so the bound is loose.
    """
    config = config_file(
        {"CAPTURE_FILE": "/dev/zero", "PLAYBACK_FILE": os.devnull},
        base="file_devices.yml",
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    # The rate is published once per update interval, so shorten it rather than hold
    # every core at full tilt for the default second.
    cdsp.send("SetUpdateInterval", 100)
    rate = cdsp.poll_until_true("GetCaptureRate", lambda value: value > 10 * SAMPLERATE)
    assert rate > 10 * SAMPLERATE
    assert cdsp.send("GetStopReason") == "None"

    code, elapsed = timed_exit(cdsp)
    assert code == EXIT_OK
    assert elapsed < PROMPT, f"exit took {elapsed:.2f} s"
