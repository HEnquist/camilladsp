"""A stdin capture played on WASAPI, through a start and a restart, without losing audio.

Stdin has no clock of its own. While data is coming it reads as fast as the pipeline
takes it, so it runs ahead of the playback and keeps the channel to it full. After every
start or restart the playback level is then above the target by the whole channel. With
a capture that has a clock, that would be a backlog that arrived late, and the backend
drops it to get back to the target. Here it is just the stream, which was never late,
and dropping it would cut out a piece of it.

The cable is not bit exact, so this can not follow single frames the way the ALSA and
CoreAudio suites do. It measures how long the tone lasts instead. Dropping the backlog
after a restart removes more than a second here, far more than the cable's timing
spreads it.

The restart is made by pausing the writer for longer than everything buffered between
the pipe and the device lasts, so that the playback runs dry.
"""

import threading
import time

import numpy as np
import pytest

from .cable import (
    CHANNELS,
    EXIT_OK,
    LEVEL_DB,
    RATE,
    TONE_HZ,
    record,
    render_endpoint,
    wasapi_block,
)

pytestmark = pytest.mark.wasapi

TONE_SECONDS = 3.0
# Well over what the pipe, the channels and the ring buffer hold together.
PAUSE = 1.0
RECORD_SECONDS = 8
BLOCK = RATE // 100
TOLERANCE = 0.25


def stdin_block():
    return ["  capture:", "    type: Stdin", f"    channels: {CHANNELS}", "    format: F32_LE"]


def tone_seconds(data):
    """How long the tone is on, in 10 ms blocks whose RMS is near the tone's."""
    column = data[: len(data) // BLOCK * BLOCK, 0].reshape(-1, BLOCK)
    rms = np.sqrt(np.mean(column.astype(np.float64) ** 2, axis=1))
    expected = 10 ** (LEVEL_DB / 20) / np.sqrt(2)
    return np.count_nonzero(rms > expected / 2) * BLOCK / RATE


def test_no_audio_is_lost_at_a_start_or_a_restart(spawn_cdsp, win_config):
    """The tone comes out as long as it went in, with a hole where the writer paused.

    The hole is checked so the test can not pass without a restart having happened.
    """
    frames = int(TONE_SECONDS * RATE)
    tone = 10 ** (LEVEL_DB / 20) * np.sin(2 * np.pi * TONE_HZ / RATE * np.arange(frames))
    data = np.repeat(tone, CHANNELS).astype("<f4").tobytes()
    half = len(data) // 2 // (4 * CHANNELS) * (4 * CHANNELS)

    recorded = {}
    recorder = threading.Thread(
        target=lambda: recorded.update(data=record(RECORD_SECONDS * RATE)), daemon=True
    )
    recorder.start()
    time.sleep(0.5)
    config = win_config(
        capture=stdin_block(), playback=wasapi_block("playback", render_endpoint()[1])
    )
    process, _ = spawn_cdsp(config=config, pipe_stdin=True)
    process.stdin.write(data[:half])
    process.stdin.flush()
    time.sleep(PAUSE)
    process.stdin.write(data[half:])
    process.stdin.close()
    assert process.wait(timeout=20) == EXIT_OK
    recorder.join(timeout=RECORD_SECONDS + 10)

    out = recorded["data"]
    seconds = tone_seconds(out)
    assert abs(seconds - TONE_SECONDS) < TOLERANCE, (
        f"the tone lasted {seconds:.2f} s, {TONE_SECONDS} s went in"
    )
    column = out[: len(out) // BLOCK * BLOCK, 0].reshape(-1, BLOCK)
    on = np.flatnonzero(np.sqrt(np.mean(column.astype(np.float64) ** 2, axis=1)) > 0.1)
    assert np.any(np.diff(on) > PAUSE / 4 * 100), "the playback did not restart during the pause"
