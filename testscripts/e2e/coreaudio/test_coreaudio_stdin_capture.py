"""A stdin capture played on CoreAudio, through a start and a restart, without losing a frame.

Stdin has no clock of its own. While data is coming it reads as fast as the pipeline
takes it, so it runs ahead of the playback and keeps the channel to it full. After every
start or restart the playback level is then above the target by the whole channel. With
a capture that has a clock, that would be a backlog that arrived late, and the backend
drops it to get back to the target. Here it is just the stream, which was never late,
and dropping it would cut out a piece of it.

The input is a counter, one value per frame. It is written as float32 values that are
whole multiples of 2^-31, which CamillaDSP and BlackHole carry exactly, so every value
can be checked. Silence may be inserted at a start or after an underrun, but every value
has to come out once and in order. A dropped chunk shows up as a jump in the counter.

The restart is made by pausing the writer for longer than everything buffered between
the pipe and the device lasts, so that the playback runs dry. Stopping the process, as
the ALSA suite does, would not do it here, since that stops the render callback too.
"""

import threading
import time

import numpy as np
import pytest

from .blackhole import NOMINAL_RATE, SINK_CHANNELS, SINK_DEVICE

pytestmark = pytest.mark.coreaudio

EXIT_OK = 0
RATE = NOMINAL_RATE
CHUNKSIZE = 1024
FRAMES = 140 * CHUNKSIZE
SCALE = 2.0**-31
# Well over what the pipe, the channels and the ring buffer hold together.
PAUSE = 1.0
RECORD_SECONDS = 8


def config(tmp_path):
    path = tmp_path / "stdincapture.yml"
    path.write_text(
        "\n".join(
            [
                "devices:",
                f"  samplerate: {RATE}",
                f"  chunksize: {CHUNKSIZE}",
                "  capture:",
                "    type: Stdin",
                "    channels: 2",
                "    format: F32_LE",
                "  playback:",
                "    type: CoreAudio",
                "    channels: 2",
                f'    device: "{SINK_DEVICE}"',
                "",
                "pipeline: []",
                "",
            ]
        )
    )
    return str(path)


def test_no_frames_are_lost_at_a_start_or_a_restart(spawn_cdsp, tmp_path):
    """Every value of the counter comes out once and in order, and the pause made a hole.

    The hole is checked so the test can not pass without a restart having happened.
    """
    import sounddevice

    values = np.arange(1, FRAMES + 1, dtype=np.int64)
    data = np.repeat(values * SCALE, 2).astype("<f4").tobytes()
    half = len(data) // 2 // 8 * 8

    recording = sounddevice.rec(
        RECORD_SECONDS * RATE,
        samplerate=RATE,
        channels=SINK_CHANNELS,
        dtype="float32",
        device=SINK_DEVICE,
    )
    try:
        time.sleep(0.5)
        process, _ = spawn_cdsp(config=config(tmp_path), pipe_stdin=True)

        def write():
            process.stdin.write(data[:half])
            process.stdin.flush()
            time.sleep(PAUSE)
            process.stdin.write(data[half:])
            process.stdin.close()

        writer = threading.Thread(target=write, daemon=True)
        writer.start()
        writer.join(timeout=20)
        assert process.wait(timeout=10) == EXIT_OK
        sounddevice.wait()
    finally:
        sounddevice.stop()

    frames = np.rint(recording[:, :2].astype(np.float64) / SCALE).astype(np.int64)
    assert np.array_equal(frames[:, 0], frames[:, 1]), "the channels differ"
    signal_frames = np.flatnonzero(frames[:, 0])
    out = frames[signal_frames, 0]
    assert len(out) > 0 and out[0] == 1, "the stream does not start with the first frame"
    # The highest value so far has to grow one at a time, a dropped chunk makes it jump.
    # Comparing against the highest value rather than the previous one keeps this the same
    # check as the ALSA suite's, where the device can repeat a few old frames.
    highest = np.maximum.accumulate(out)
    jumps = np.flatnonzero(np.diff(highest) > 1)
    assert len(jumps) == 0, (
        f"frames were dropped, the counter jumps at "
        f"{[(int(highest[i]), int(highest[i + 1])) for i in jumps[:5]]}"
    )
    assert highest[-1] > len(values) * 3 // 4, (
        f"only {highest[-1]} of {len(values)} frames came out"
    )
    holes = np.flatnonzero(np.diff(signal_frames) > 1)
    assert len(holes) > 0, "the playback did not restart during the pause"
