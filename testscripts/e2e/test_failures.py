"""Device failures, format changes, and the end of a stream.

`StopReason` has seven variants and until now the suite only ever saw `None`. Everything
that sets one of the others lives in the supervisor at `src/engine.rs`, in the
`fail_and_restart` arms and the `PlaybackDone` arm, and none of that code had any
end-to-end coverage: a device has to fail, change rate, or run out of audio for it to
run at all, and no test device could be made to do any of those.

The `error`, `rate` and `eof` keys on the dummy control socket are what make them
reachable. Each one stands for a different thing a real device does:

- `error` is the device reporting a failure, an ALSA write returning an error or a
  CoreAudio device disappearing.
- `rate` is the source changing sample rate underneath the engine, which is the scenario
  issue #531 singles out. On the capture the engine has to notice by measuring, the way
  the real backends do; on the playback it is simply told, since that is how the driver
  notification at `src/coreaudio_backend/device.rs:713` arrives.
- `eof` is a stream that ends, the way a file capture reaches the end of its file.

The shape shared by all of them is worth knowing before reading the tests. A fatal device
event clears the active config, `src/engine.rs:84`, so the engine does not come back on
its own: under `--wait` it drops to `Inactive` and stands by, and without `--wait` the
supervisor finds no config and no queued command and exits. That is what makes the stop
reason observable at all. Were the session restarted automatically, the reason would be
reset by the next `CaptureReady`, `src/engine.rs:212`, a few milliseconds later, and
there would be nothing to poll for.

One scenario here comes from cdsp's `Repro_StopReason_None_Overwriting_Done`.
"""

import time

import pytest

EXIT_OK = 0

NOMINAL_RATE = 48000
# A rate far enough from the configured 48000 to clear the 4 % threshold in
# RATE_CHANGE_THRESHOLD_VALUE, and a rate comfortably inside it.
CHANGED_RATE = 44100
NUDGED_RATE = 48500
# The highest rate the watcher can possibly fire on, from how RATE_CHANGE_THRESHOLD_VALUE
# builds its band in `ValueWatcher::new`.
DETECTION_EDGE = NOMINAL_RATE / 1.04

# The engine needs four consecutive measurement windows outside the threshold before it
# calls it a rate change, see RATE_CHANGE_THRESHOLD_COUNT, so a short window is what
# keeps these tests to about a second rather than four.
MEASURE_SETTINGS = {
    "  chunksize: 1024\n": "  chunksize: 1024\n  rate_measure_interval_s: 0.2\n",
}
STOPPING_MEASURE_SETTINGS = {
    "  chunksize: 1024\n": (
        "  chunksize: 1024\n  rate_measure_interval_s: 0.2\n  stop_on_rate_change: true\n"
    ),
}


def control_for(cdsp, device):
    """The control socket of whichever dummy device the test is about."""
    return cdsp.capture_control if device == "capture" else cdsp.playback_control


def wait_for_standby(cdsp):
    """Wait for a fatal device event to have taken the engine down, and return the reason.

    The reason is what is polled for, because it is what the tests assert on. `Inactive`
    looks like the better gate and is not: it is set twice, once by the device thread as
    its loop ends and again by the supervisor at the end of the teardown,
    `src/engine.rs:84`. The first of those lands before the supervisor has even read the
    device's status message, so a test gated on the state reads the stop reason from the
    session before last. Same rule as everywhere else in this suite, poll the thing the
    command under test actually reads.
    """
    reason = cdsp.poll_until_true("GetStopReason", lambda value: value != "None")
    cdsp.poll_until("GetState", "Inactive")
    return reason


@pytest.mark.parametrize("device", ["capture", "playback"])
def test_a_device_failure_sets_the_stop_reason(control_cdsp, device):
    """A device that reports a failure should name itself in the stop reason."""
    cdsp = control_cdsp(extra_args=["--wait"])
    control_for(cdsp, device).set("error", 1)
    reason = wait_for_standby(cdsp)
    expected = "CaptureError" if device == "capture" else "PlaybackError"
    assert reason == {expected: f"Dummy {device} device failed on request"}


