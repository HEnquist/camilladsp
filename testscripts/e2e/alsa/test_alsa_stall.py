"""A device that stops moving, and later starts again.

This is the UAC2 gadget case: the host closes its stream, the gadget stops moving without
an error, and it resumes when the host opens the stream again. A gadget capture, with
the host playing into CamillaDSP, is the common setup. A gadget playback is rarer.

The stalling device here is the Dummy clocked loopback card, see `loopback.py`. Killing
the `DummyClock` stalls it and starting a new one resumes it. The other end of
CamillaDSP is on the first loopback card and keeps running all along, the way the
microphone or the DAC on the other side of a gadget does. The card can lose time on a
busy machine, and a test that fails after it did is run again, see `attempts`.

Four things can go wrong, and each has a test:

- A stalled capture has to show in the processing state.
- A playback that resumes has to play what comes in again. The device moves on silence
  just as well, so neither its position nor a level tells whether audio gets through.
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
import threading
import time
from collections import namedtuple

import numpy as np
import pytest

from .loopback import (
    NOMINAL_SHIFT,
    PLAYBACK,
    STALL_CAPTURE,
    STALL_CARD,
    STALL_PLAYBACK,
    STALL_RECORD,
    DummyClock,
    decode,
    hw_ptr,
    pcm_state,
    record,
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
# How many times a test is run before a stall card that keeps losing time fails it.
ATTEMPTS = 3
EXIT_OK = 0
STALL_CABLES = {"playback": 0, "capture": 1}
# The feeder's tone, see the `feeder` fixture, and how close a peak has to come to it.
RATE = 48000
TONE_DB = -6.0
TONE_TOLERANCE_DB = 0.2
# How much of the output is recorded after a resume, and in what pieces it is judged.
RECORD_FRAMES = RATE // 2
PIECE_FRAMES = RATE // 20


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


def is_tone(peaks):
    """Whether peak meter readings in dB are the feeder's tone, on every channel."""
    return len(peaks) == 2 and all(abs(peak - TONE_DB) < TONE_TOLERANCE_DB for peak in peaks)


def tone_fraction(recorded):
    """The fraction of `PIECE_FRAMES` pieces of a S16_LE recording that carry the tone.

    Judged in pieces, like `exact_fraction` in test_alsa_audio.py, so that an xrun on a
    busy VM costs a piece rather than the whole recording. A playback that only writes
    silence fails every piece.
    """
    pieces = len(recorded) // PIECE_FRAMES
    found = 0
    for n in range(pieces):
        piece = recorded[n * PIECE_FRAMES : (n + 1) * PIECE_FRAMES]
        peak = np.abs(piece).max(axis=0)
        with np.errstate(divide="ignore"):
            peaks_db = 20 * np.log10(peak / 2**15)
        if is_tone(list(peaks_db)):
            found += 1
    return found / pieces


class TickWatch:
    """Counts the frames the stall card loses against the Dummy that clocks it.

    See the third trap in loopback.py. A lost period makes the card run slow for a
    moment, which CamillaDSP rightly answers like a slow device, and the level moves.
    A test that saw that cannot tell it from a bug, so `attempts` starts it over.

    Only stretches where both streams are running and moving forward count, so a
    stopped clock, a stall, or a device restarted by CamillaDSP is never taken for one.
    Both positions move a period at a time, and the card's a moment after the clock's,
    so within a stretch the card can be a period behind without having lost anything.
    """

    def __init__(self, direction):
        self.direction = direction
        self.lost = 0
        self._behind = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _positions(self):
        device, stream = (0, "p") if self.direction == "playback" else (1, "c")
        sub = STALL_CABLES[self.direction]
        if pcm_state(0, "p", 0, card="Dummy") != "RUNNING":
            return None
        if pcm_state(device, stream, sub, card=STALL_CARD) != "RUNNING":
            return None
        return hw_ptr(0, "p", 0, card="Dummy"), hw_ptr(device, stream, sub, card=STALL_CARD)

    def _run(self):
        previous = None
        while not self._stop.wait(0.25):
            try:
                current = self._positions()
            except OSError:
                current = None
            if previous is not None and current is not None:
                clock = current[0] - previous[0]
                card = current[1] - previous[1]
                if clock > 0 and card >= 0:
                    self._behind += clock - card
                    period = SETUPS[self.direction].period
                    self.lost = max(self.lost, self._behind - period)
                else:
                    self._behind = 0
            else:
                self._behind = 0
            previous = current

    def stop(self):
        self._stop.set()
        self._thread.join(timeout=5)


