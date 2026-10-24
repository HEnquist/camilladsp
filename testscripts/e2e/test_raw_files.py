"""The raw file devices, on a stock build, asserted byte for byte.

Everything else in the suite runs against a `dummy-backend` build, which is not the
binary anyone ships. These run against a stock one, and what they cover is the path a
release has and the dummy suites cannot reach: the file capture and playback devices,
every `BinarySampleFormat`, and the three options that decide which bytes of an input
file are read.

The assertion is an identity rather than a level. A raw file in, an empty pipeline, and a
raw file out in the same format means nothing along the way is allowed to change a single
byte, so the comparison is exact and a one bit conversion error is as visible as a silent
pipeline. `test_file_devices.py` proves the same for F64_LE on a dummy build and spends
its effort on the pacing instead.

The input is a -6 dBFS sine, well inside full scale, so no format in the matrix can clip
and the round trip is the conversion alone.
"""

import os
import time

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK, SAMPLERATE, sine_samples
from swdevices import BYTES_PER_SAMPLE, FORMATS, decode, encode, file_playback, raw_capture

pytestmark = pytest.mark.stock

EXIT_OK = 0

FRAMES = 4096
CHANNELS = 2


def write_input(tmp_path, fmt, frames=FRAMES, name="in.raw"):
    """Write the test tone as `fmt`, and return the path, the bytes and the samples."""
    samples = sine_samples(frames / SAMPLERATE, channels=CHANNELS)
    data = encode(fmt, samples.ravel())
    path = tmp_path / name
    path.write_bytes(data)
    return str(path), data, samples


def wait_for_size(path, size, timeout=10.0):
    """Wait until a playback file holds exactly `size` bytes.

    What a restarted session writes is the thing to watch, not the stop reason: the
    reason still holds the previous session's `Done` for as long as it takes the new one
    to start, so a poll for anything other than `None` is satisfied by the stale value
    before the reload has done anything at all.
    """
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if os.path.exists(path) and os.path.getsize(path) == size:
            return
        time.sleep(0.02)
    got = os.path.getsize(path) if os.path.exists(path) else 0
    raise TimeoutError(f"{path} held {got} bytes after {timeout} s, wanted {size}")


def run_to_completion(spawn_cdsp, config, timeout=30):
    """Run a file to file config until the input runs out, and assert it ended cleanly.

    Without `--wait` the supervisor finds no config and no queued command once the
    session ends, so the process exits on its own. Nothing here needs the websocket, and
    not opening one keeps a matrix of seven formats to a couple of seconds.
    """
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=timeout) == EXIT_OK


@pytest.mark.parametrize("fmt", FORMATS)
def test_a_round_trip_through_every_sample_format_is_bit_exact(
    spawn_cdsp, config_file, tmp_path, fmt
):
    """Reading a format and writing it back may not change a byte.

    This is the strongest statement the suite can make about the conversions in
    `src/utils/conversions.rs`, and it is only available here: both ends are files, so
    there is something to compare against, and neither end resamples, so the comparison
    is an identity. A scale factor off by one LSB, a byte order slip or a 24 bit
    justification mixed up all fail it.
    """
    source, data, _ = write_input(tmp_path, fmt)
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, fmt),
            FILE_PLAYBACK: file_playback(destination, fmt),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    written = open(destination, "rb").read()
    assert len(written) == len(data)
    assert written == data


@pytest.mark.parametrize("fmt", ["S24_4_RJ_LE", "S24_4_LJ_LE"])
def test_the_padding_byte_of_a_four_byte_24_bit_sample_is_written_as_zero(
    spawn_cdsp, config_file, tmp_path, fmt
):
    """Which byte of a 4 byte 24 bit sample is padding, and what goes in it.

    `S24_4_RJ_LE` keeps the data in the low three bytes and `S24_4_LJ_LE` in the high
    three, and CamillaDSP writes the remaining byte as zero rather than sign extending
    it. Worth pinning down because the two layouts are otherwise indistinguishable from
    a level measurement, and because a reader that expects a sign extended 32 bit word
    would disagree about every negative sample.
    """
    source, _, _ = write_input(tmp_path, fmt)
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, fmt),
            FILE_PLAYBACK: file_playback(destination, fmt),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    words = np.frombuffer(open(destination, "rb").read(), dtype="u1").reshape(-1, 4)
    padding = words[:, 3] if fmt == "S24_4_RJ_LE" else words[:, 0]
    assert np.count_nonzero(padding) == 0
    # And the rest is not all zero, or the assertion above would hold on silence.
    data_bytes = words[:, :3] if fmt == "S24_4_RJ_LE" else words[:, 1:]
    assert np.count_nonzero(data_bytes) > 0


def test_the_padding_byte_is_ignored_on_the_way_in(spawn_cdsp, config_file, tmp_path):
    """A right justified input whose padding is sign extended reads the same as a zeroed one.

    The other half of the pair above. CamillaDSP writes zero there, but files from
    elsewhere carry a sign extension, and the two have to produce identical audio or
    every negative sample from such a file would come back as a large positive one.
    """
    samples = sine_samples(FRAMES / SAMPLERATE, channels=CHANNELS)
    zeroed = encode("S24_4_RJ_LE", samples.ravel())
    words = np.frombuffer(zeroed, dtype="u1").reshape(-1, 4).copy()
    # Sign extend into the padding byte, which is what a file from other software has.
    words[:, 3] = np.where(words[:, 2] >= 0x80, 0xFF, 0x00)
    assert np.count_nonzero(words[:, 3]) > 0, "the tone has no negative samples to extend"

    source = str(tmp_path / "extended.raw")
    open(source, "wb").write(words.tobytes())
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "S24_4_RJ_LE"),
            FILE_PLAYBACK: file_playback(destination, "S24_4_RJ_LE"),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    assert open(destination, "rb").read() == zeroed


