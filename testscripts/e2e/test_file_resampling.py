"""Resampling on the stock path, where both ends of the pipeline are free running.

`test_resampling.py` covers the capture side resampler against the dummy capture, which
is paced and has the rate adjust loop around it. This is the other half: the same
`src/utils/resampling.rs` selection reached from a file device, with no clock anywhere
and no rate to correct, which is what someone converting a file offline actually runs.

Two things make it a different test rather than a copy. The input is a file, so the
number of frames that went in is known and the number that came out can be checked
against the ratio, which is not available when the capture generates forever. And every
resampler type can be exercised cheaply, including `Synchronous`, since a file to file
run is hundreds of times real time and the CPU cost of a resampler stops mattering.
"""

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK
from swdevices import QUEUELIMIT, encode, file_playback, raw_capture, resampling

pytestmark = pytest.mark.stock

EXIT_OK = 0

CHANNELS = 2
LEVEL_DB = -6.0
AMPLITUDE = 10 ** (LEVEL_DB / 20)
TONE_HZ = 1000.0

RESAMPLERS = {
    "AsyncSinc": "type: AsyncSinc\nprofile: Balanced",
    "AsyncPoly": "type: AsyncPoly\ninterpolation: Cubic",
    "Synchronous": "type: Synchronous",
}


def write_tone(tmp_path, frames, samplerate):
    """A sine at `samplerate`, so the tone is at the same frequency whatever the rate."""
    wave = AMPLITUDE * np.sin(2 * np.pi * TONE_HZ * np.arange(frames) / samplerate)
    samples = np.repeat(wave[:, None], CHANNELS, axis=1)
    path = tmp_path / f"in{samplerate}.raw"
    path.write_bytes(encode("F64_LE", samples.ravel()))
    return str(path)


def convert(spawn_cdsp, config_file, tmp_path, source, capture_rate, resampler, timeout=60):
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            QUEUELIMIT: resampling(capture_rate, resampler),
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=timeout) == EXIT_OK
    return np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)


def assert_tone(written, samplerate):
    """The tone came through at its own frequency and its own level, after the rate change."""
    for channel in range(CHANNELS):
        column = written[:, channel]
        spectrum = np.abs(np.fft.rfft(column * np.hanning(len(column))))
        peak_hz = np.fft.rfftfreq(len(column), 1 / samplerate)[spectrum.argmax()]
        assert peak_hz == pytest.approx(TONE_HZ, rel=0.01)
        rms_db = 20 * np.log10(np.sqrt((column**2).mean()))
        assert rms_db == pytest.approx(LEVEL_DB - 3.0103, abs=0.1)


@pytest.mark.parametrize("name", list(RESAMPLERS))
def test_every_resampler_converts_44100_to_48000(spawn_cdsp, config_file, tmp_path, name):
    """The upward conversion, through each of the three that can change the rate.

    The frame count is the part that only a file can prove: the output has to be the
    input scaled by the ratio, give or take the tail the resampler has not flushed. One
    percent covers `Synchronous` dropping the last partial block, which is several
    hundred frames at this chunksize, and is still far tighter than a resampler running
    at the wrong ratio.
    """
    frames = 44100
    source = write_tone(tmp_path, frames, 44100)
    written = convert(spawn_cdsp, config_file, tmp_path, source, 44100, RESAMPLERS[name])
    assert len(written) == pytest.approx(frames * 48000 / 44100, rel=0.01)
    assert_tone(written, 48000)


@pytest.mark.parametrize("name", list(RESAMPLERS))
def test_every_resampler_converts_96000_to_48000(spawn_cdsp, config_file, tmp_path, name):
    """And downward, at the integer ratio `Synchronous` is meant for.

    Both directions because the resampler is selected once from the ratio, so a
    selection that only handles ratios above one would pass the test above and fail here.
    """
    frames = 96000
    source = write_tone(tmp_path, frames, 96000)
    written = convert(spawn_cdsp, config_file, tmp_path, source, 96000, RESAMPLERS[name])
    assert len(written) == pytest.approx(frames / 2, rel=0.01)
    assert_tone(written, 48000)


def test_the_slip_resampler_passes_equal_rates_through(spawn_cdsp, config_file, tmp_path):
    """`Slip` cannot change the rate, so at equal rates it has to be a passthrough.

    It exists to absorb clock drift between two devices at the same nominal rate, and
    with two files there is no drift to absorb, so every frame has to come out. Not a
    bit exact assertion, since the resampler is still in the path and free to work on
    the samples, but the count is exact and the tone is unharmed.
    """
    frames = 44100
    source = write_tone(tmp_path, frames, 44100)
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            "  samplerate: 48000": "  samplerate: 44100",
            QUEUELIMIT: resampling(44100, "type: Slip"),
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=60) == EXIT_OK

    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert len(written) == frames
    assert_tone(written, 44100)


def test_a_resampled_run_is_not_silence(spawn_cdsp, config_file, tmp_path):
    """The level assertions above pass on a constant, so the waveform is checked once.

    A resampler that emitted a DC offset at the right RMS, or repeated one block, would
    satisfy every other assertion in this file. Comparing against the tone the output
    rate should hold catches it, with a tolerance that is loose enough for the
    resampler's own passband ripple and the edges of the file.
    """
    frames = 44100
    source = write_tone(tmp_path, frames, 44100)
    written = convert(
        spawn_cdsp, config_file, tmp_path, source, 44100, RESAMPLERS["AsyncSinc"]
    )
    # Skip the first and last 2000 frames, where the resampler is still filling.
    column = written[2000:-2000, 0]
    phase = 2 * np.pi * TONE_HZ * np.arange(len(column)) / 48000
    # Against a sine and a cosine, taking the magnitude of the pair, because the
    # resampler has a delay of its own and the phase where the output was cut is not
    # something this test should have to work out. Against one of them alone the answer
    # would be the cosine of whatever that phase happened to be.
    projection = np.hypot(
        np.corrcoef(column, np.sin(phase))[0, 1], np.corrcoef(column, np.cos(phase))[0, 1]
    )
    assert projection > 0.99, f"the output projects onto the tone at {projection}"