class Run:
    """One start of CamillaDSP with the stall card on one end, and what it was given.

    Each attempt of a test gets its own, so a retry starts from nothing.
    """

    def __init__(self, direction, dummy_clock, start_cdsp, alsa_config, feeder):
        self.direction = direction
        self.setup = SETUPS[direction]
        self._dummy_clock = dummy_clock
        self.clock = dummy_clock(self.setup.period)
        self.feed = None
        if direction == "playback":
            self.feed = feeder()
            config = alsa_config(
                playback_device=STALL_PLAYBACK,
                chunksize=self.setup.chunksize,
                devices=RATE_ADJUST,
            )
        else:
            config = alsa_config(
                capture_device=STALL_CAPTURE,
                playback_device=PLAYBACK,
                chunksize=self.setup.chunksize,
                devices=RATE_ADJUST,
            )
        self.cdsp = start_cdsp(config=config)
        # GetRateAdjust is a status snapshot, and this keeps it a tenth of a second old at
        # most rather than a second, as in test_alsa_rate_adjust.py.
        self.cdsp.send("SetUpdateInterval", 100)

    def settle(self):
        """Wait for the level to settle.

        With the playback on the stall card, settled means the level near the target
        and the speed near 1. A speed far from 1 is the controller still recovering from
        the start, and a stall test that began there would measure that. With the
        capture on the stall card rate adjust has nothing to act on, see the module
        docstring, so settled only means the level stopped moving.
        """
        deadline = time.monotonic() + SETTLE_TIMEOUT
        previous = None
        while True:
            level = median_level(self.cdsp, seconds=1.0)
            speed = self.cdsp.send("GetRateAdjust")
            if self.direction == "playback":
                settled = (
                    abs(level - TARGET_LEVEL) < self.setup.level_tolerance
                    and abs(speed - 1) < SPEED_TOLERANCE
                )
            else:
                settled = previous is not None and abs(level - previous) < self.setup.period / 2
            if settled:
                return
            if time.monotonic() > deadline:
                raise AssertionError(
                    f"not settled after {SETTLE_TIMEOUT} s, level {level}, speed {speed}"
                )
            previous = level

    def stop_clock(self):
        """Stop the clock and wait for the device to stand still."""
        self.clock.stop()
        wait_for_hw_ptr(self.direction, moving=False)

    def stall(self):
        """Stop the clock and wait out the stall."""
        self.stop_clock()
        time.sleep(STALL_TIME)

    def resume(self):
        self.clock = self._dummy_clock(self.setup.period)
        wait_for_hw_ptr(self.direction, moving=True)

    def close(self):
        """Stop everything this run started, CamillaDSP before its clock."""
        if self.cdsp.is_running():
            self.cdsp.exit()
        if self.feed is not None:
            self.feed.stop()
        self.clock.stop()


@pytest.fixture
def attempts(dummy_clock, start_cdsp, alsa_config, feeder):
    """Run a test body on a settled `Run`, and start over if the stall card lost time.

    A body that fails while the card kept time fails the test at once. One that fails
    after the card lost periods says nothing about CamillaDSP, see `TickWatch`, and is
    run again on a new `Run`, up to `ATTEMPTS` times.
    """

    def _attempts(direction, body):
        for attempt in range(1, ATTEMPTS + 1):
            watch = TickWatch(direction)
            run = Run(direction, dummy_clock, start_cdsp, alsa_config, feeder)
            try:
                run.settle()
                body(run)
                return
            except (AssertionError, TimeoutError):
                if watch.lost == 0 or attempt == ATTEMPTS:
                    raise
                print(f"attempt {attempt}: the stall card lost {watch.lost} frames, again")
            finally:
                watch.stop()
                run.close()

    return _attempts


