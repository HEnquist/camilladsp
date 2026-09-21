"""Stalled and paused processing, driven from the dummy control socket.

`ProcessingState` has no central watchdog: every backend decides for itself when it is
stalled, and runs its own silence counter. These tests drive both from the outside, by
telling the dummy capture to stop producing or to go quiet while the engine runs.

Several scenarios here come from cdsp's `tests/test_dsp_engine.c`, marked where they do.
"""

import time

import pytest

# The base config has no silence detection, since most tests should not have any. These
# settings switch it on for the ones that do.
SILENCE_SETTINGS = {
    "  chunksize: 1024\n": "  chunksize: 1024\n  silence_threshold: -60.0\n  silence_timeout_s: 0.5\n",
}
SILENCE_TIMEOUT = 0.5
# The playback RMS of the base config, a -6 dBFS sine through a -6 dB gain filter, and
# what it becomes once that filter is changed to -12 dB.
PLAYBACK_RMS_DB = -15.01
PATCHED_RMS_DB = -21.01
TOLERANCE = 0.2


def wait_for_playback_rms(cdsp, expected, timeout=5.0):
    """Wait for the playback meters to settle at `expected`.

    Polling for the value rather than for "something above the noise floor" is what makes
    this sharp. The meters climb back through everything in between as the first chunks of
    signal arrive after a pause, so a predicate that accepts any real number reads one of
    those. A level that never arrives then fails as a timeout naming where it got stuck.
    """
    return cdsp.poll_until_true(
        "GetPlaybackSignalRms",
        lambda values: len(values) == 2
        and all(level == pytest.approx(expected, abs=TOLERANCE) for level in values),
        timeout=timeout,
    )


def test_a_stalled_capture_reaches_the_stalled_state(control_cdsp):
    """A device that stops producing should report it, and recover when it resumes."""
    cdsp = control_cdsp()
    cdsp.capture_control.set("stall", 1)
    cdsp.poll_until("GetState", "Stalled")
    cdsp.capture_control.set("stall", 0)
    cdsp.poll_until("GetState", "Running")


def test_a_stall_sends_pause_messages_down_the_chain(control_cdsp):
    """The state alone proves nothing: the pause has to reach the playback device."""
    cdsp = control_cdsp()
    before = cdsp.playback_control.get_int("pauses")
    cdsp.capture_control.set("stall", 1)
    cdsp.poll_until("GetState", "Stalled")
    deadline = time.monotonic() + 5.0
    while cdsp.playback_control.get_int("pauses") <= before:
        assert time.monotonic() < deadline, "no pause message reached the playback device"
        time.sleep(0.02)
    # The capture counts what it sent, the playback what it received.
    assert cdsp.capture_control.get_int("pauses") > 0


def test_a_stalled_capture_produces_nothing(control_cdsp):
    """Frames stop while stalled, and start again from where they left off."""
    cdsp = control_cdsp()
    cdsp.capture_control.set("stall", 1)
    cdsp.poll_until("GetState", "Stalled")
    # Read after the state changed, so the chunk in flight is already counted.
    stopped_at = cdsp.capture_control.get_int("frames")
    time.sleep(0.5)
    assert cdsp.capture_control.get_int("frames") == stopped_at
    cdsp.capture_control.set("stall", 0)
    cdsp.poll_until("GetState", "Running")
    time.sleep(0.5)
    resumed = cdsp.capture_control.get_int("frames") - stopped_at
    # Anchoring the pacer to the clock during the stall is what keeps this from being a
    # burst of everything the device missed.
    assert 0 < resumed < 48000


def test_exit_while_stalled_is_prompt(control_cdsp):
    """Exiting must not wait on a device that will never move again.

    Scenario from cdsp's DSPEngineE2E_ImmediateAbort_PlaybackDrainingBug.
    """
    cdsp = control_cdsp()
    cdsp.capture_control.set("stall", 1)
    cdsp.poll_until("GetState", "Stalled")
    started = time.monotonic()
    assert cdsp.exit() == 0
    assert time.monotonic() - started < 5.0


