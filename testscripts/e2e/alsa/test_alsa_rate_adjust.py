"""Rate adjust with ALSA on both ends, against a playback clock the test can skew.

test_rate_control.py covers the controller with the Dummy devices, where a SetSpeed goes
into the Dummy capture's own pacer. Here it goes where it goes on a real system with a
loopback capture: into the loopback's `PCM Rate Shift 100000` control, which
`src/alsa_backend/device.rs` finds on the capture cable and writes on every adjust.
Nothing else in the suite reaches that write.

The sink is the second loopback cable, with nothing on its far end. snd-dummy would be
the natural sink but has no control over its clock, and the cable's own rate shift is
exactly that control. The test sets it, and the adjust loop has to bring the capture
cable to the same shift, which it can read straight back from the card. So the
assertions here are on the device, not only on what the engine says it asked for.

The capture cable's shift is also what the feeder runs at, since both ends of a cable
share it, so the source slows down and speeds up along with the capture the way a
loopback source does.

In CI this runs in a VM on a shared runner, where scheduling is jittery, and the loop
answers jitter the way it answers drift: the speed moves. So nothing here asserts on a
single reading. Settling is judged on the median of several, with a tolerance that is
wide against jitter and still narrow against a wrong sign, and timeouts are generous.
"""

import statistics
import time

import pytest

from .loopback import (
    CAPTURE_CABLE,
    DUMMY,
    NOMINAL_SHIFT,
    PLAYBACK_CABLE,
    rate_shift,
    set_rate_shift,
    shifted_rate,
)

pytestmark = pytest.mark.alsa

RATE = 48000
CHUNKSIZE = 1024
TARGET_LEVEL = 2048
# Short compared to the 10 s default, for the reason given in dummy_rate.yml: the
# controller works on the error per interval, so a short one moves faster.
ADJUST_INTERVAL = 0.5
RATE_ADJUST = {
    "enable_rate_adjust": True,
    "target_level": TARGET_LEVEL,
    "adjust_interval_s": ADJUST_INTERVAL,
}
# A sink skew well inside the controller's 0.5 % clamp, and big enough that a wrong sign
# or a factor of two lands far outside the tolerance below.
SKEW = 300
# How close the capture cable has to come to the sink's shift. The controller keeps
# nudging the buffer level towards its target, so it hovers a few tens of units off the
# exact match rather than sitting on it.
SHIFT_TOLERANCE = 100
SETTLE_TIMEOUT = 40.0


def median_shift(readings=5, interval=0.2):
    """The capture cable's rate shift, as the median of a few readings.

    One reading can catch the loop in the middle of answering a scheduling hiccup, see
    the module docstring, and the median is what says where it actually sits.
    """
    values = []
    for _ in range(readings):
        values.append(rate_shift(CAPTURE_CABLE))
        time.sleep(interval)
    return statistics.median(values)


def wait_for_capture_shift(expected, tolerance=SHIFT_TOLERANCE, timeout=SETTLE_TIMEOUT):
    """Wait for the capture cable's rate shift to settle near `expected`, and return it."""
    deadline = time.monotonic() + timeout
    while True:
        shift = median_shift()
        if abs(shift - expected) <= tolerance:
            return shift
        if time.monotonic() > deadline:
            raise AssertionError(
                f"the capture cable's shift was {shift} after {timeout} s, not near {expected}"
            )


def wait_for_level(cdsp, target=TARGET_LEVEL, timeout=20.0):
    """Wait for the averaged buffer level to come within a chunk and a half of `target`."""
    deadline = time.monotonic() + timeout
    while True:
        level = average_level(cdsp)
        if abs(level - target) < 0.5 * target + CHUNKSIZE:
            return level
        if time.monotonic() > deadline:
            raise AssertionError(
                f"the buffer level was {level:.0f} after {timeout} s, not near {target}"
            )


def average_level(cdsp, samples=8, interval=0.05):
    """The buffer level averaged over a few readings, see test_rate_control.py for why."""
    values = []
    for _ in range(samples):
        values.append(cdsp.send("GetBufferLevel"))
        time.sleep(interval)
    return sum(values) / len(values)


