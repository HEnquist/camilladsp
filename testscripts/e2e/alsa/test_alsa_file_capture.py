"""A file capture played on ALSA, through a start and a restart, without losing a frame.

A file capture has no clock of its own. It reads as fast as the pipeline takes the data,
so it runs ahead of the playback and keeps the channel to it full. After every start
or restart the playback level is then above the target by the whole channel. With a
capture that has a clock, that would be a backlog that arrived late, and the backend
drops it to get back to the target. Here it is just the stream, which was never late,
and dropping it would cut out a piece of the file.

The input is a counter, one value per frame, which the 32 bit loopback carries exactly.
Silence may be inserted at a start or after an underrun, but every value has to come
out once and in order. A dropped chunk shows up as a jump in the counter.

The restart is made the same way as in test_alsa_gap.py, by stopping the process for
a moment so that the playback underruns.
"""

import os
import signal
import subprocess
import time

import numpy as np
import pytest

from .loopback import ALSA_FORMAT, PLAYBACK, RECORD, decode, encode

pytestmark = pytest.mark.alsa

EXIT_OK = 0
RATE = 48000
FMT = "S32_LE"
CHUNKSIZE = 1024
# Three seconds, in whole chunks so that the last one is not padded.
FRAMES = 140 * CHUNKSIZE
# Longer than the target level plus a chunk, so the playback underruns.
GAP = 0.12
RECORD_SECONDS = 7


def write_counter(path):
    """A file whose frame n holds n + 1 on both channels, and the values."""
    values = np.arange(1, FRAMES + 1, dtype=np.int64)
    path.write_bytes(encode(FMT, np.repeat(values, 2)))
    return values


def config(tmp_path, filename):
    path = tmp_path / "filecapture.yml"
    path.write_text(
        "\n".join(
            [
                "devices:",
                f"  samplerate: {RATE}",
                f"  chunksize: {CHUNKSIZE}",
                "  capture:",
                "    type: RawFile",
                "    channels: 2",
                f"    filename: {filename}",
                f"    format: {FMT}",
                "  playback:",
                "    type: Alsa",
                "    channels: 2",
                f'    device: "{PLAYBACK}"',
                f"    format: {FMT}",
                "",
                "pipeline: []",
                "",
            ]
        )
    )
    return str(path)


def start_recorder(frames, path):
    """Start recording the playback cable to `path`, before CamillaDSP opens the other end.

    It records to a file rather than a pipe, since nothing reads a pipe until the end,
    and arecord would block on it as soon as it was full and miss the run.
    """
    return subprocess.Popen(
        [
            "arecord", "-q", "-D", RECORD, "-t", "raw",
            "-f", ALSA_FORMAT[FMT], "-r", str(RATE), "-c", "2",
            "-s", str(frames), str(path),
        ],
        stderr=subprocess.PIPE,
    )


def test_no_frames_are_lost_at_a_start_or_a_restart(spawn_cdsp, tmp_path):
    """Every value of the counter comes out once and in order, and the gap made a hole.

    The hole is checked so the test can not pass without a restart having happened.
    """
    values = write_counter(tmp_path / "counter.raw")
    recording = tmp_path / "recorded.raw"
    recorder = start_recorder(RECORD_SECONDS * RATE, recording)
    try:
        # Give arecord the time to open the cable and lock its format.
        time.sleep(0.5)
        process, _ = spawn_cdsp(config=config(tmp_path, tmp_path / "counter.raw"))
        time.sleep(1.0)
        os.kill(process.pid, signal.SIGSTOP)
        try:
            time.sleep(GAP)
        finally:
            # A process left stopped would hang the teardown.
            os.kill(process.pid, signal.SIGCONT)
        assert process.wait(timeout=10) == EXIT_OK
        _, errors = recorder.communicate(timeout=RECORD_SECONDS + 10)
    finally:
        if recorder.poll() is None:
            recorder.kill()
            recorder.wait()
    assert recorder.returncode == 0, errors.decode().strip()

    frames = decode(FMT, recording.read_bytes())
    assert np.array_equal(frames[:, 0], frames[:, 1]), "the channels differ"
    signal_frames = np.flatnonzero(frames[:, 0])
    out = frames[signal_frames, 0]
    assert len(out) > 0 and out[0] == 1, "the stream does not start with the first frame"
    # The loopback goes on playing what is left in its buffer for a moment after the
    # underrun, so a short run of old values comes out again before the silence. Those
    # are repeats, never new values, so the highest value so far has to grow one at a
    # time. A dropped chunk makes it jump. The values must also get well past the gap.
    # The tail is not checked, since how the end of the stream is played is not what
    # this test is about.
    highest = np.maximum.accumulate(out)
    jumps = np.flatnonzero(np.diff(highest) > 1)
    assert len(jumps) == 0, (
        f"frames were dropped, the counter jumps at "
        f"{[(int(highest[i]), int(highest[i + 1])) for i in jumps[:5]]}"
    )
    assert highest[-1] > len(values) // 2, f"only {highest[-1]} of {len(values)} frames came out"
    holes = np.flatnonzero(np.diff(signal_frames) > 1)
    assert len(holes) > 0, "the playback did not restart during the gap"
