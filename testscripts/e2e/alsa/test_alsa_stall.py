"""A device that stops moving, and later starts again.

This is the UAC2 gadget case: the host closes its stream, the gadget stops moving without
an error, and it resumes when the host opens the stream again. A gadget capture, with
the host playing into CamillaDSP, is the common setup. A gadget playback is rarer.

The stalling device here is the Dummy clocked loopback card, see `loopback.py`. Killing
the `DummyClock` stalls it and starting a new one resumes it. The other end of
CamillaDSP is on the first loopback card and keeps running all along, the way the
microphone or the DAC on the other side of a gadget does.

Three things can go wrong, and each has a test:

- A stalled capture has to show in the processing state.
- While a playback is stalled there is no buffer level to measure. Anything that keeps
  averaging one anyway feeds the rate controller garbage, and it winds up to the clamp.
  A stalled capture delivers no chunks, so nothing reaches the controller then.
- On resume, whatever piled up during the stall, zeros in the device and stale chunks in
  the backend's own queues, is played before anything current. The level then starts
  far from where it was, and rate adjust can only move it at half a percent.

Rate adjust writes the capture cable's rate shift, as in test_alsa_rate_adjust.py. On
the stall card that control does nothing, since a cable on a card timer moves one
period per tick whatever the shift. So with the capture on the stall card the
controller runs and reports, but cannot move the level. Nothing drifts, every clock
here is the system's, so the level stays where the start or the last restart put it.
"""

import statistics
import time
from collections import namedtuple

import pytest

from .loopback import (
    NOMINAL_SHIFT,
    PLAYBACK,
    STALL_CAPTURE,
    STALL_CARD,
    STALL_PLAYBACK,
    DummyClock,
    hw_ptr,
    set_rate_shift,
    stall_card_present,
)

pytestmark = pytest.mark.alsa

# The chunk size for each direction, the period CamillaDSP picks for it, see
# `DummyClock`, and how far from the target the level may come back after a stall.
Setup = namedtuple("Setup", ["chunksize", "period", "level_tolerance"])
SETUPS = {
    # A large chunk gives a large device buffer, 8192 frames. A restart lands up to a
    # chunk from the target, depending on where in a chunk it falls, while zeros left
    # behind by a stall put the level more than 4000 frames too high.
    "playback": Setup(1536, 1024, 1536),
    # A resume after a capture stall restarts the playback, and in the VM that lands
    # within about 450 frames of the target. A backend that resumes without topping
    # the device up to the target comes back most of a chunk low.
    "capture": Setup(1024, 512, 600),
}
TARGET_LEVEL = 2048
ADJUST_INTERVAL = 0.5
RATE_ADJUST = {
    "enable_rate_adjust": True,
    "target_level": TARGET_LEVEL,
    "adjust_interval_s": ADJUST_INTERVAL,
}
# Six adjust periods, enough for a controller fed garbage to reach the clamp.
STALL_TIME = 3.0
# Comfortably longer than it takes a backend to call the device stalled. The threaded
# one waits twice the device buffer, a third of a second here.
DETECT_TIME = 1.0
# How far from 1 the speed may be to count as settled. The level the threaded backend
# estimates jitters by a few hundred frames between adjust periods, which moves the
# speed by up to about 0.3 %.
SPEED_TOLERANCE = 0.003
SETTLE_TIMEOUT = 30.0
STALL_CABLES = {"playback": 0, "capture": 1}


@pytest.fixture
def dummy_clock():
    """Factory for a running DummyClock, stopped when the test ends.

    Ask for it before `start_cdsp`, so that CamillaDSP exits before its clock stops.
    The stall card's rate shifts are reset around the test, like the first card's.
    """
    if not stall_card_present():
        pytest.skip(f"needs the {STALL_CARD} card clocked by snd-dummy, see alsa/loopback.py")
    for cable in STALL_CABLES.values():
        set_rate_shift(cable, NOMINAL_SHIFT, card=STALL_CARD)
    started = []

    def _start(period):
        clock = DummyClock(period)
        started.append(clock)
        return clock

    yield _start

    for clock in started:
        clock.stop()
    for cable in STALL_CABLES.values():
        set_rate_shift(cable, NOMINAL_SHIFT, card=STALL_CARD)


def stall_hw_ptr(direction):
    device, stream = (0, "p") if direction == "playback" else (1, "c")
    return hw_ptr(device, stream, STALL_CABLES[direction], card=STALL_CARD)


def wait_for_hw_ptr(direction, moving, timeout=5.0):
    """Wait for the stalling device to be moving, or to have stopped moving."""
    deadline = time.monotonic() + timeout
    previous = stall_hw_ptr(direction)
    while True:
        time.sleep(0.1)
        current = stall_hw_ptr(direction)
        if (current != previous) == moving:
            return
        if time.monotonic() > deadline:
            state = "moving" if moving else "still"
            raise TimeoutError(f"the stalling {direction} was not {state} after {timeout} s")
        previous = current