@pytest.mark.parametrize("skew", [SKEW, -SKEW])
def test_rate_adjust_matches_the_capture_to_a_skewed_sink(start_cdsp, alsa_config, feeder, skew):
    """A sink running off nominal from the start, and the capture has to follow it.

    A positive skew is a slow sink, which has to be fed more slowly, so the capture
    shift goes up with it. The engine's own report of the speed has to agree with what
    reached the card, since the shift is written as 100000 over the speed.
    """
    sink_shift = NOMINAL_SHIFT + skew
    set_rate_shift(PLAYBACK_CABLE, sink_shift)
    feeder()
    cdsp = start_cdsp(config=alsa_config(devices=RATE_ADJUST))
    # The card is written on every adjust, but the reported speed is a status snapshot
    # refreshed once per update interval. At the default second that is two adjusts
    # behind at worst, and after an underrun on a jittery runner the loop moves enough
    # between them to pull the median off. A tenth of a second keeps most pairs on the
    # same adjust.
    cdsp.send("SetUpdateInterval", 100)
    wait_for_capture_shift(sink_shift)
    # What the engine reports asking for against what reached the card. Interleaved
    # readings and a median of the differences compare where the two sit, rather than
    # trusting one pair to land between two adjusts.
    differences = []
    for _ in range(10):
        reported = NOMINAL_SHIFT / cdsp.send("GetRateAdjust")
        differences.append(reported - rate_shift(CAPTURE_CABLE))
        time.sleep(0.2)
    assert abs(statistics.median(differences)) < SHIFT_TOLERANCE
    # The point of matching the clocks is the level, so that has to be where it was asked.
    wait_for_level(cdsp)


def test_rate_adjust_follows_a_sink_that_changes(start_cdsp, alsa_config, feeder):
    """Settle against one skew, then flip it, and the capture has to go all the way over.

    This is the case a reload cannot test, since a reload restarts the devices and the
    controller along with them, and it is what a sound card's clock wandering looks like.
    """
    set_rate_shift(PLAYBACK_CABLE, NOMINAL_SHIFT + SKEW)
    feeder()
    start_cdsp(config=alsa_config(devices=RATE_ADJUST))
    wait_for_capture_shift(NOMINAL_SHIFT + SKEW)
    set_rate_shift(PLAYBACK_CABLE, NOMINAL_SHIFT - SKEW)
    wait_for_capture_shift(NOMINAL_SHIFT - SKEW)


def test_rate_adjust_against_snd_dummy_stays_near_nominal(start_cdsp, alsa_config, feeder):
    """Against a sink on the same system clock there is nothing to correct.

    snd-dummy and the loopback both run on the kernel's timers, so the capture shift
    should stay close to where it started. A loop that wandered off here would be
    chasing its own noise.
    """
    feeder()
    cdsp = start_cdsp(config=alsa_config(playback_device=DUMMY, devices=RATE_ADJUST))
    # The first SetSpeed is what shows the loop is running at all. From there it may
    # take a while to come back from whatever the startup did to the buffer level, so
    # this waits for it to settle rather than reading it at a fixed time.
    cdsp.poll_until_true("GetRateAdjust", lambda speed: speed > 0.5, timeout=10.0)
    wait_for_capture_shift(NOMINAL_SHIFT)
    assert cdsp.send("GetState") == "Running"


def test_the_correction_is_clamped(start_cdsp, alsa_config, feeder):
    """A sink 2 % slow is beyond the half percent the controller may ask for.

    So the capture shift has to stop at the clamp, 100000 / 0.995, rather than follow
    the sink all the way to 102000.
    """
    set_rate_shift(PLAYBACK_CABLE, 102000)
    feeder()
    start_cdsp(config=alsa_config(devices=RATE_ADJUST))
    # Pegged, so jitter cannot pull it off the clamp: the error it would have to undo
    # keeps growing for as long as the sink stays slow.
    clamped = NOMINAL_SHIFT / 0.995
    wait_for_capture_shift(clamped, tolerance=3)
    time.sleep(2.0)
    assert median_shift() == pytest.approx(clamped, abs=3)


def test_without_rate_adjust_the_capture_cable_is_left_alone(start_cdsp, alsa_config, feeder):
    """The backend only writes the control when the loop asks it to.

    Worth pinning because the control belongs to the card and outlives the process: a
    stray write here would change the rate for whatever uses the loopback next.
    """
    set_rate_shift(PLAYBACK_CABLE, NOMINAL_SHIFT + SKEW)
    feeder()
    cdsp = start_cdsp(config=alsa_config())
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0, timeout=10.0)
    time.sleep(3.0)
    assert rate_shift(CAPTURE_CABLE) == NOMINAL_SHIFT
    assert cdsp.send("GetRateAdjust") == 0.0


@pytest.mark.parametrize("shift", [99000, 101000])
def test_a_clock_off_nominal_is_measured(start_cdsp, alsa_config, feeder, shift):
    """Both cables shifted together, and the measured capture rate has to follow.

    Shifting only the capture would leave the two ends disagreeing, and a capture that
    runs ahead of its playback ends up waiting on it, which measures the playback's rate
    rather than its own. With both moved the same way nothing drifts, and what is left
    is the measurement itself, which is what the rate watcher and the GUI's rate display
    rely on.
    """
    set_rate_shift(CAPTURE_CABLE, shift)
    set_rate_shift(PLAYBACK_CABLE, shift)
    feeder()
    cdsp = start_cdsp(config=alsa_config())
    expected = shifted_rate(RATE, shift)
    cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: abs(rate - expected) < 0.001 * RATE, timeout=10.0
    )
