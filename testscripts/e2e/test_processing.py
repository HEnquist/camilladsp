"""Processing correctness, asserted through the audio rather than through the config.

The suite already proves that a Gain filter drops the level and that Mute floors it, so
what is left of the processing correctness item is a mixer that moves audio between
channels and a filter with a response worth checking a number against.

Both are asserted on the playback meters. That is the whole point: a config round trip
proves CamillaDSP stored what it was given, and says nothing about whether the audio was
processed. These read the level at the far end of the pipeline.

The tone is at 1 kHz and the filters are placed relative to it, so every expected value
here is exact rather than approximate. See the Biquad test for why.
"""

import pytest

# dummy_sine.yml generates a -6 dBFS sine, so its RMS is 3.01 dB below that, and sends it
# through a -6 dB Gain filter.
SINE_RMS_DB = -9.01
GAIN_DB = -6.0
# The meters are computed from the same samples the pipeline produced, so the only spread
# is which chunk was measured. A tenth of a dB is loose enough for that and tight enough
# to catch a filter that did nothing.
TOLERANCE = 0.1

# The pipeline of dummy_sine.yml, replaced by the mixer tests, and the whole filter and
# pipeline section, replaced by the Biquad tests so nothing else stands between the
# generator and the meters.
PIPELINE = """pipeline:
  - type: Filter
    channels: [0, 1]
    names: [testgain]"""
FILTERS_AND_PIPELINE = """filters:
  testgain:
    type: Gain
    description: "nbr 1"
    parameters:
      gain: -6.0

""" + PIPELINE

# A mixer that crosses the two channels over, plus an asymmetry ahead of it so the swap
# is visible at all. The generator puts the same waveform on both channels, so without
# something to tell them apart a channel swap is undetectable by construction.
SWAP_CONFIG = """mixers:
  swap:
    description: "cross the two channels over"
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
  - type: Filter
    channels: [0]
    names: [testgain]
  - type: Mixer
    name: swap"""

# A peaking filter sitting exactly on the tone. At its centre frequency an RBJ peaking
# section has a magnitude of exactly its configured gain, whatever Q is set to, so the
# expected level is a subtraction rather than a response curve evaluated in the test.
PEAKING_GAIN_DB = 6.0
PEAKING_CONFIG = f"""filters:
  peak:
    type: Biquad
    parameters:
      type: Peaking
      freq: 1000
      gain: {PEAKING_GAIN_DB}
      q: 2.0

pipeline:
  - type: Filter
    channels: [0, 1]
    names: [peak]"""

# The same filter moved three octaves down. Far outside its bandwidth a peaking section
# is flat, so this is the control: it proves the level change above came from the
# response at the tone and not from a peaking filter changing the level wherever it sits.
DISTANT_CONFIG = PEAKING_CONFIG.replace("freq: 1000", "freq: 125")


def playback_rms(cdsp, timeout=10.0):
    """Wait for the playback meters to hold still, and return both channels.

    The first reads after a config change catch the pipeline mid transition, and a
    Biquad needs a few chunks to settle besides, so this waits for two consecutive
    readings that agree rather than taking the first one that arrives.
    """
    previous = [None]

    def settled(values):
        if len(values) != 2:
            return False
        last, previous[0] = previous[0], values
        return last is not None and all(
            new == pytest.approx(old, abs=TOLERANCE / 2) for new, old in zip(values, last)
        )

    return cdsp.poll_until_true("GetPlaybackSignalRms", settled, timeout=timeout)


def test_a_mixer_moves_audio_between_channels(start_cdsp, config_file):
    """A crossover mixer has to put each channel's audio out on the other one."""
    config = config_file({PIPELINE: SWAP_CONFIG})
    cdsp = start_cdsp(config=config)

    # Channel 0 goes through the -6 dB gain and channel 1 does not, then the mixer
    # crosses them, so the quiet one has to come out on the far side.
    left, right = playback_rms(cdsp)
    assert left == pytest.approx(SINE_RMS_DB, abs=TOLERANCE)
    assert right == pytest.approx(SINE_RMS_DB + GAIN_DB, abs=TOLERANCE)


def test_a_mixer_without_the_swap_leaves_the_channels_alone(start_cdsp, config_file):
    """The control for the test above, so the asymmetry is shown to be real.

    Without this, a mixer that dropped the audio and a mixer that swapped it would both
    satisfy the assertion as long as the two channels happened to differ the right way.
    """
    straight = SWAP_CONFIG.replace("channel: 1\n            gain", "channel: 0\n            gain", 1)
    straight = straight.replace(
        "      - dest: 1\n        sources:\n          - channel: 0",
        "      - dest: 1\n        sources:\n          - channel: 1",
        1,
    )
    cdsp = start_cdsp(config=config_file({PIPELINE: straight}))

    left, right = playback_rms(cdsp)
    assert left == pytest.approx(SINE_RMS_DB + GAIN_DB, abs=TOLERANCE)
    assert right == pytest.approx(SINE_RMS_DB, abs=TOLERANCE)


def test_a_biquad_applies_its_response_at_the_tone(start_cdsp, config_file):
    """A peaking filter centred on the tone changes the level by exactly its gain.

    Exactly, not approximately, and that is why this shape was chosen over a lowpass at
    its corner. An RBJ peaking section has `b0 = 1 + alpha*A`, `a0 = 1 + alpha/A` with
    matching `b1 = a1` and `b2 = 1 - alpha*A`, so at `w0` the numerator and denominator
    collapse to `2j*alpha*A*sin(w0)` and `2j*alpha*sin(w0)/A`. The magnitude is `A**2`,
    which is the configured gain in dB, independent of Q and of the sample rate. A
    lowpass would instead need the prewarped response worked out in the test, which
    would mean reimplementing the filter to check it.
    """
    cdsp = start_cdsp(config=config_file({FILTERS_AND_PIPELINE: PEAKING_CONFIG}))

    left, right = playback_rms(cdsp)
    expected = SINE_RMS_DB + PEAKING_GAIN_DB
    assert left == pytest.approx(expected, abs=TOLERANCE)
    assert right == pytest.approx(expected, abs=TOLERANCE)


def test_a_biquad_away_from_the_tone_leaves_it_alone(start_cdsp, config_file):
    """The same filter three octaves down must not touch the level.

    This is what makes the test above a statement about the response rather than about
    the filter being in the path at all.
    """
    cdsp = start_cdsp(config=config_file({FILTERS_AND_PIPELINE: DISTANT_CONFIG}))

    left, right = playback_rms(cdsp)
    assert left == pytest.approx(SINE_RMS_DB, abs=TOLERANCE)
    assert right == pytest.approx(SINE_RMS_DB, abs=TOLERANCE)