@pytest.mark.parametrize("fmt", ["S16_LE", "S24_3_LE", "S32_LE", "F32_LE"])
def test_a_conversion_to_a_narrower_format_stays_within_one_step(
    spawn_cdsp, config_file, tmp_path, fmt
):
    """F64 in and a narrower format out has to quantize, and nothing else.

    The identity tests above cannot see a scale factor that is wrong by the same amount
    in both directions, since it would cancel. This one reads the output on its own
    terms and compares it against the float input, so the two conversions are pinned
    separately rather than only as a pair.
    """
    source, _, samples = write_input(tmp_path, "F64_LE")
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, fmt),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    written = decode(fmt, open(destination, "rb").read(), channels=CHANNELS)
    assert written.shape == samples.shape
    step = 2.0 ** (1 - 8 * BYTES_PER_SAMPLE[fmt]) if fmt != "F32_LE" else 2.0**-23
    assert np.abs(written - samples).max() <= step


def test_the_whole_input_is_written_and_nothing_more(spawn_cdsp, config_file, tmp_path):
    """No truncation before EOF, and no tail unless one was asked for.

    A capture that gives up a chunk early, or a playback that drops the last partial
    chunk, both show up here and nowhere else: every other test in the suite stops the
    run itself and cannot tell a short output from an early stop.
    """
    source, data, _ = write_input(tmp_path, "F64_LE")
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)
    assert os.path.getsize(destination) == len(data)


@pytest.mark.parametrize("extra_samples", [1, 500, 5000])
def test_extra_samples_lengthens_the_output_by_exactly_that_many_frames(
    spawn_cdsp, config_file, tmp_path, extra_samples
):
    """`extra_samples` is a tail of silence, and its length is exact.

    5000 is over four chunks, so the tail spans more than one send and the count is not
    simply whatever one chunk happened to hold.
    """
    source, data, _ = write_input(tmp_path, "F64_LE")
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE", extra_samples=extra_samples),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert len(written) == FRAMES + extra_samples
    # The audio ahead of the tail is untouched, and the tail itself is silence.
    assert written[:FRAMES].tobytes() == data
    assert not written[FRAMES:].any()


@pytest.mark.parametrize(
    "skip_bytes, read_bytes, expected_frames",
    [
        (1600, None, FRAMES - 100),
        (None, 8000, 500),
        (1600, 8000, 500),
        (None, 10**9, FRAMES),
    ],
)
def test_skip_bytes_and_read_bytes_select_a_window_of_the_input(
    spawn_cdsp, config_file, tmp_path, skip_bytes, read_bytes, expected_frames
):
    """The two byte counts pick out a window, and `read_bytes` counts from after the skip.

    A `read_bytes` larger than the file is the case worth having: it has to run out at
    EOF rather than wait for bytes that never come, which is what a user pointing it at
    a pipe would hit.
    """
    source, data, _ = write_input(tmp_path, "F64_LE")
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(
                source, "F64_LE", skip_bytes=skip_bytes, read_bytes=read_bytes
            ),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    run_to_completion(spawn_cdsp, config)

    written = open(destination, "rb").read()
    assert len(written) == expected_frames * CHANNELS * 8
    start = skip_bytes or 0
    assert written == data[start : start + len(written)]


def test_a_stream_that_runs_out_sets_done(start_cdsp, config_file, tmp_path):
    """A file capture that reaches EOF is a normal end, the same as a dummy one told to stop.

    `test_failures.py` covers this with the dummy capture, where the end of the stream
    comes from the control socket. Here it comes from the data running out, which is the
    only way a user ever reaches it, and the capture device on the ready barrier is a
    different one. The reason has to survive the teardown either way, see
    `StopReason::None` at `src/engine.rs:235`.
    """
    source, _, _ = write_input(tmp_path, "F64_LE", frames=SAMPLERATE // 4)
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    assert cdsp.poll_until_true("GetStopReason", lambda reason: reason != "None") == "Done"
    assert cdsp.poll_until("GetState", "Inactive")
    assert cdsp.exit() == EXIT_OK


def test_a_reload_reopens_the_input_from_the_start(start_cdsp, config_file, tmp_path):
    """A session whose capture ran out has to start over on a reload, not resume at EOF.

    The devices are rebuilt from the config, so the file is opened again and the whole
    input is read a second time. A device that held its file handle across the restart
    would report `Done` straight away with nothing written, which is what this separates
    from a real second pass by checking the output rather than the reason alone.

    What this deliberately does not assert is the stop reason going back to `None` in
    between, the way the dummy version in `test_failures.py` does. A file to file run is
    hundreds of times real time, so the new session is over before a poll can see it
    start, and the plan's note about a startup delay on a device is what that would need.
    """
    source, data, _ = write_input(tmp_path, "F64_LE", frames=SAMPLERATE // 4)
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    for _ in range(3):
        wait_for_size(destination, len(data))
        assert open(destination, "rb").read() == data
        os.remove(destination)
        cdsp.send("Reload")
    assert cdsp.exit() == EXIT_OK
