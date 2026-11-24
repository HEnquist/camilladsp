"""Processing asserted sample by sample rather than through a meter.

`test_processing.py` checks a mixer and a Biquad on the dummy devices, where the only
window onto the audio is the playback's level meters, so what it can claim is a level
within a tenth of a dB. With a file at each end the output is there to be read, and the
claims become exact: this gain is that number times every sample, and this mixer put
channel 0's samples in channel 1 and nothing else.

That is worth having separately because a level assertion has a blind spot. A mixer that
crossed the channels over and also halved them, or a gain that was applied twice to half
the frames, both land within a tenth of a dB of the right answer on a stationary tone.
Neither survives a comparison against the samples that went in.

The input carries a different waveform per channel on purpose. The generator and the
dummy capture put the same signal on every channel, so a channel swap is undetectable by
construction there and `test_processing.py` has to put a gain in front of the mixer to
see anything at all.
"""

import numpy as np
import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK, SAMPLERATE
from swdevices import encode, file_playback, raw_capture

pytestmark = pytest.mark.stock

EXIT_OK = 0

FRAMES = 4096
CHANNELS = 2

EMPTY_PIPELINE = "pipeline: []"

GAIN_DB = -6.0
GAIN_CONFIG = f"""filters:
  half:
    type: Gain
    parameters:
      gain: {GAIN_DB}

pipeline:
  - type: Filter
    channels: [0, 1]
    names: [half]"""

SWAP_CONFIG = """mixers:
  swap:
    channels:
      in: 2
      out: 2
    mapping:
      - dest: 0
        sources:
          - channel: 1
            gain: 0
      - dest: 1
        sources:
          - channel: 0
            gain: 0

pipeline:
  - type: Mixer
    name: swap"""


def two_tones():
    """One channel a sine and the other a quarter amplitude cosine, so they differ."""
    phase = 2 * np.pi * 1000.0 * np.arange(FRAMES) / SAMPLERATE
    return np.stack([0.5 * np.sin(phase), 0.125 * np.cos(phase)], axis=1)


def run(spawn_cdsp, config_file, tmp_path, pipeline, timeout=30):
    samples = two_tones()
    source = str(tmp_path / "in.raw")
    open(source, "wb").write(encode("F64_LE", samples.ravel()))
    destination = str(tmp_path / "out.raw")
    config = config_file(
        {
            FILE_CAPTURE: raw_capture(source, "F64_LE"),
            FILE_PLAYBACK: file_playback(destination, "F64_LE"),
            EMPTY_PIPELINE: pipeline,
        },
        base="file_devices.yml",
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=timeout) == EXIT_OK
    written = np.fromfile(destination, dtype="<f8").reshape(-1, CHANNELS)
    assert written.shape == samples.shape
    return samples, written


def test_an_empty_pipeline_changes_nothing(spawn_cdsp, config_file, tmp_path):
    """The control case, without which the two below prove only that something happened.

    A run that silently replaced the audio with the input's first chunk, or that dropped
    the pipeline entirely, has to fail here before the mixer and gain results mean
    anything.
    """
    samples, written = run(spawn_cdsp, config_file, tmp_path, EMPTY_PIPELINE)
    assert np.array_equal(written, samples)


def test_a_gain_filter_scales_every_sample_by_the_same_factor(
    spawn_cdsp, config_file, tmp_path
):
    """A Gain is a multiplication, so the output is the input times one number.

    Asserted per sample rather than on the level, which is what separates a gain applied
    once from one applied to some of the frames twice. The tolerance is the float error
    of a single multiply, not a measurement band.
    """
    samples, written = run(spawn_cdsp, config_file, tmp_path, GAIN_CONFIG)
    factor = 10 ** (GAIN_DB / 20)
    assert np.abs(written - samples * factor).max() < 1e-12
    # And the two channels were scaled by the same factor, not normalized separately.
    ratios = np.abs(written).max(axis=0) / np.abs(samples).max(axis=0)
    assert ratios[0] == pytest.approx(ratios[1], rel=1e-9)


def test_a_mixer_moves_the_channels_and_leaves_them_otherwise_alone(
    spawn_cdsp, config_file, tmp_path
):
    """A crossover mixer has to produce the input with its two columns exchanged.

    Nothing weaker would do: the output is compared against the input's own samples, so
    a mixer that swapped the channels and also changed their gain, filtered them or
    delayed them by a frame fails on the comparison rather than passing a level check.
    """
    samples, written = run(spawn_cdsp, config_file, tmp_path, SWAP_CONFIG)
    assert np.array_equal(written[:, 0], samples[:, 1])
    assert np.array_equal(written[:, 1], samples[:, 0])