def median_level(cdsp, seconds=2.0, interval=0.1):
    """The median of GetBufferLevel over a few adjust periods.

    The level is an average the backend publishes once per adjust period, so one
    reading is one period's worth, and the median keeps a jittery one from deciding.
    """
    values = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        values.append(cdsp.send("GetBufferLevel"))
        time.sleep(interval)
    return statistics.median(values)


def start_settled(direction, dummy_clock, start_cdsp, alsa_config, feeder):
    """Start with the stall card on one end, and wait for the level to settle.

    Returns the clock and the handle. With the playback on the stall card, settled
    means the level near the target and the speed near 1. A speed far from
    1 is the controller still recovering from the start, and a stall test that began
    there would measure that. With the capture on the stall card rate adjust has nothing
    to act on, see the module docstring, so settled only means the level stopped moving.
    """
    setup = SETUPS[direction]
    clock = dummy_clock(setup.period)
    if direction == "playback":
        feeder()
        config = alsa_config(
            playback_device=STALL_PLAYBACK, chunksize=setup.chunksize, devices=RATE_ADJUST
        )
    else:
        config = alsa_config(
            capture_device=STALL_CAPTURE,
            playback_device=PLAYBACK,
            chunksize=setup.chunksize,
            devices=RATE_ADJUST,
        )
    cdsp = start_cdsp(config=config)
    # GetRateAdjust is a status snapshot, and this keeps it a tenth of a second old at
    # most rather than a second, as in test_alsa_rate_adjust.py.
    cdsp.send("SetUpdateInterval", 100)
    deadline = time.monotonic() + SETTLE_TIMEOUT
    previous = None
    while True:
        level = median_level(cdsp, seconds=1.0)
        speed = cdsp.send("GetRateAdjust")
        if direction == "playback":
            settled = (
                abs(level - TARGET_LEVEL) < setup.level_tolerance
                and abs(speed - 1) < SPEED_TOLERANCE
            )
        else:
            settled = previous is not None and abs(level - previous) < setup.period / 2
        if settled:
            return clock, cdsp
        if time.monotonic() > deadline:
            raise AssertionError(
                f"not settled after {SETTLE_TIMEOUT} s, level {level}, speed {speed}"
            )
        previous = level


def stall(direction, clock):
    """Stop the clock and wait out the stall, with the device seen to stand still."""
    clock.stop()
    wait_for_hw_ptr(direction, moving=False)
    time.sleep(STALL_TIME)


def resume(direction, dummy_clock):
    dummy_clock(SETUPS[direction].period)
    wait_for_hw_ptr(direction, moving=True)


def test_a_stalled_capture_shows_in_the_state(dummy_clock, start_cdsp, alsa_config, feeder):
    """A capture that stops delivering stalls the processing, and the engine says so.

    This is what a GUI shows for a gadget whose host has stopped playing, and it has to
    go back to Running when the host starts again.
    """
    clock, cdsp = start_settled("capture", dummy_clock, start_cdsp, alsa_config, feeder)
    clock.stop()
    cdsp.poll_until("GetState", "Stalled")
    resume("capture", dummy_clock)
    cdsp.poll_until("GetState", "Running")


def test_nothing_is_adjusted_during_a_playback_stall(
    dummy_clock, start_cdsp, alsa_config, feeder
):
    """Once the playback is called stalled, neither the level nor the speed moves.

    Nothing is played during the stall, so there is no level to report and no drift to
    correct. A backend that keeps reporting a level while stalled reports one that decays
    to nothing or grows without bound, and the controller follows it to the clamp.

    The time before the stall is called is left out. The device stops taking data before
    it stops accepting it, so the level rises as the buffer fills, and the controller
    answers that like it answers a slow device. That is the right thing to do until it
    is clear that the device is not slow but stopped.
    """
    clock, cdsp = start_settled("playback", dummy_clock, start_cdsp, alsa_config, feeder)
    clock.stop()
    wait_for_hw_ptr("playback", moving=False)
    time.sleep(DETECT_TIME)
    level, speed = cdsp.send("GetBufferLevel"), cdsp.send("GetRateAdjust")
    time.sleep(STALL_TIME)
    level_after, speed_after = cdsp.send("GetBufferLevel"), cdsp.send("GetRateAdjust")
    assert (level_after, speed_after) == (level, speed), (
        f"during the stall the level went from {level} to {level_after}, "
        f"and the speed from {speed} to {speed_after}"
    )


@pytest.mark.parametrize("direction", ["capture", "playback"])
def test_the_level_comes_back_after_a_stall(
    dummy_clock, start_cdsp, alsa_config, feeder, direction
):
    """After a stall the level starts at the target again, the way a fresh start does.

    The first two adjust periods after the resume are skipped, since the level
    published then can still hold readings from before the stall.
    """
    clock, cdsp = start_settled(direction, dummy_clock, start_cdsp, alsa_config, feeder)
    stall(direction, clock)
    resume(direction, dummy_clock)
    time.sleep(2 * ADJUST_INTERVAL)
    level = median_level(cdsp)
    assert abs(level - TARGET_LEVEL) < SETUPS[direction].level_tolerance, (
        f"the level was {level} after the stall, the target is {TARGET_LEVEL}"
    )
