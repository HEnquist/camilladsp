"""Rate adjust on WASAPI and ASIO, against a Dummy device with a drifting clock.

The cable has one clock and no pitch control, so the second clock comes from a Dummy
device, whose `drift` the test sets through its control socket. There are two shapes:

- A Dummy capture into WASAPI or ASIO playback. The real playback device measures its
  buffer and runs the controller, and the correction goes to the Dummy capture.
- WASAPI or ASIO capture into a Dummy playback. The Dummy playback runs the controller,
  and the correction goes to the real capture device.

Both run with an asynchronous resampler at 48 kHz on both sides, which is where the
correction lands on a real capture device, since its clock is not ours to change.

The cable's clock is not exactly the Dummy's. So each test settles first with no drift,
takes that as the baseline, and then asserts that setting the drift moves the correction
by the drift, with the right sign. The loop answers scheduling jitter the way it answers
drift, so settling is judged on medians of several readings.
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
# Well inside the controller's 5000 ppm clamp, and far above what the loop wanders by.
DRIFT_PPM = 2000
# How far from the expected shift the correction may settle. Loose against a busy runner
# and the cable's noisy clock, still tight against a wrong sign or a factor of two.
PPM_TOLERANCE = 0.3 * DRIFT_PPM
SETTLE_TIMEOUT = 60.0


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
    """Wait for two medians in a row to agree within `spread` ppm, and return the last."""
    deadline = time.monotonic() + timeout
    previous = median_ppm(cdsp)
    while True:
        current = median_ppm(cdsp)
        if abs(current - previous) < spread:
            return current
        if time.monotonic() > deadline:
            raise AssertionError(f"the correction was still moving after {timeout} s")
        previous = current


def wait_for_ppm(cdsp, expected, timeout=SETTLE_TIMEOUT):
    """Wait for the median correction to settle near `expected` ppm, and return it."""
    deadline = time.monotonic() + timeout
    while True:
        value = median_ppm(cdsp)
        if abs(value - expected) < PPM_TOLERANCE:
            return value
        if time.monotonic() > deadline:
            raise AssertionError(
                f"the correction was {value:.0f} ppm after {timeout} s, not near {expected:.0f}"
            )


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
    baseline = wait_until_steady(cdsp)
    control = Control(port)
    control.wait_until_ready()
    control.set("drift", drift)
    wait_for_ppm(cdsp, baseline - drift)


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
    baseline = wait_until_steady(cdsp)
    control = Control(port)
    control.wait_until_ready()
    control.set("drift", drift)
    wait_for_ppm(cdsp, baseline + drift)
