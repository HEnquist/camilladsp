"""Wav in and wav out, on a stock build.

`src/utils/wavtools.rs` has unit tests for the sink on its own, which cover the header
bytes a few samples at a time. What they cannot cover is a whole run: the header is
written before a single chunk exists and its sizes are patched on the way out, so
whether a file is readable afterwards depends on the playback device finishing properly
rather than on the header writer being right.

So the round trip is the test. CamillaDSP writes the wav, CamillaDSP reads it back, and
the samples that come out the far side are compared against the ones that went in. A
header with the wrong channel count, rate or data length fails the second half even
though the first half wrote every byte it meant to. The header fields are asserted
directly as well, because a file only this program can read is not a wav file.

RF64 is the same round trip with the sizes deliberately left at their placeholders and
the real lengths in a `ds64` chunk, which is what lets the format past the 4 GB ceiling
that `MAX_WAV_DATA_BYTES` enforces on a plain wav.
"""

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK, SAMPLERATE, sine_samples
from swdevices import (
    BYTES_PER_SAMPLE,
    encode,
    file_playback,
    raw_capture,
    wav_capture,
    wav_chunks,
)

pytestmark = pytest.mark.stock

EXIT_OK = 0

FRAMES = 4096
CHANNELS = 2
PLACEHOLDER = 0xFFFFFFFF


def write_input(tmp_path):
    samples = sine_samples(FRAMES / SAMPLERATE, channels=CHANNELS)
    data = encode("F64_LE", samples.ravel())
    path = tmp_path / "in.raw"
    path.write_bytes(data)
    return str(path), data


def run(spawn_cdsp, config_file, capture, playback, timeout=30):
    config = config_file(
        {FILE_CAPTURE: capture, FILE_PLAYBACK: playback}, base="file_devices.yml"
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=timeout) == EXIT_OK


@pytest.mark.parametrize("fmt", ["S16_LE", "S24_3_LE", "S32_LE", "F32_LE", "F64_LE"])
def test_a_wav_playback_writes_a_header_that_describes_the_file(
    spawn_cdsp, config_file, tmp_path, fmt
):
    """A plain wav has its sizes patched on the way out, not left at the placeholder.

    The header goes down before the first chunk arrives, so the lengths in it start as
    `u32::MAX` and are only correct if the file was finalized. A crash, or a device that
    forgot to seek back, leaves a file every player rejects, and the size fields are
    where that shows.
    """
    source, _ = write_input(tmp_path)
    destination = str(tmp_path / "out.wav")
    run(
        spawn_cdsp,
        config_file,
        raw_capture(source, "F64_LE"),
        file_playback(destination, fmt, wav_header=True),
    )

    layout = wav_chunks(destination)
    assert layout["magic"] == "RIFF"
    assert layout["form"] == "WAVE"
    assert "fmt" in layout["chunks"]
    data_size, data_offset = layout["chunks"]["data"]
    assert data_size == FRAMES * CHANNELS * BYTES_PER_SAMPLE[fmt]
    assert data_offset + data_size == layout["total"]
    # The RIFF size covers everything after its own field, which is the rest of the file.
    assert layout["riff_size"] == layout["total"] - 8

    # And the header changed the framing only. The same run without one writes the audio
    # this is compared against, which keeps the claim clear of how either side rounds:
    # the two files came out of the same conversion code on the same input.
    plain = str(tmp_path / "out.raw")
    run(spawn_cdsp, config_file, raw_capture(source, "F64_LE"), file_playback(plain, fmt))
    payload = open(destination, "rb").read()[data_offset : data_offset + data_size]
    assert payload == open(plain, "rb").read()


