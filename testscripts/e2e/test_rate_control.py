"""Rate control: the PI controller, the buffer level, and clocks that disagree.

`PIRateController` runs in all six real backends and in none of the software ones, so
until the dummy playback became a clock master nothing exercised it end to end on any
platform. These tests close that: the dummy playback measures its buffer, feeds the
controller and sends SetSpeed, and the dummy capture answers by running its own clock
faster or slower, which is what a clock-slave capture device does.

The `drift` knob on the control socket is what makes the two clocks disagree. Carrying
the magnitude on a command rather than in the config is what allows a settled session to
be perturbed: a reload would restart the devices and reset the controller, which is the
state the interesting cases start from.

The base config runs a 0.2 s adjust interval, see dummy_rate.yml for why that converges
faster than the 10 s default rather than slower.
"""

import time

import pytest

# Every assertion here is about a control loop settling, which needs the clock kept
# accurately enough for the buffer level to mean something, so CI runs this file on
# Linux only. See pytest.ini.
pytestmark = pytest.mark.pacing

TARGET_LEVEL = 2048
CHUNKSIZE = 512
# A drift the controller can correct, comfortably inside its +/- 5000 ppm clamp.
DRIFT_PPM = 1000
# What the correction is allowed to be off by. The controller settles within a couple of
# hundred ppm of the drift it is cancelling, so this is loose enough for a busy runner and
# still tight enough to catch a wrong sign or a factor of two.
PPM_TOLERANCE = 0.5
# A drift far beyond the clamp, for the cases where the loop is meant to lose.
RUNAWAY_PPM = 50000

NO_RATE_ADJUST = {"enable_rate_adjust: true": "enable_rate_adjust: false"}

# The resampler cases below run on dummy_resample.yml, which shares this file's chunk size
# and target level so the tolerances above carry over unchanged. Its capture runs at 96 kHz
# into a 48 kHz pipeline, which is what puts a resampler in the path.
RESAMPLING = "dummy_resample.yml"
SYNCHRONOUS = {"    type: AsyncPoly\n    interpolation: Cubic": "    type: Synchronous"}
# Enough drift to empty the buffer inside a few seconds, and still comfortably inside the
# controller's clamp, so a loop that could act on it would.
SYNC_DRIFT_PPM = 4000


def level_tolerance(target):
    """How far the averaged level may sit from the target and still count as settled.

    A whole chunk of it is the sawtooth the average sits in the middle of, see
    `average_level`, so the band has to be at least that wide however small the target
    is. The rest is room for a busy runner.
    """
    return 0.15 * target + CHUNKSIZE


def start(control_cdsp, replacements=None, base="dummy_rate.yml"):
    return control_cdsp(replacements=replacements, base=base)


def average_level(cdsp, samples=8, interval=0.03):
    """The buffer level averaged over a few readings.

    One reading is a point on a sawtooth. The level is measured as a chunk arrives and
    drains by a chunk before the next one does, so consecutive readings differ by up to a
    whole chunk, which on this config is a quarter of the target. The average is both
    steadier and closer to what the controller itself works on, though still half a chunk
    below the true level for the same reason.
    """
    values = []
    for _ in range(samples):
        values.append(cdsp.send("GetBufferLevel"))
        time.sleep(interval)
    return sum(values) / len(values)


def wait_for_level(cdsp, target, timeout=20.0):
    """Wait for the averaged buffer level to settle at `target`, and return it."""
    deadline = time.monotonic() + timeout
    while True:
        level = average_level(cdsp)
        if abs(level - target) < level_tolerance(target):
            return level
        if time.monotonic() >= deadline:
            raise AssertionError(f"Buffer level was still {level:.0f} after {timeout} s, not near {target}")


def wait_for_adjust_to_start(cdsp, timeout=20.0):
    """Wait for the first SetSpeed to arrive.

    The adjust is reported as exactly 0.0 until then, not as 1.0, so a test that reads it
    too early sees a factor of a million rather than a nominal speed.
    """
    return cdsp.poll_until_true("GetRateAdjust", lambda speed: speed > 0.5, timeout=timeout)