def test_a_stalled_capture_shows_in_the_state(attempts):
    """A capture that stops delivering stalls the processing, and the engine says so.

    This is what a GUI shows for a gadget whose host has stopped playing, and it has to
    go back to Running when the host starts again.
    """

    def body(run):
        run.stop_clock()
        run.cdsp.poll_until("GetState", "Stalled")
        run.resume()
        run.cdsp.poll_until("GetState", "Running")

    attempts("capture", body)


def test_nothing_is_adjusted_during_a_playback_stall(attempts):
    """Once the playback is called stalled, neither the level nor the speed moves.

    Nothing is played during the stall, so there is no level to report and no drift to
    correct. A backend that keeps reporting a level while stalled reports one that decays
    to nothing or grows without bound, and the controller follows it to the clamp.

    The time before the stall is called is left out. The device stops taking data before
    it stops accepting it, so the level rises as the buffer fills, and the controller
    answers that like it answers a slow device. That is the right thing to do until it
    is clear that the device is not slow but stopped.
    """

    def body(run):
        run.stop_clock()
        time.sleep(DETECT_TIME)
        level, speed = run.cdsp.send("GetBufferLevel"), run.cdsp.send("GetRateAdjust")
        time.sleep(STALL_TIME)
        level_after = run.cdsp.send("GetBufferLevel")
        speed_after = run.cdsp.send("GetRateAdjust")
        assert (level_after, speed_after) == (level, speed), (
            f"during the stall the level went from {level} to {level_after}, "
            f"and the speed from {speed} to {speed_after}"
        )

    attempts("playback", body)


@pytest.mark.parametrize("direction", ["capture", "playback"])
def test_the_level_comes_back_after_a_stall(attempts, direction):
    """After a stall the level starts at the target again, the way a fresh start does.

    The first two adjust periods after the resume are skipped, since the level
    published then can still hold readings from before the stall.

    The published level is only updated while chunks reach the playback, and holds its
    last value otherwise. A playback that stops taking chunks after the resume would
    keep showing the level from before the stall, close to the target, so with the
    playback on the stall card the test also checks that the tone got through while the
    level was read. With the capture on the stall card the playback gets silence.
    """

    def body(run):
        run.stall()
        run.resume()
        time.sleep(2 * ADJUST_INTERVAL)
        measure_time = 2.0
        level = median_level(run.cdsp, seconds=measure_time)
        if direction == "playback":
            peaks = run.cdsp.send("GetPlaybackSignalPeakSince", measure_time)
            assert is_tone(peaks), (
                f"the playback peak was {peaks} while the level was read, "
                f"no chunks got through after the stall"
            )
        assert abs(level - TARGET_LEVEL) < SETUPS[direction].level_tolerance, (
            f"the level was {level} after the stall, the target is {TARGET_LEVEL}"
        )

    attempts(direction, body)


def test_the_audio_comes_back_after_a_playback_stall(attempts):
    """After a playback stall, what CamillaDSP plays reaches the device again.

    This is the gadget playback whose host closed the microphone stream and opened it
    again. The device moves on silence just as well as on audio, so only a recording of
    its far end shows that the audio came back.
    """

    def body(run):
        run.stall()
        run.resume()
        time.sleep(2 * ADJUST_INTERVAL)
        recorded = decode("S16_LE", record(RECORD_FRAMES, device=STALL_RECORD))
        fraction = tone_fraction(recorded)
        assert fraction >= 0.75, (
            f"only {fraction:.0%} of the output after the stall carried the tone"
        )

    attempts("playback", body)


@pytest.mark.parametrize("direction", ["capture", "playback"])
def test_the_process_exits_while_a_device_is_stalled(attempts, direction):
    """Exit has to get the device threads out of their waits on a device that has stopped.

    A gadget whose host has closed the stream stays stopped until the host comes back,
    and stopping CamillaDSP must not wait for that.
    """

    def body(run):
        run.stop_clock()
        time.sleep(DETECT_TIME)
        assert run.cdsp.exit() == EXIT_OK

    attempts(direction, body)