def test_an_rf64_playback_leaves_the_sizes_in_the_ds64_chunk(
    spawn_cdsp, config_file, tmp_path
):
    """RF64 is how output gets past the 4 GB ceiling a plain wav has.

    The 32 bit size fields stay at their placeholder on purpose and a `ds64` chunk
    carries the real lengths, so a reader that only knows plain wav sees an obviously
    invalid file rather than a plausible wrong one. `MAX_WAV_DATA_BYTES` is what stops a
    plain wav run before it writes a file with wrapped sizes; this is the way around it.
    """
    source, _ = write_input(tmp_path)
    destination = str(tmp_path / "out.wav")
    run(
        spawn_cdsp,
        config_file,
        raw_capture(source, "F64_LE"),
        file_playback(destination, "S32_LE", wav_header=True, use_rf64=True),
    )

    layout = wav_chunks(destination)
    assert layout["magic"] == "RF64"
    assert layout["form"] == "WAVE"
    assert layout["riff_size"] == PLACEHOLDER
    assert "ds64" in layout["chunks"]
    assert layout["chunks"]["data"][0] == PLACEHOLDER
    # The ds64 chunk holds the RIFF and data sizes as 64 bit values, first and second.
    size, offset = layout["chunks"]["ds64"]
    riff64, data64 = np.frombuffer(
        open(destination, "rb").read()[offset : offset + 16], dtype="<u8"
    )
    assert data64 == FRAMES * CHANNELS * 4
    assert riff64 == layout["total"] - 8


@pytest.mark.parametrize("use_rf64", [False, True])
def test_a_wav_round_trip_is_bit_exact(spawn_cdsp, config_file, tmp_path, use_rf64):
    """Write a wav, read it back, and the samples have to be the ones that went in.

    F64_LE both ways, so the only thing between the two comparisons is the header: if
    the channel count, the sample format or the data offset in it is wrong, the capture
    reads the payload as something else and the bytes do not match. That makes this a
    stronger statement about the header than reading its fields, which only says the
    numbers are self consistent.
    """
    source, data = write_input(tmp_path)
    wav = str(tmp_path / "middle.wav")
    run(
        spawn_cdsp,
        config_file,
        raw_capture(source, "F64_LE"),
        file_playback(wav, "F64_LE", wav_header=True, use_rf64=use_rf64),
    )

    destination = str(tmp_path / "out.raw")
    run(spawn_cdsp, config_file, wav_capture(wav), file_playback(destination, "F64_LE"))
    assert open(destination, "rb").read() == data


def test_a_wav_capture_takes_its_channels_and_rate_from_the_header(
    spawn_cdsp, config_file, tmp_path
):
    """A `WavFile` capture has no `channels` or `format` key, so the header is all it has.

    That is the difference from `RawFile` worth testing: a raw capture is told what the
    file holds and a wav capture works it out, so a header parsed wrongly is a wrong
    channel count rather than a read error. Asserting the data comes back interleaved
    the way it went in is what catches a channel count read from the wrong offset.
    """
    samples = sine_samples(FRAMES / SAMPLERATE, channels=CHANNELS)
    # One channel scaled down, so a swapped or miscounted pair is visible in the output.
    samples[:, 1] *= 0.25
    source = str(tmp_path / "in.raw")
    open(source, "wb").write(encode("F64_LE", samples.ravel()))

    wav = str(tmp_path / "middle.wav")
    run(
        spawn_cdsp,
        config_file,
        raw_capture(source, "F64_LE"),
        file_playback(wav, "F64_LE", wav_header=True),
    )
    destination = str(tmp_path / "out.raw")
    run(spawn_cdsp, config_file, wav_capture(wav), file_playback(destination, "F64_LE"))

    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert written.shape == samples.shape
    assert np.array_equal(written, samples)
    assert np.abs(written[:, 1]).max() == pytest.approx(
        0.25 * np.abs(written[:, 0]).max(), rel=1e-9
    )


def test_extra_samples_works_on_a_wav_capture_too(spawn_cdsp, config_file, tmp_path):
    """The one option a `WavFile` capture shares with a raw one.

    It is a separate config struct with its own `extra_samples`, so the raw file version
    of this proves nothing about it.
    """
    source, data = write_input(tmp_path)
    wav = str(tmp_path / "middle.wav")
    run(
        spawn_cdsp,
        config_file,
        raw_capture(source, "F64_LE"),
        file_playback(wav, "F64_LE", wav_header=True),
    )
    destination = str(tmp_path / "out.raw")
    run(
        spawn_cdsp,
        config_file,
        wav_capture(wav, extra_samples=500),
        file_playback(destination, "F64_LE"),
    )

    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert len(written) == FRAMES + 500
    assert written[:FRAMES].tobytes() == data
    assert not written[FRAMES:].any()
