"""Clipping, where the audio meets the sample format.

Clipping happens in exactly one place in CamillaDSP: the conversion from the processing
floats to whatever the playback device wants, `chunk_to_buffer_rawbytes` in
`src/utils/conversions.rs`. So `GetClippedSamples` was structurally always 0 in this suite,
because the dummy playback used to drop the audio it received without converting anything.
It now converts when a `format` is configured, the way a real device does on its way to the
hardware, and drops the audio as before without one.

The config pairs an integer format with 12 dB of gain on a -6 dBFS tone, so the peak is
1.995 and the samples that cannot be held are a known fraction of the whole rather than an
unknown few. See dummy_clipping.yml.
"""

import math
import time

import pytest

SAMPLERATE = 48000
CHANNELS = 2
TONE_HZ = 1000
TONE_DB = -6.0
GAIN_DB = 12.0

# What the count is compared against. The window is long enough that the reading offset
# below is a percent or two of it, and the fraction itself is exact.
WINDOW_SECONDS = 1.0
FRACTION_TOLERANCE = 0.03

# Every format that limits, and both of the 24 in 32 bit justifications. The limit differs
# between them by a part in 32768 at most, which no sample of this tone lands inside, so
# they are all expected to count the same samples.
INTEGER_FORMATS = ["S16_LE", "S24_3_LE", "S24_4_LJ_LE", "S24_4_RJ_LE", "S32_LE"]


def clipped_fraction():
    """The fraction of this tone's samples that an integer format cannot hold.

    A 1 kHz tone at 48 kHz repeats every 48 samples, so the samples sit on a fixed grid and
    the count per period is exact rather than an average: 30 of the 48 are past full scale
    at a peak of 1.995, which is 0.625. Computing it rather than writing 0.625 down is what
    keeps this honest if the tone, the level or the gain in the config changes.

    The comparison is `> 1.0`, while the real limit is a part in 32768 below that for S16
    and closer still for the wider formats. The nearest sample of this tone sits 0.0024
    from the limit, which is far outside that difference, so the distinction cannot change
    the count and every integer format counts the same samples.
    """
    peak = 10 ** ((TONE_DB + GAIN_DB) / 20)
    period = SAMPLERATE // TONE_HZ
    over = sum(1 for n in range(period) if abs(peak * math.sin(2 * math.pi * n / period)) > 1.0)
    return over / period


def start(control_cdsp, replacements=None):
    return control_cdsp(replacements=replacements, base="dummy_clipping.yml")


def counts(cdsp):
    """The clipped sample count, and the frame count it was counted over.

    Read in this order at both ends of a window, so the gap between the two readings is the
    same at each end and cancels in the difference.
    """
    return cdsp.send("GetClippedSamples"), cdsp.playback_control.get_int("frames")


def wait_for_clipping(cdsp, timeout=10.0):
    """Wait for the first clipped sample to be counted, and return the count."""
    return cdsp.poll_until_true("GetClippedSamples", lambda clipped: clipped > 0, timeout=timeout)


def assert_count_stays(cdsp, expected=0, seconds=1.0):
    """Assert the clipped count stays at `expected` over a whole second of audio.

    A second is about 47 chunks on this config, so a conversion that limited even one sample
    per chunk would be caught. The frame counter is what says the audio was really flowing,
    rather than the count holding still because nothing was played.
    """
    frames = cdsp.playback_control.get_int("frames")
    time.sleep(seconds)
    assert cdsp.playback_control.get_int("frames") > frames, "no audio was played at all"
    assert cdsp.send("GetClippedSamples") == expected


@pytest.mark.parametrize("sample_format", INTEGER_FORMATS)
def test_clipping_is_counted(control_cdsp, sample_format):
    """The count should track the samples the format cannot hold, per sample per channel.

    Asserting the rate rather than just that it is nonzero is what pins down what is being
    counted: a count per frame instead of per sample would come out half of this, and one
    per chunk a thousandth of it.
    """
    cdsp = start(control_cdsp, {"format: S16_LE": f"format: {sample_format}"})
    wait_for_clipping(cdsp)
    clipped, frames = counts(cdsp)
    time.sleep(WINDOW_SECONDS)
    clipped_later, frames_later = counts(cdsp)
    samples = (frames_later - frames) * CHANNELS
    assert samples > 0, "no audio was played during the window"
    assert (clipped_later - clipped) / samples == pytest.approx(
        clipped_fraction(), abs=FRACTION_TOLERANCE
    )


@pytest.mark.parametrize("sample_format", ["F32_LE", "F64_LE"])
def test_a_float_format_has_nothing_to_clip(control_cdsp, sample_format):
    """A float format holds a sample past full scale, so the same signal clips nothing.

    Worth pinning down, because it means a config that reports no clipping is not the same
    as a config whose audio fits.
    """
    cdsp = start(control_cdsp, {"format: S16_LE": f"format: {sample_format}"})
    assert_count_stays(cdsp)


def test_a_signal_that_fits_is_not_counted(control_cdsp):
    """With the boost removed the tone is well inside full scale and nothing is limited.

    This is what makes the test above mean something: it is the format that decides what
    can be held, but the signal that decides whether anything needs limiting.
    """
    cdsp = start(control_cdsp, {f"gain: {GAIN_DB}": "gain: 0.0"})
    assert_count_stays(cdsp)


def test_nothing_is_converted_without_a_format(control_cdsp):
    """A dummy playback with no format drops the audio without converting it.

    So the over scale signal passes through untouched and uncounted, which is what the rest
    of the suite runs on and why it reads zero everywhere.
    """
    cdsp = start(control_cdsp, {"    format: S16_LE\n": ""})
    assert_count_stays(cdsp)


def test_reset_clears_the_count(control_cdsp):
    """`ResetClippedSamples` should zero it, and counting should carry on from there.

    The reset is not a stop, so the value read straight after it can already hold the chunk
    that arrived in between. What it may not hold is what was counted before.
    """
    cdsp = start(control_cdsp)
    before = wait_for_clipping(cdsp)
    time.sleep(0.5)
    assert cdsp.send("GetClippedSamples") > before
    cdsp.send("ResetClippedSamples")
    after = cdsp.send("GetClippedSamples")
    assert after < before
    # And the counting is running again, rather than having been switched off.
    cdsp.poll_until_true("GetClippedSamples", lambda clipped: clipped > after, timeout=10.0)


def test_turning_the_volume_down_stops_the_clipping(control_cdsp):
    """The fader is ahead of the conversion, so turning it down is what a user would do.

    12 dB of attenuation takes the peak from 1.995 to 0.5, and the count has to stop rather
    than merely slow. The volume ramps rather than stepping, so it stops a moment later.
    """
    cdsp = start(control_cdsp)
    wait_for_clipping(cdsp)
    cdsp.send("SetVolume", -GAIN_DB)
    deadline = time.monotonic() + 10.0
    while True:
        clipped = cdsp.send("GetClippedSamples")
        time.sleep(0.5)
        if cdsp.send("GetClippedSamples") == clipped:
            break
        assert time.monotonic() < deadline, "the clipped count never stopped rising"
    # Still counting nothing a moment later, rather than having paused between chunks.
    assert_count_stays(cdsp, clipped)
