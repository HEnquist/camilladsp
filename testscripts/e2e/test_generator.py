"""The SignalGenerator capture device, which nothing else in the suite reaches.

The dummy capture generates its audio from the same `Signal` enum, so the waveforms
themselves are covered wherever a dummy test asserts a level. What is not covered is the
device: `src/generatordevice.rs` is its own capture implementation, free running rather
than paced, and on a stock build it is the only capture that produces audio without a
file behind it.

Free running is what makes these awkward to write. The generator never ends and produces
hundreds of megabytes a second, so pointing its playback at a file fills the runner's
disk in seconds. Stdout does not have that problem: the pipe holds 64 kB, the device
blocks once it is full, and the test reads exactly as much as it wants before asking the
process to exit. See `read_exactly` for the shape.

Each signal is asserted on what distinguishes it from the other two, so a generator that
produced the wrong one fails rather than merely reading a plausible level.
"""

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK
from swdevices import file_playback, generator_capture, read_exactly, stdout_playback

pytestmark = pytest.mark.stock

EXIT_OK = 0

SAMPLERATE = 48000
CHANNELS = 2
# A second of audio is enough for a 1 Hz FFT bin, and 768 kB through a pipe is instant.
FRAMES = SAMPLERATE
NBYTES = FRAMES * CHANNELS * 8

LEVEL_DB = -6.0
AMPLITUDE = 10 ** (LEVEL_DB / 20)
# On an FFT bin centre at 48 kHz, so the peak reads its configured level exactly rather
# than losing a fraction of a dB to the window. See test_spectrum.py for the reasoning.
TONE_HZ = 984.375


def generate(start_cdsp, config_file, signal, nbytes=NBYTES, edits=None):
    """Run a generator into stdout, take `nbytes` of it, and stop.

    The websocket is what stops it, so the exit code is a real clean shutdown rather than
    a kill, and `exit` drains the pipe while it waits so the playback device is never
    left blocked inside a write with nothing reading the far end.
    """
    replacements = {
        FILE_CAPTURE: generator_capture(signal),
        FILE_PLAYBACK: stdout_playback("F64_LE"),
    }
    replacements.update(edits or {})
    config = config_file(replacements, base="file_devices.yml")
    cdsp = start_cdsp(config=config, pipe_stdout=True)
    data = read_exactly(cdsp.process.stdout, nbytes)
    assert cdsp.exit() == EXIT_OK
    return np.frombuffer(data, dtype="<f8").reshape(-1, CHANNELS)


def peak_frequency(column, samplerate=SAMPLERATE):
    spectrum = np.abs(np.fft.rfft(column * np.hanning(len(column))))
    return np.fft.rfftfreq(len(column), 1 / samplerate)[spectrum.argmax()]


def rms_db(column):
    return 20 * np.log10(np.sqrt((column**2).mean()))


def test_a_generated_sine_has_the_level_and_the_frequency_it_was_given(
    start_cdsp, config_file
):
    """The peak is the configured level and the energy is where it was asked for.

    A sine's RMS is 3.01 dB below its peak, which is the check that says the waveform is
    a sine rather than anything else reaching the same peak.
    """
    written = generate(
        start_cdsp, config_file, f"type: Sine, freq: {TONE_HZ}, level: {LEVEL_DB}"
    )
    assert len(written) == FRAMES
    for channel in range(CHANNELS):
        column = written[:, channel]
        assert np.abs(column).max() == pytest.approx(AMPLITUDE, rel=1e-4)
        assert rms_db(column) == pytest.approx(LEVEL_DB - 3.0103, abs=0.01)
        assert peak_frequency(column) == pytest.approx(TONE_HZ, abs=2.0)


def test_a_generated_square_is_two_values_and_nothing_between(start_cdsp, config_file):
    """A square wave only ever sits at plus or minus its level, so its RMS is that level.

    Counting the distinct values is the sharp version of the same claim: a square that
    had been filtered, ramped or resampled anywhere on the way would have more than two.
    """
    written = generate(
        start_cdsp, config_file, f"type: Square, freq: 1000.0, level: {LEVEL_DB}"
    )
    for channel in range(CHANNELS):
        column = written[:, channel]
        assert sorted(np.unique(column)) == pytest.approx([-AMPLITUDE, AMPLITUDE], rel=1e-4)
        assert rms_db(column) == pytest.approx(LEVEL_DB, abs=0.01)


def test_generated_white_noise_is_broadband_and_at_its_level(start_cdsp, config_file):
    """Noise is asserted on what it is not: a peak and a spectrum that do not concentrate.

    The generator's noise is uniform over plus and minus the amplitude, so its RMS sits
    4.77 dB below the level, and the two channels are independent, which a generator
    handing the same buffer to every channel would fail.
    """
    written = generate(start_cdsp, config_file, "type: WhiteNoise, level: -20.0")
    amplitude = 10 ** (-20.0 / 20)
    for channel in range(CHANNELS):
        column = written[:, channel]
        assert np.abs(column).max() <= amplitude
        assert rms_db(column) == pytest.approx(-20.0 - 4.771, abs=0.1)
    correlation = np.corrcoef(written[:, 0], written[:, 1])[0, 1]
    assert abs(correlation) < 0.1, f"the two channels correlate at {correlation}"


def test_the_generator_keeps_producing_until_it_is_stopped(start_cdsp, config_file):
    """It has no end of its own, which is the property the rest of these depend on.

    Taking four times as much audio as the other tests do proves the device is not
    handing over one buffer and stopping, and that the run ends because it was asked to
    rather than because the generator ran out.
    """
    written = generate(
        start_cdsp,
        config_file,
        f"type: Sine, freq: {TONE_HZ}, level: {LEVEL_DB}",
        nbytes=4 * NBYTES,
    )
    assert len(written) == 4 * FRAMES
    assert rms_db(written[:, 0]) == pytest.approx(LEVEL_DB - 3.0103, abs=0.01)


def test_a_generator_into_a_file_needs_no_channel_count_from_the_file(
    start_cdsp, config_file, tmp_path
):
    """The generator's `channels` is what decides the width, with no file to disagree.

    Worth one test with a file on the far end rather than a pipe, since that is the
    combination someone generating test material actually uses, and it is the only place
    the run is stopped by the websocket while a file is open.
    """
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: generator_capture(
                f"type: Sine, freq: {TONE_HZ}, level: {LEVEL_DB}"
            ),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
        },
        base="file_devices.yml",
    )
    cdsp = start_cdsp(config=config)
    # Stopped as soon as the engine is up, since a free running generator writes a few
    # hundred megabytes a second and there is nothing to wait for.
    assert cdsp.exit() == EXIT_OK

    written = np.fromfile(destination, dtype="<f8")
    assert len(written) > 0
    assert len(written) % CHANNELS == 0
    assert np.abs(written).max() == pytest.approx(AMPLITUDE, rel=1e-4)
