"""Audio through the PipeWire backend on both ends: rates, silence, and what comes out.

Everything else in the suite reaches the engine through the Dummy or the software
devices, so this is the only place the PipeWire capture and playback code runs at all:
the stream setup, the process callbacks, and the ring buffers between them and the
device threads.

PipeWire is float32 throughout and the CamillaDSP nodes always ask for F32, so with an
empty pipeline at the graph rate what CamillaDSP plays should be exactly what the
feeder played. That is a stronger check than a level, since it catches a channel
swapped or a frame repeated at a callback boundary, neither of which moves a peak meter.
"""

import statistics
import time

import pytest

from .graph import NOMINAL_RATE, exact_fraction, noise, record

pytestmark = pytest.mark.pipewire

RATE = NOMINAL_RATE
LEVEL_DB = -6.0


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    """Wait for both channels of a peak meter to read `level`, and return them."""
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.2 for peak in peaks),
        timeout=timeout,
    )


def test_audio_passes_through_bit_exact(start_cdsp, pw_config, feeder):
    """A second of noise in a loop, and the output has to match it exactly.

    Matched in 50 ms pieces rather than as one stretch, see `exact_fraction`: a dropout
    on a busy runner is a gap between pieces, not a wrong sample, and is not what this
    is about. A wrong conversion fails every piece, so most of them matching is the
    whole claim.
    """
    block = noise(RATE)
    feeder(block=block)
    start_cdsp(config=pw_config())
    recorded = record(RATE * 2)
    # The last second, well clear of whatever the start of the recording caught.
    window = recorded[-RATE:]
    assert exact_fraction(block, window, RATE // 20) >= 0.75


@pytest.mark.parametrize("rate", [44100, 96000])
def test_other_sample_rates(start_cdsp, pw_config, feeder, rate):
    """A config at another rate than the graph's, and the audio still gets through.

    The graph stays at 48 kHz and the adapter in front of each CamillaDSP stream
    resamples, so the feeder keeps playing at 48 kHz. The measured rate is the stream's,
    and has to agree with the config. The chunk size scales with the rate, so every case
    has the same chunk duration and the same headroom.
    """
    feeder()
    cdsp = start_cdsp(config=pw_config(samplerate=rate, chunksize=1024 * rate // RATE))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.poll_until_true(
        "GetCaptureRate", lambda measured: abs(measured - rate) < 0.005 * rate, timeout=10.0
    )


def test_the_audio_keeps_flowing(start_cdsp, pw_config, feeder):
    """Started and still going, rather than falling over after the first few callbacks."""
    feeder()
    cdsp = start_cdsp(config=pw_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    time.sleep(3.0)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


def test_a_silent_source_pauses_and_resumes(start_cdsp, pw_config, feeder):
    """With nothing playing into it the feed sink's monitor delivers zeros, which is silence.

    The silence detection is in each backend, next to the source, so that a paused
    session skips the resampler too, and this is the PipeWire copy of it. A feeder
    starting again has to bring the session back without anything being restarted.
    """
    config = pw_config(devices={"silence_threshold": -60.0, "silence_timeout_s": 0.5})
    cdsp = start_cdsp(config=config, wait_for_running=False)
    cdsp.poll_until("GetState", "Paused", timeout=10.0)
    feeder()
    cdsp.poll_until("GetState", "Running", timeout=10.0)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")


def test_rate_adjust_stays_near_nominal(start_cdsp, pw_config, feeder):
    """With rate adjust on, the speed the playback asks the capture for stays near 1.

    The playback reports its buffer level and the capture's async resampler takes the
    speed, which is the whole of rate adjust in this backend. Both ends run on one
    driver, so there is no drift to correct, and a loop that wanders off anyway has a
    wrong sign or a wrong buffer level in it, and ends up at the 0.5 % clamp. Judged on a
    median, as in the ALSA rate tests, since a single reading can catch the loop
    answering a scheduling hiccup.

    The 1 s interval is on purpose, unlike the 0.2 s in the convergence tests. The
    output is relative to the frames in one interval, so at 0.2 s a dip of 200 frames in
    one period's average, a fifth of a chunk, already moves the speed by 0.4 %. This
    test is about where the loop sits, not how fast it gets there. The speed is only
    published once per status update, so the readings span a few seconds to get a
    handful of distinct values.
    """
    config = pw_config(
        devices={
            "enable_rate_adjust": True,
            "target_level": 2048,
            "adjust_interval_s": 1.0,
            "resampler": "{type: AsyncPoly, interpolation: Cubic}",
        }
    )
    feeder()
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.poll_until_true("GetRateAdjust", lambda speed: speed > 0.5, timeout=10.0)
    time.sleep(2.0)
    readings = []
    for _ in range(10):
        readings.append(cdsp.send("GetRateAdjust"))
        time.sleep(0.5)
    assert abs(statistics.median(readings) - 1.0) < 0.002
    assert cdsp.send("GetState") == "Running"
