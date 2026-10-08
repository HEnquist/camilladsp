"""Rate adjust on WASAPI and ASIO, against a Dummy device with a drifting clock.

The cable has one clock and no pitch control, so the second clock comes from a Dummy
device, whose `drift` the test sets through its control socket. There are two shapes:

- A Dummy capture into WASAPI or ASIO playback. The real playback device measures its
  buffer and runs the controller, and the correction goes to the Dummy capture.
- WASAPI or ASIO capture into a Dummy playback. The Dummy playback runs the controller,
  and the correction goes to the real capture device.

Both run with an asynchronous resampler at 48 kHz on both sides, which is where the
correction lands on a real capture device, since its clock is not ours to change.

The cable's clock is not exactly the Dummy's, so the tests assert that switching the drift
on moves the correction by the drift, with the right sign, rather than to a fixed value.
The loop answers scheduling jitter the way it answers drift, so every reading is a median
of several, and a stall on a busy runner can upset it for a minute, see
`assert_follows_drift` for how the measurement survives that.
"""

import statistics
import time

import pytest
from dummyctl import Control

from .cable import (
    asio_block,
    capture_endpoint,
    dummy_block,
    free_port,
    render_endpoint,
    wait_for_peak,
    wasapi_block,
)

pytestmark = [pytest.mark.wasapi, pytest.mark.usefixtures("dummy_backend")]

DEVICES = {
    "enable_rate_adjust": "true",
    "target_level": 2048,
    # Short compared to the 10 s default, see dummy_rate.yml for why that is faster.
    "adjust_interval_s": 0.5,
    "resampler": "{type: AsyncPoly, interpolation: Cubic}",
}
CLAMP_PPM = 5000
# Well inside the controller's clamp, and far above what the loop wanders by.
DRIFT_PPM = 2000
# How far from the expected shift the correction may settle. Loose against a busy runner
# and the cable's noisy clock, still tight against a wrong sign or a factor of two.
PPM_TOLERANCE = 0.3 * DRIFT_PPM
SETTLE_TIMEOUT = 60.0
# How long the drift is held on or off before the next reading. A drift step settles within
# a few adjust intervals, through the proportional term.
STEP_HOLD = 8.0
# How long to keep stepping the drift before giving up on a clean reading. Long enough to
# outlast a stall that pins the loop at the clamp for most of a minute.
MEASURE_TIMEOUT = 180.0


def median_ppm(cdsp, readings=10, interval=0.2):
    """The correction in ppm, as the median of a few readings."""
    values = []
    for _ in range(readings):
        values.append((cdsp.send("GetRateAdjust") - 1.0) * 1e6)
        time.sleep(interval)
    return statistics.median(values)


def wait_for_adjust_to_start(cdsp, timeout=20.0):
    """The adjust reads exactly 0.0 until the first SetSpeed, see test_rate_control.py."""
    cdsp.poll_until_true("GetRateAdjust", lambda speed: speed > 0.5, timeout=timeout)


def wait_until_steady(cdsp, spread=150.0, timeout=SETTLE_TIMEOUT):
    """Wait for two medians in a row to agree within `spread` ppm.

    Only a gate past the startup transient, so the measurement does not spend its time on
    it. Two medians also agree at the top of a slow swing, so this is no baseline.
    """
    deadline = time.monotonic() + timeout
    previous = median_ppm(cdsp)
    while True:
        current = median_ppm(cdsp)
        if abs(current - previous) < spread:
            return
        if time.monotonic() > deadline:
            raise AssertionError(f"the correction was still moving after {timeout} s")
        previous = current


def assert_follows_drift(cdsp, port, drift, expected):
    """Switch the drift on and off until a reading shows the correction moving by `expected`.

    A stall on a busy runner throws the buffer level off, and the integrator carries that
    as a tail that can take a minute to decay, much longer than a drift step takes to
    settle. A single baseline read before the drift is at its mercy. So the drift is
    switched on and off, and each reading is compared against the mean of its neighbours,
    which cancels a slow tail. A disturbance spoils the few readings around it, while a
    wrong sign or a wrong magnitude spoils every one. Two steps in a row have to agree, so
    a disturbance cannot fake a pass on its own either.
    """
    control = Control(port)
    control.wait_until_ready()
    deadline = time.monotonic() + MEASURE_TIMEOUT
    readings = []
    shifts = []
    passed_before = False
    while True:
        # The drift is off for the first reading, so it is on for every odd one.
        readings.append(median_ppm(cdsp))
        if len(readings) >= 3:
            before, middle, after = readings[-3:]
            neighbours = (before + after) / 2
            if len(readings) % 2 == 1:
                shift = middle - neighbours
            else:
                shift = neighbours - middle
            shifts.append(shift)
            # A step that runs into the clamp is cut short, and says nothing about the loop.
            unclamped = max(abs(before), abs(middle), abs(after)) < 0.95 * CLAMP_PPM
            passed = unclamped and abs(shift - expected) < PPM_TOLERANCE
            if passed and passed_before:
                return
            passed_before = passed
        if time.monotonic() > deadline:
            seen = ", ".join(f"{shift:.0f}" for shift in shifts)
            raise AssertionError(
                f"no step moved the correction by {expected:.0f} ppm in {MEASURE_TIMEOUT} s, "
                f"the shifts were {seen}"
            )
        control.set("drift", drift if len(readings) % 2 == 1 else 0)
        time.sleep(STEP_HOLD)


def real_block(backend, side):
    if backend == "asio":
        return asio_block(side)
    device = capture_endpoint()[1] if side == "capture" else render_endpoint()[1]
    return wasapi_block(side, device)


@pytest.fixture(params=["wasapi", "asio"])
def backend(request):
    if request.param == "asio":
        request.getfixturevalue("steinberg")
    return request.param


@pytest.mark.parametrize("drift", [DRIFT_PPM, -DRIFT_PPM])
def test_a_real_playback_follows_a_drifting_dummy_capture(start_cdsp, win_config, backend, drift):
    """A capture running fast has to be slowed down, so the correction goes the other way."""
    port = free_port()
    config = win_config(
        capture=dummy_block("capture", port),
        playback=real_block(backend, "playback"),
        devices=DEVICES,
    )
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    wait_for_adjust_to_start(cdsp)
    wait_until_steady(cdsp)
    assert_follows_drift(cdsp, port, drift, -drift)


@pytest.mark.parametrize("drift", [DRIFT_PPM, -DRIFT_PPM])
def test_a_real_capture_follows_a_drifting_dummy_playback(
    start_cdsp, win_config, feeder, backend, drift
):
    """A playback running fast has to be fed faster, so the correction goes with it."""
    feeder()
    port = free_port()
    config = win_config(
        capture=real_block(backend, "capture"),
        playback=dummy_block("playback", port),
        devices=DEVICES,
    )
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    wait_for_adjust_to_start(cdsp)
    wait_until_steady(cdsp)
    assert_follows_drift(cdsp, port, drift, drift)
