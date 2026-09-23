"""Rate adjust with CoreAudio on both ends, against a playback clock the test can skew.

test_rate_control.py covers the controller with the Dummy devices, where a SetSpeed goes
into the Dummy capture's own pacer. Here it goes where it goes on a Mac with a loopback
capture: into BlackHole's pitch control, which `src/coreaudio_backend/device.rs` finds
behind the "Internal Adjustable" clock source and writes through the stereo pan on every
adjust. Nothing else in the suite reaches that write.

The sink is BlackHole 16ch put on its own adjustable clock at a pitch the test sets, and
the adjust loop has to bring the capture device's pitch to the same value, which the
test reads straight back from the device. So the assertions here are on the device, not
only on what the engine says it asked for.

The capture device's pitch is also what the feeder runs at, since it plays into the same
device, so the source speeds up and slows down along with the capture the way a loopback
source does.

The loop answers scheduling jitter the way it answers drift, so nothing here asserts on
a single reading. Settling is judged on the median of several, with a tolerance that is
wide against jitter and still narrow against a wrong sign.
"""

import statistics
import time

import pytest

from .blackhole import (
    ADJUSTABLE_CLOCK,
    FEED_DEVICE,
    FIXED_CLOCK,
    SINK_DEVICE,
    clock_source,
    pitch,
    set_pitch,
)

pytestmark = pytest.mark.coreaudio

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
SKEW = 0.003
# How close the capture has to come to the sink's pitch. The controller keeps nudging
# the buffer level towards its target, so it hovers a little off the exact match.
PITCH_TOLERANCE = 0.001
# The controller's limit on the correction, `src/utils/rate_controller.rs`.
CLAMP = 0.005
SETTLE_TIMEOUT = 40.0


def median_pitch(readings=5, interval=0.2):
    """The capture device's pitch, as the median of a few readings.

    One reading can catch the loop in the middle of answering a scheduling hiccup, and
    the median is what says where it actually sits.
    """
    values = []
    for _ in range(readings):
        values.append(pitch(FEED_DEVICE))
        time.sleep(interval)
    return statistics.median(values)


def wait_for_capture_pitch(expected, tolerance=PITCH_TOLERANCE, timeout=SETTLE_TIMEOUT):
    """Wait for the capture device's pitch to settle near `expected`, and return it."""
    deadline = time.monotonic() + timeout
    while True:
        value = median_pitch()
        if abs(value - expected) <= tolerance:
            return value
        if time.monotonic() > deadline:
            raise AssertionError(
                f"the capture pitch was {value:.5f} after {timeout} s, not near {expected:.5f}"
            )


def average_level(cdsp, samples=8, interval=0.05):
    """The buffer level averaged over a few readings, see test_rate_control.py for why."""
    values = []
    for _ in range(samples):
        values.append(cdsp.send("GetBufferLevel"))
        time.sleep(interval)
    return sum(values) / len(values)


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


@pytest.mark.parametrize("skew", [SKEW, -SKEW])
def test_rate_adjust_matches_the_capture_to_a_skewed_sink(start_cdsp, ca_config, feeder, skew):
    """A sink running off nominal from the start, and the capture has to follow it.

    A pitch above 1 is a fast sink, which has to be fed faster, so the capture pitch
    goes up with it. The engine's own report of the speed has to agree with what reached
    the device, since the pitch is written as the speed itself.
    """
    set_pitch(SINK_DEVICE, 1.0 + skew)
    feeder()
    cdsp = start_cdsp(config=ca_config(devices=RATE_ADJUST))
    wait_for_capture_pitch(1.0 + skew)
    # What the engine reports asking for against what reached the device. The report is
    # a status snapshot refreshed on its own interval, so interleaved readings and a
    # median of the differences compare where the two sit rather than two moments.
    differences = []
    for _ in range(10):
        differences.append(cdsp.send("GetRateAdjust") - pitch(FEED_DEVICE))
        time.sleep(0.2)
    assert abs(statistics.median(differences)) < PITCH_TOLERANCE
    # The point of matching the clocks is the level, so that has to be where it was asked.
    wait_for_level(cdsp)


def test_rate_adjust_follows_a_sink_that_changes(start_cdsp, ca_config, feeder):
    """Settle against one skew, then flip it, and the capture has to go all the way over.

    This is the case a reload cannot test, since a reload restarts the devices and the
    controller along with them, and it is what a sound card's clock wandering looks like.
    """
    set_pitch(SINK_DEVICE, 1.0 + SKEW)
    feeder()
    start_cdsp(config=ca_config(devices=RATE_ADJUST))
    wait_for_capture_pitch(1.0 + SKEW)
    set_pitch(SINK_DEVICE, 1.0 - SKEW)
    wait_for_capture_pitch(1.0 - SKEW)


def test_rate_adjust_against_an_unskewed_sink_stays_near_nominal(start_cdsp, ca_config, feeder):
    """Both devices on the host clock, so there is nothing to correct.

    A loop that wandered off here would be chasing its own noise.
    """
    feeder()
    cdsp = start_cdsp(config=ca_config(devices=RATE_ADJUST))
    # The first SetSpeed is what shows the loop is running at all.
    cdsp.poll_until_true("GetRateAdjust", lambda speed: speed > 0.5, timeout=10.0)
    wait_for_capture_pitch(1.0)
    assert cdsp.send("GetState") == "Running"


def test_the_correction_is_clamped(start_cdsp, ca_config, feeder):
    """A sink 1 % fast is beyond the half percent the controller may ask for.

    1 % is also as far as BlackHole's pitch goes. The capture pitch has to stop at the
    clamp rather than follow the sink all the way.
    """
    set_pitch(SINK_DEVICE, 1.01)
    feeder()
    start_cdsp(config=ca_config(devices=RATE_ADJUST))
    # Pegged, so jitter cannot pull it off the clamp: the error it would have to undo
    # keeps growing for as long as the sink stays fast.
    wait_for_capture_pitch(1.0 + CLAMP, tolerance=1e-4)
    time.sleep(2.0)
    assert median_pitch() == pytest.approx(1.0 + CLAMP, abs=1e-4)


@pytest.mark.parametrize(
    "clock,start_pitch",
    [(FIXED_CLOCK, 1.0), (ADJUSTABLE_CLOCK, 1.0), (ADJUSTABLE_CLOCK, 1.002)],
)
def test_without_rate_adjust_the_capture_device_is_left_alone(
    start_cdsp, ca_config, feeder, clock, start_pitch
):
    """The clock source and pitch are only touched when rate adjust is going to use them.

    Both belong to the device, outlive the process, and may be set by someone else, a
    user in Audio MIDI Setup or another program. So a session without rate adjust has to
    leave the device on whatever clock and pitch it found, including a pitch an earlier
    session with rate adjust ended at.
    """
    if clock == ADJUSTABLE_CLOCK:
        set_pitch(FEED_DEVICE, start_pitch)
    set_pitch(SINK_DEVICE, 1.0 + SKEW)
    feeder()
    cdsp = start_cdsp(config=ca_config())
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0, timeout=10.0)
    time.sleep(3.0)
    assert clock_source(FEED_DEVICE) == clock
    assert pitch(FEED_DEVICE) == pytest.approx(start_pitch, abs=1e-6)
    assert cdsp.send("GetRateAdjust") == 0.0