def test_a_stalled_playback_pins_its_buffer_level(control_cdsp):
    """A stalled playback keeps taking chunks and throws them away, as a real one does.

    So the queue does not back up behind it, the capture keeps running, and the buffer
    level stops being updated rather than reporting a level nothing is draining.
    """
    cdsp = control_cdsp()
    cdsp.poll_until_true("GetBufferLevel", lambda level: level > 0)
    cdsp.playback_control.set("stall", 1)
    time.sleep(0.2)
    pinned = cdsp.send("GetBufferLevel")
    consumed = cdsp.playback_control.get_int("frames")
    time.sleep(0.5)
    assert cdsp.send("GetBufferLevel") == pinned
    # Still consuming, and the capture never noticed.
    assert cdsp.playback_control.get_int("frames") > consumed
    assert cdsp.send("GetState") == "Running"
    started = time.monotonic()
    assert cdsp.exit() == 0
    assert time.monotonic() - started < 5.0


def test_silence_pauses_processing_after_the_timeout(control_cdsp):
    """The silence counter should wait out its timeout before pausing, and not longer."""
    cdsp = control_cdsp(replacements=SILENCE_SETTINGS)
    cdsp.capture_control.set("silence", 1)
    started = time.monotonic()
    # Nothing should have happened yet at half the timeout.
    time.sleep(SILENCE_TIMEOUT / 2)
    assert cdsp.send("GetState") == "Running"
    cdsp.poll_until("GetState", "Paused")
    assert time.monotonic() - started < 2 * SILENCE_TIMEOUT


def test_signal_returning_resumes_processing(control_cdsp):
    """And the pause has to end as soon as there is signal again."""
    cdsp = control_cdsp(replacements=SILENCE_SETTINGS)
    cdsp.capture_control.set("silence", 1)
    cdsp.poll_until("GetState", "Paused")
    cdsp.capture_control.set("silence", 0)
    cdsp.poll_until("GetState", "Running")
    # The signal that comes back is the one the config asks for.
    wait_for_playback_rms(cdsp, PLAYBACK_RMS_DB)


def test_silence_without_a_threshold_keeps_running(control_cdsp):
    """Silence detection is off by default, so a quiet capture is just a quiet capture."""
    cdsp = control_cdsp()
    cdsp.capture_control.set("silence", 1)
    time.sleep(2 * SILENCE_TIMEOUT)
    assert cdsp.send("GetState") == "Running"
    # The audio really is silent, the engine simply does not act on it.
    assert cdsp.poll_until_true(
        "GetPlaybackSignalRms",
        lambda values: len(values) == 2 and all(level < -100.0 for level in values),
        timeout=5.0,
    )


def test_a_pipeline_swap_while_paused_takes_effect(control_cdsp, config_file):
    """A config change applied while paused must be what runs when the signal returns.

    Every other reload test starts from Running. Scenario from cdsp's
    DSPEngineE2E_PausedState_PipelineSwap_Delay_Vulnerability.
    """
    cdsp = control_cdsp(replacements=SILENCE_SETTINGS)
    cdsp.capture_control.set("silence", 1)
    cdsp.poll_until("GetState", "Paused")

    # Same ports as the running config, so this is a filter change and not a device
    # change, which would restart the devices and end the pause on its own.
    patched = {**cdsp.control_edits, **SILENCE_SETTINGS}
    patched["gain: -6.0"] = "gain: -12.0"
    patched['description: "nbr 1"'] = 'description: "nbr 2"'
    with open(config_file(patched, base="dummy_control.yml")) as conf:
        cdsp.send("SetConfig", conf.read())
    cdsp.poll_until_true("GetConfigJson", lambda text: '"nbr 2"' in text, timeout=5.0)
    # Still paused: applying a config is not a reason to resume.
    assert cdsp.send("GetState") == "Paused"

    cdsp.capture_control.set("silence", 0)
    cdsp.poll_until("GetState", "Running")
    wait_for_playback_rms(cdsp, PATCHED_RMS_DB)