def test_a_failed_device_leaves_the_engine_standing_by(control_cdsp):
    """The process has to survive the failure, or there is nothing left to recover.

    Under `--wait` a fatal device event is not fatal to CamillaDSP: the devices close,
    the config is cleared and the control plane stays up, which is what lets a GUI show
    the error and send a new config.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.capture_control.set("error", 1)
    wait_for_standby(cdsp)
    assert cdsp.is_running()
    # The websocket is still answering, not merely the process still resident.
    assert cdsp.send("GetVersion")
    # Cleared by the supervisor after the devices are joined, so it trails the reason.
    assert cdsp.poll_until_true("GetConfig", lambda text: text.strip() == "null")
    assert cdsp.exit() == EXIT_OK


def test_a_reload_restarts_the_devices_after_a_failure(control_cdsp):
    """Recovery has to build genuinely new devices, not resume the failed ones.

    The frame counter is what proves it. It belongs to the device, so a count that has
    gone back to nearly nothing is a device that was created again, and the session
    coming back up at all is the failure flag having gone with the old one. Were the
    control state shared across a restart instead, this would fail immediately on the
    second failure.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    before = cdsp.capture_control.get_int("frames")
    assert before > 0

    cdsp.capture_control.set("error", 1)
    wait_for_standby(cdsp)

    # The path came from the command line and survives the failure, so this is all it
    # takes to start again on the same config and the same control ports.
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    assert cdsp.send("GetStopReason") == "None"
    cdsp.capture_control.wait_until_ready()
    assert cdsp.capture_control.get_int("frames") < before

    # And the recovered session is a working one, not just a state.
    assert cdsp.capture_control.get_int("error") == 0
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)


def test_a_device_failure_without_wait_ends_the_process(control_cdsp):
    """Without `--wait` there is no config left to run and nothing to wait for.

    Worth pinning down because the exit code is not the one it looks like it should be.
    A device failure is a clean shutdown as far as the supervisor is concerned, so this
    is EXIT_OK and not EXIT_PROCESSING_ERROR: the latter is only reached when `run`
    itself returns an error, `src/engine.rs:466`, which a device cannot cause.
    """
    cdsp = control_cdsp()
    cdsp.capture_control.set("error", 1)
    assert cdsp.process.wait(timeout=20) == EXIT_OK