def wait_for_adjust(cdsp, expected_ppm, timeout=20.0):
    """Wait for the rate adjust to settle at `expected_ppm`, and return it in ppm.

    The adjust is a speed factor around 1.0, so the ppm it is compared against is what it
    is asking the capture clock to do.
    """
    span = abs(expected_ppm) * PPM_TOLERANCE

    def close_enough(speed):
        return speed > 0.5 and abs((speed - 1.0) * 1e6 - expected_ppm) < span

    return (cdsp.poll_until_true("GetRateAdjust", close_enough, timeout=timeout, interval=0.05) - 1.0) * 1e6


@pytest.mark.parametrize("target", [1024, TARGET_LEVEL, 4096])
def test_the_buffer_level_settles_at_the_target(control_cdsp, target):
    """With both clocks on nominal, the level should sit where the config asks."""
    cdsp = start(control_cdsp, {f"target_level: {TARGET_LEVEL}": f"target_level: {target}"})
    wait_for_level(cdsp, target)
    # And stay there, rather than passing through on the way somewhere else.
    time.sleep(1.0)
    assert average_level(cdsp) == pytest.approx(target, abs=level_tolerance(target))


@pytest.mark.parametrize(
    "device,drift_ppm",
    [("playback", DRIFT_PPM), ("playback", -DRIFT_PPM), ("capture", DRIFT_PPM)],
)
def test_rate_adjust_cancels_a_drifting_clock(control_cdsp, device, drift_ppm):
    """The loop should find the speed that makes the two clocks agree again.

    A playback running fast has to be fed faster, a capture running fast has to be slowed
    down, so the correction has the opposite sign on the two sides. Getting that backwards
    gives a loop that runs away instead of settling, which is what this pins down.
    """
    cdsp = start(control_cdsp)
    wait_for_level(cdsp, TARGET_LEVEL)
    wait_for_adjust_to_start(cdsp)

    controls = {"capture": cdsp.capture_control, "playback": cdsp.playback_control}
    controls[device].set("drift", drift_ppm)
    expected_ppm = drift_ppm if device == "playback" else -drift_ppm
    wait_for_adjust(cdsp, expected_ppm)
    # The point of the correction is the level, so check that too.
    wait_for_level(cdsp, TARGET_LEVEL)


def test_the_level_runs_away_without_rate_adjust(control_cdsp):
    """With the loop off, a drifting clock empties the buffer and nothing stops it.

    This is the other half of the test above: it is what the correction is preventing.
    Nothing here polls for a settled starting level, because with the loop off there is
    nothing to settle it: whatever a startup transient leaves in the buffer stays.
    """
    cdsp = start(control_cdsp, NO_RATE_ADJUST)
    assert average_level(cdsp) > 0.3 * TARGET_LEVEL
    # A playback running fast drains the buffer it is given.
    cdsp.playback_control.set("drift", RUNAWAY_PPM)
    cdsp.poll_until_true("GetBufferLevel", lambda level: level < 0.1 * TARGET_LEVEL, timeout=20.0)
    # Nothing was ever asked of the capture clock.
    assert cdsp.send("GetRateAdjust") == 0.0


def test_a_slow_playback_backs_the_buffer_up(control_cdsp):
    """The other direction fills the buffer instead, and then the capture has to wait.

    A capture that has to wait falls behind its own clock, which is what an overrun is
    here, so the resync counter is where that shows up.
    """
    cdsp = start(control_cdsp, NO_RATE_ADJUST)
    assert average_level(cdsp) < 1.5 * TARGET_LEVEL
    cdsp.playback_control.set("drift", -RUNAWAY_PPM)
    # The buffer holds twice the target level, so that is where the level tops out.
    cdsp.poll_until_true("GetBufferLevel", lambda level: level > 2 * TARGET_LEVEL, timeout=20.0)
    deadline = time.monotonic() + 20.0
    while cdsp.capture_control.get_int("resyncs") == 0:
        assert time.monotonic() < deadline, "the capture never fell behind"
        time.sleep(0.05)


