"""Stopping a run, in the states where stopping is hard.

`Exit` is already covered for its exit code, and while a device is stalled for its
promptness. What was not covered is the two places where the capture is busy handing
audio over and cannot look at its command channel: a queue that is full because the far
end is slower, and the tail of silence a file capture adds after its input ends.

Both are a question of latency rather than of outcome, so these tests measure how long
the exit took. The margins are wide on purpose. What they are meant to catch is a stop
that waits for the audio to finish rather than cutting it short, which is a difference
of seconds, not the tens of milliseconds a loaded runner adds.

From cdsp's `ImmediateAbort_PlaybackDrainingBug`, `UserStopDuringEOFDrain_UnblocksPlayback`
and `GracefulTeardown_Sequence`.

Its fourth teardown scenario, `NonRealtimeImmediateAbort_ExitsImmediately`, is not here
because the state it describes cannot be reached. A file to file run goes at a few
hundred times real time, measured at 60 s of audio in 0.11 s and still only 0.28 s
through a 65536 tap FIR, so the job is finished before a stop can be sent and there is
nothing to abort. Making it last long enough would take an input of several hundred
megabytes. The two paths where a stop really can be delayed are the ones below, and
both involve a capture blocked handing audio to a paced far end.
"""

import time

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