def test_a_stalled_capture_can_still_be_failed(control_cdsp):
    """A device already in trouble must still be able to report a hard failure.

    The stall path hands over nothing and loops, so a failure check placed after it
    would never run and a stalled device could never fail. That ordering is easy to get
    wrong and invisible until a real device stalls and then dies.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.capture_control.set("stall", 1)
    cdsp.poll_until("GetState", "Stalled")
    cdsp.capture_control.set("error", 1)
    assert wait_for_standby(cdsp) == {
        "CaptureError": "Dummy capture device failed on request"
    }


@pytest.mark.pacing
def test_a_capture_rate_change_stops_processing(control_cdsp):
    """The engine has to notice the source changed rate, and say what it changed to.

    Nothing tells CamillaDSP what a capture device is doing, it counts frames against
    the clock, so the rate in the reason is a measurement. Which measurement it is is
    the part worth knowing: it comes from the window the detection tripped in, and the
    switch lands partway through a window, so the first windows after it read a mix of
    the two rates. The reported value is therefore somewhere between the new rate and
    the edge of the detection band, not on the new rate, and asserting a tight
    tolerance around 44100 fails perhaps one run in three.

    So the bound is the band itself rather than a tolerance. Above the edge is
    impossible by construction, since the watcher only counts values outside it, and
    below the new rate would mean the device produced fewer frames than its clock
    allows. Between those two is every honest answer and no garbage one.
    """
    cdsp = control_cdsp(replacements=STOPPING_MEASURE_SETTINGS, extra_args=["--wait"])
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    cdsp.capture_control.set("rate", CHANGED_RATE)
    reason = wait_for_standby(cdsp)
    assert list(reason) == ["CaptureFormatChange"]
    reported = reason["CaptureFormatChange"]
    assert CHANGED_RATE * 0.98 <= reported < DETECTION_EDGE, (
        f"reported {reported} Hz, expected between {CHANGED_RATE} and {DETECTION_EDGE:.0f}"
    )


@pytest.mark.pacing
def test_a_capture_rate_change_is_ignored_when_not_asked_to_stop(control_cdsp):
    """`stop_on_rate_change` defaults to off, and off has to mean off.

    The measured rate still follows the source, so the detection ran and the decision
    not to act on it is what is under test here, rather than the change going unnoticed.
    """
    cdsp = control_cdsp(replacements=MEASURE_SETTINGS, extra_args=["--wait"])
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    cdsp.capture_control.set("rate", CHANGED_RATE)
    measured = cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: rate == pytest.approx(CHANGED_RATE, rel=0.02)
    )
    assert measured == pytest.approx(CHANGED_RATE, rel=0.02)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


@pytest.mark.pacing
def test_a_small_rate_change_is_below_the_threshold(control_cdsp):
    """A source a fraction off nominal is not a source that changed rate.

    Every real capture clock is a little off, so a detector that fired on any difference
    would restart the engine continuously. RATE_CHANGE_THRESHOLD_VALUE is what stops it,
    and this is the test that the threshold is real rather than nominally configured.
    """
    cdsp = control_cdsp(replacements=STOPPING_MEASURE_SETTINGS, extra_args=["--wait"])
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    cdsp.capture_control.set("rate", NUDGED_RATE)
    # Well past the four windows a change needs, so a detector that was going to fire
    # has had every chance to.
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


def test_a_playback_rate_change_stops_processing(control_cdsp):
    """A playback device is told its rate changed rather than measuring it.

    So unlike the capture case this reports the new rate exactly, and needs no timing
    tolerance and no measurement window.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.playback_control.set("rate", CHANGED_RATE)
    assert wait_for_standby(cdsp) == {"PlaybackFormatChange": CHANGED_RATE}


def test_a_stream_that_ends_sets_done(control_cdsp):
    """A capture that runs out of audio is a normal end, not a failure.

    `Done` is set on the playback finishing, `src/engine.rs:238`, so reaching it proves
    the end of stream travelled the whole chain rather than the capture simply stopping.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.capture_control.set("eof", 1)
    assert wait_for_standby(cdsp) == "Done"


def test_nothing_overwrites_a_natural_end(control_cdsp):
    """`Done` has to survive everything that runs after it during the teardown.

    From cdsp's `Repro_StopReason_None_Overwriting_Done`. The devices close after the
    reason is set, and a device that reported on its way out could replace it, leaving a
    session that finished normally claiming an error. The guard is the `StopReason::None`
    check at `src/engine.rs:235`, and what tests it is that the reason is still `Done`
    once everything has settled rather than only at the moment it lands.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.capture_control.set("eof", 1)
    assert wait_for_standby(cdsp) == "Done"
    time.sleep(1.0)
    assert cdsp.send("GetStopReason") == "Done"
    assert cdsp.send("GetState") == "Inactive"


def test_a_new_session_clears_the_stop_reason(control_cdsp):
    """The reason describes the last session, so a new one has to start clean.

    Otherwise a GUI shows the previous failure over a running engine, with nothing on
    the engine side ever putting it right.

    The reset happens when the second of the two devices reports ready, and which one
    that is is not guaranteed anywhere: `start_pipeline` starts the playback first and
    the dummy playback has less to do before it reports, so in this suite the capture is
    always second. The cycle repeats to give the other order a chance on a loaded
    runner, but it is not a reliable way to reach it. See the note in the plan about the
    startup delay a device would need for that to be testable properly.
    """
    cdsp = control_cdsp(extra_args=["--wait"])
    for _ in range(3):
        cdsp.capture_control.set("eof", 1)
        assert wait_for_standby(cdsp) == "Done"
        cdsp.send("Reload")
        # Running is set by the capture device and the reason is cleared by the
        # supervisor, so the reason is what this waits on. A read straight after the
        # state would be the race rather than a test of it.
        cdsp.poll_until("GetStopReason", "None")
        cdsp.poll_until("GetState", "Running")
        cdsp.capture_control.wait_until_ready()
