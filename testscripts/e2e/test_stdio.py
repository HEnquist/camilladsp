"""The Stdin and Stdout devices, byte for byte.

The one path a user cannot easily check for themselves. A file device leaves something
to look at afterwards, but a pipe leaves nothing, so a stdio device that dropped a chunk
at the seam, added a header nobody asked for, or translated a line ending on Windows
would be found by whoever piped CamillaDSP into a recorder and noticed the result was
wrong, without much to go on.

The suite spawns CamillaDSP with stderr inherited on purpose, so the log still reaches
pytest on a failure while stdout carries audio. `pipe_stdin` and `pipe_stdout` on
`spawn_cdsp` are what switch the two streams over, one at a time.

Windows is the interesting runner here rather than a formality: it is the platform where
a byte stream can be mangled on its way through a handle opened in the wrong mode, and
the file backend has a separate reader implementation there and on macOS anyway, see
`src/file_backend/mod.rs`.
"""

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK, SAMPLERATE, sine_samples
from swdevices import (
    FORMATS,
    encode,
    file_playback,
    raw_capture,
    stdin_capture,
    stdout_playback,
    wav_chunks,
)

pytestmark = pytest.mark.stock

EXIT_OK = 0

FRAMES = 4096
CHANNELS = 2
PLACEHOLDER = 0xFFFFFFFF


def tone(fmt="F64_LE", frames=FRAMES):
    return encode(fmt, sine_samples(frames / SAMPLERATE, channels=CHANNELS).ravel())


def build(config_file, capture, playback):
    return config_file(
        {FILE_CAPTURE: capture, FILE_PLAYBACK: playback}, base="file_devices.yml"
    )


@pytest.mark.parametrize("fmt", FORMATS)
def test_stdin_to_a_file_is_byte_exact(spawn_cdsp, config_file, tmp_path, fmt):
    """Everything fed in on stdin comes back out, in every sample format.

    Parametrized over the whole matrix because `CaptureDeviceStdin` is its own config
    struct and its own branch in the device, so the raw file version of this says
    nothing about it.
    """
    data = tone(fmt)
    destination = str(tmp_path / "out.raw")
    config = build(config_file, stdin_capture(fmt), file_playback(destination, fmt))
    process, _ = spawn_cdsp(config=config, pipe_stdin=True)
    process.communicate(input=data, timeout=30)
    assert process.returncode == EXIT_OK
    assert open(destination, "rb").read() == data


@pytest.mark.parametrize("fmt", FORMATS)
def test_a_file_to_stdout_is_byte_exact(spawn_cdsp, config_file, tmp_path, fmt):
    """And everything read from a file comes back out on stdout.

    `read()` on the pipe runs to EOF, which the process reaches on its own once the
    input file is done, so there is nothing to bound here.
    """
    data = tone(fmt)
    source = str(tmp_path / "in.raw")
    open(source, "wb").write(data)
    config = build(config_file, raw_capture(source, fmt), stdout_playback(fmt))
    process, _ = spawn_cdsp(config=config, pipe_stdout=True)
    written, _ = process.communicate(timeout=30)
    assert process.returncode == EXIT_OK
    assert written == data


def test_stdin_straight_to_stdout_is_byte_exact(spawn_cdsp, config_file):
    """Both ends at once, which is how the devices are actually used.

    `communicate` is what makes this safe: writing 64 kB into a pipe while reading
    another one back would deadlock on the first buffer to fill if either side were done
    in this thread alone.
    """
    data = tone()
    config = build(config_file, stdin_capture(), stdout_playback())
    process, _ = spawn_cdsp(config=config, pipe_stdin=True, pipe_stdout=True)
    written, _ = process.communicate(input=data, timeout=30)
    assert process.returncode == EXIT_OK
    assert written == data


def test_stdin_honours_the_byte_options(spawn_cdsp, config_file, tmp_path):
    """`extra_samples` on a stdin capture, which is the one of the three worth proving.

    A tail of silence has to be added after the pipe closes rather than dropped with it,
    since that is the whole reason the option exists: it is what gives a reverb or a
    filter's ringing somewhere to go.
    """
    data = tone()
    destination = str(tmp_path / "out.raw")
    config = build(
        config_file, stdin_capture(extra_samples=500), file_playback(destination, "F64_LE")
    )
    process, _ = spawn_cdsp(config=config, pipe_stdin=True)
    process.communicate(input=data, timeout=30)
    assert process.returncode == EXIT_OK

    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert len(written) == FRAMES + 500
    assert written[:FRAMES].tobytes() == data
    assert not written[FRAMES:].any()


def test_a_wav_header_on_stdout_keeps_its_placeholder_sizes(
    spawn_cdsp, config_file, tmp_path
):
    """Stdout cannot be seeked, so the sizes in its header stay at the placeholder.

    A file playback goes back and patches them on the way out, and that is not available
    here, so the header is written once and left alone. Worth pinning down because it is
    the difference between the two destinations rather than an oversight: a streaming wav
    with `u32::MAX` lengths is what every tool that reads from a pipe expects, and a
    player given the file on disk will read to the end of the stream instead.
    """
    data = tone("S16_LE")
    source = str(tmp_path / "in.raw")
    open(source, "wb").write(data)
    config = build(
        config_file, raw_capture(source, "S16_LE"), stdout_playback("S16_LE", wav_header=True)
    )
    process, _ = spawn_cdsp(config=config, pipe_stdout=True)
    written, _ = process.communicate(timeout=30)
    assert process.returncode == EXIT_OK

    piped = tmp_path / "piped.wav"
    piped.write_bytes(written)
    layout = wav_chunks(str(piped))
    assert layout["magic"] == "RIFF"
    assert layout["form"] == "WAVE"
    assert layout["riff_size"] == PLACEHOLDER
    assert layout["chunks"]["data"][0] == PLACEHOLDER
    # The audio behind the unpatched header is still all of it.
    assert written[layout["chunks"]["data"][1] :] == data
