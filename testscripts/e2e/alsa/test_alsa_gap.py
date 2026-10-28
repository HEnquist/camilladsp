"""A gap in the data, made by stopping the CamillaDSP process for a moment.

When the process gets no CPU time for a while, both devices keep running without it.
The playback device runs dry and underruns, and the capture device collects what was
played into it. When the process runs again, the playback is restarted with silence up to
the target level, and the backlog from the capture then arrives on top of that silence.
Unless the backend drops it, the level ends up above the target by about the length of
the gap, and rate adjust, which can only move it at half a percent, takes many seconds
to bring it back.

On a real system the gap comes from the scheduling: a busy machine, a VM whose vCPU is
paused, or kernel work that runs late. Here it is a SIGSTOP and a SIGCONT, which gives
the same picture every time. The cards run on kernel timers and keep going while the
process is stopped, and so does the feeder, which is a process of its own.
"""

import os
import signal
import statistics
import time

import pytest

pytestmark = pytest.mark.alsa

# A larger chunk than the suite's usual 1024, for a larger capture buffer. The buffer is
# 8192 frames here, and has to hold the whole gap, or the capture overruns and the
# backlog is lost along with the data.
CHUNKSIZE = 2048
TARGET_LEVEL = 2048
ADJUST_INTERVAL = 0.5
RATE_ADJUST = {
    "enable_rate_adjust": True,
    "target_level": TARGET_LEVEL,
    "adjust_interval_s": ADJUST_INTERVAL,
}
# 5760 frames at 48 kHz. Longer than the 4096 frames the playback device holds at the
# target, so it underruns, and short enough to fit in the capture buffer with a chunk to
# spare.
GAP = 0.12
SETTLE_TIMEOUT = 20.0


def median_level(cdsp, seconds=2.0, interval=0.1):
    """The median of GetBufferLevel over a few adjust periods, see test_alsa_stall.py."""
    values = []
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        values.append(cdsp.send("GetBufferLevel"))
        time.sleep(interval)
    return statistics.median(values)


def wait_for_target(cdsp, tolerance, timeout=SETTLE_TIMEOUT):
    """Wait for the level to come within `tolerance` of the target, and return it."""
    deadline = time.monotonic() + timeout
    while True:
        level = median_level(cdsp, seconds=1.0)
        if abs(level - TARGET_LEVEL) < tolerance:
            return level
        if time.monotonic() > deadline:
            raise AssertionError(
                f"the level was {level} after {timeout} s, the target is {TARGET_LEVEL}"
            )


def test_the_level_comes_back_after_a_gap(start_cdsp, alsa_config, feeder):
    """After a gap the level is back near the target within a few adjust periods.

    The backlog is the length of the gap, close to three chunks. A backend that plays
    it on top of the silence is still more than two chunks above the target when the
    level is read, since rate adjust removes only about 240 frames per second. One that
    drops it is at most a chunk above.

    The first adjust periods after the gap are skipped, since the level published then
    can still hold readings from before the backlog was dropped.
    """
    feeder()
    cdsp = start_cdsp(config=alsa_config(chunksize=CHUNKSIZE, devices=RATE_ADJUST))
    wait_for_target(cdsp, tolerance=CHUNKSIZE / 2)

    os.kill(cdsp.pid, signal.SIGSTOP)
    try:
        time.sleep(GAP)
    finally:
        # A process left stopped would hang the teardown.
        os.kill(cdsp.pid, signal.SIGCONT)

    time.sleep(3 * ADJUST_INTERVAL)
    level = median_level(cdsp)
    assert level < TARGET_LEVEL + 1.5 * CHUNKSIZE, (
        f"the level was {level} after a gap of {GAP} s, the target is {TARGET_LEVEL}"
    )