def test_the_correction_is_clamped(control_cdsp):
    """The controller may not ask for more than half a percent, whatever it sees.

    So a drift this far off nominal is one the loop is meant to lose, and the buffer runs
    away anyway. A clamp that was not there would show up as a correction that follows the
    drift all the way out.
    """
    cdsp = start(control_cdsp)
    wait_for_level(cdsp, TARGET_LEVEL)
    cdsp.playback_control.set("drift", RUNAWAY_PPM)
    cdsp.poll_until_true("GetBufferLevel", lambda level: level < TARGET_LEVEL // 4, timeout=20.0)
    # Saturated at the clamp in `PIRateController::next`, nowhere near the 50000 ppm it
    # would need to keep up.
    assert cdsp.poll_until_true(
        "GetRateAdjust", lambda speed: speed == pytest.approx(1.005, abs=1e-4), timeout=20.0
    )


def test_a_settled_session_recovers_from_a_disturbance(control_cdsp):
    """Let the loop settle, knock the buffer off target, and watch it come back.

    This is the case the control socket exists for. A drift set in the config could not
    test it, since applying one needs a reload, and a reload restarts the devices and
    resets the controller along with them.
    """
    cdsp = start(control_cdsp)
    wait_for_level(cdsp, TARGET_LEVEL)
    wait_for_adjust_to_start(cdsp)

    # Far enough off to be a real excursion, brief enough to leave the buffer with
    # something in it.
    cdsp.playback_control.set("drift", 20000)
    cdsp.poll_until_true("GetBufferLevel", lambda level: level < 0.6 * TARGET_LEVEL, timeout=10.0)
    cdsp.playback_control.set("drift", 0)

    wait_for_level(cdsp, TARGET_LEVEL, timeout=30.0)
    assert cdsp.send("GetState") == "Running"


@pytest.mark.parametrize(
    "device,drift_ppm",
    [("playback", DRIFT_PPM), ("capture", DRIFT_PPM), ("capture", -DRIFT_PPM)],
)
def test_rate_adjust_through_an_async_resampler(control_cdsp, device, drift_ppm):
    """The same loop, with an asynchronous resampler answering SetSpeed instead of a clock.

    This is the case every real backend runs: the capture device's clock is not ours to
    change, so the correction goes into the resample ratio,
    `src/utils/resampling.rs:set_resample_ratio_relative`. The test above covers the other
    shape, where there is no resampler and the device slews its own clock instead. Both have
    to settle at the same correction, since what the controller sees is the same buffer.
    """
    cdsp = start(control_cdsp, base=RESAMPLING)
    wait_for_level(cdsp, TARGET_LEVEL)
    wait_for_adjust_to_start(cdsp)

    controls = {"capture": cdsp.capture_control, "playback": cdsp.playback_control}
    controls[device].set("drift", drift_ppm)
    expected_ppm = drift_ppm if device == "playback" else -drift_ppm
    wait_for_adjust(cdsp, expected_ppm)
    wait_for_level(cdsp, TARGET_LEVEL)


def test_a_synchronous_resampler_ignores_rate_adjust(control_cdsp):
    """A synchronous resampler has a fixed ratio, so it warns and drops the request.

    Nothing then closes the loop: the controller keeps seeing the same error, winds out to
    its clamp and stays there, and the buffer goes wherever the clocks take it. The drift
    here is well inside the clamp, so an async resampler would have corrected it and settled,
    which is what makes the pegged correction the signature of the request being refused
    rather than of a drift too large to answer.
    """
    cdsp = start(control_cdsp, SYNCHRONOUS, base=RESAMPLING)
    wait_for_adjust_to_start(cdsp)
    # There is nothing to settle at, so this starts from whatever the prefill left in the
    # buffer rather than from the target. It only has to be enough to have something to lose.
    assert average_level(cdsp) > TARGET_LEVEL // 2
    cdsp.playback_control.set("drift", SYNC_DRIFT_PPM)
    cdsp.poll_until_true(
        "GetRateAdjust",
        lambda speed: abs(speed - 1.0) == pytest.approx(0.005, abs=1e-4),
        timeout=20.0,
    )
    # And the buffer empties anyway, since nothing acted on the request.
    cdsp.poll_until_true("GetBufferLevel", lambda level: level < TARGET_LEVEL // 4, timeout=25.0)
