"""Resampling on the capture side.

The dummy capture generates at `capture_samplerate` and hands the pipeline `samplerate`,
which is what puts a resampler in the path. Nothing in the suite could do that before: the
file and generator devices are free running, so the one rate that mattered was the one the
pipeline ran at, and `GetResamplerLoad` sat at 0.0 for every test in the suite.

What that unlocks is the selection between the resampler types and the whole capture side
of rate adjust, which lives in `src/utils/resampling.rs` and is reached from every real
backend. The rate adjust half is in test_rate_control.py, next to the no-resampler case it
completes; what is here is everything that does not depend on the clock being kept
accurately.

See dummy_resample.yml for the config, and for why AsyncPoly is the one it uses.
"""

import time

import pytest

SAMPLERATE = 48000
CAPTURE_SAMPLERATE = 96000
# The generator sits on an FFT bin centre at the pipeline rate, so the peak reads its
# configured level exactly. test_spectrum.py has the full reasoning, which is worth
# knowing before touching this number.
TONE_HZ = 984.375
TONE_DB = -6.0

RESAMPLER = "    type: AsyncPoly\n    interpolation: Cubic"
SYNCHRONOUS = {RESAMPLER: "    type: Synchronous"}
NO_RESAMPLER = {
    f"  resampler:\n{RESAMPLER}\n": "  resampler: null\n",
}
NO_RATE_ADJUST = {"enable_rate_adjust: true": "enable_rate_adjust: false"}

# 64 log spaced bins from 20 Hz to 20 kHz are 11.6 % apart, so a peak in the right bin is
# within 15 % of the tone and one bin out is already outside that.
SPECTRUM_REQUEST = {
    "side": "playback",
    "channel": None,
    "min_freq": 20.0,
    "max_freq": 20000.0,
    "n_bins": 64,
}
BIN_TOLERANCE = 0.15


def start(control_cdsp, replacements=None, **kwargs):
    return control_cdsp(replacements=replacements, base="dummy_resample.yml", **kwargs)


def spectrum(cdsp, **overrides):
    """Read a spectrum once the ring buffer has enough history to fill one.

    The buffer is not filled until something asks for spectrum data, and the request that
    asks is the one that sets the flag, so the first few come back as an error.
    """
    request = {**SPECTRUM_REQUEST, **overrides}
    deadline = time.monotonic() + 10.0
    while True:
        reply = cdsp.send_raw("GetSpectrum", request)
        if reply.get("result") == "Ok":
            return reply["value"]
        if time.monotonic() >= deadline:
            raise TimeoutError(f"GetSpectrum never succeeded, last reply {reply}")
        time.sleep(0.05)


def peak_of(data):
    """The (frequency, magnitude) of the loudest bin."""
    pairs = list(zip(data["frequencies"], data["magnitudes"]))
    return max(pairs, key=lambda pair: pair[1])


@pytest.mark.parametrize("capture_rate", [44100, CAPTURE_SAMPLERATE, 192000])
def test_the_measured_rate_is_the_capture_rate(control_cdsp, capture_rate):
    """The device measures what it produces, which is the rate on its own side.

    Reading back `samplerate` here would mean the device was generating at the pipeline
    rate and the resampler was converting from a rate nothing ran at. Both a lower and a
    higher capture rate are covered, since the ratio decides how many input frames one
    output chunk needs and only one of the two directions makes that more than a chunk.
    """
    cdsp = start(
        control_cdsp,
        {f"capture_samplerate: {CAPTURE_SAMPLERATE}": f"capture_samplerate: {capture_rate}"},
    )
    cdsp.poll_until_true(
        "GetCaptureRate",
        lambda rate: rate == pytest.approx(capture_rate, rel=0.05),
        timeout=8.0,
    )


def test_the_resampler_reports_its_load(control_cdsp):
    """`GetResamplerLoad` is a percentage of a chunk period, and is only set by a resample.

    It is written from `ChunkResampler::resample_chunk` and nowhere else, so a run with no
    resampler leaves it at exactly zero however busy the machine is. That is what it read
    for every test in this suite until this config existed.
    """
    cdsp = start(control_cdsp)
    load = cdsp.poll_until_true("GetResamplerLoad", lambda load: load > 0.0, timeout=8.0)
    # AsyncPoly resampling 2:1 is a fraction of a percent of a chunk period on an optimised
    # build. Above 100 % would mean the capture cannot resample a chunk inside a chunk
    # period, which is a decimal point in the wrong place rather than a slow machine.
    assert load < 100.0


def test_no_resampler_means_no_load(cdsp):
    """The other half of the above, on the config the rest of the suite runs."""
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0)
    assert cdsp.send("GetResamplerLoad") == 0.0


@pytest.mark.parametrize(
    "resampler",
    [
        "    type: AsyncSinc\n    profile: VeryFast",
        "    type: AsyncSinc\n    profile: Balanced",
        "    type: AsyncSinc\n    profile: Accurate",
        "    type: AsyncPoly\n    interpolation: Linear",
        "    type: AsyncPoly\n    interpolation: Septic",
        "    type: Synchronous",
    ],
)
def test_every_resampler_type_runs(control_cdsp, resampler):
    """Each type in the config enum should build, resample, and keep the audio moving.

    Nothing here asserts on rates or levels, deliberately. The claim is that the type is
    selectable and really does resample, which holds on any machine: the frame counter
    climbing says audio moved, and a load above zero says it moved through the resampler.
    What the rate comes out as is asserted once, on the cheap resampler, in the test above.
    """
    cdsp = start(control_cdsp, {RESAMPLER: resampler})
    cdsp.poll_until_true("GetResamplerLoad", lambda load: load > 0.0, timeout=8.0)
    assert cdsp.send("GetState") == "Running"
    frames = cdsp.capture_control.get_int("frames")
    time.sleep(0.3)
    assert cdsp.capture_control.get_int("frames") > frames


def test_slip_resamples_at_equal_rates(control_cdsp):
    """Slip is the one type that cannot convert between rates, only adjust around 1:1.

    So it gets its own case with the rates made equal, rather than a parametrize entry that
    would ask it for a 2:1 conversion it does not implement.
    """
    cdsp = start(
        control_cdsp,
        {
            RESAMPLER: "    type: Slip",
            f"capture_samplerate: {CAPTURE_SAMPLERATE}": f"capture_samplerate: {SAMPLERATE}",
        },
    )
    cdsp.poll_until_true("GetResamplerLoad", lambda load: load > 0.0, timeout=8.0)
    cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: rate == pytest.approx(SAMPLERATE, rel=0.05), timeout=8.0
    )


def test_capture_samplerate_is_ignored_without_a_resampler(control_cdsp):
    """With no resampler there is nothing to convert with, so the extra rate is dropped.

    `new_capture_device` warns and falls back to `samplerate`, which is observable as the
    device running at the pipeline rate rather than at the one it was asked for.
    """
    cdsp = start(control_cdsp, NO_RESAMPLER)
    cdsp.poll_until_true(
        "GetCaptureRate", lambda rate: rate == pytest.approx(SAMPLERATE, rel=0.05), timeout=8.0
    )
    assert cdsp.send("GetResamplerLoad") == 0.0


def test_the_resampled_tone_keeps_its_frequency(control_cdsp):
    """Halving the rate must not move the tone, which is the point of resampling at all.

    The playback side of the spectrum is measured after the resampler, so this is the whole
    path: generated at 96 kHz, resampled to 48 kHz, and still 984 Hz at the far end. A
    resampler fed the wrong ratio would put it at half or double.

    Rate adjust is off here. It works through the resampler ratio, so while it pulls the
    buffer level in after startup it moves the tone too, up to half a percent at the
    clamp. That is 0.4 of a bin, and the scalloping loss at that offset is most of a dB,
    which is what the level assertion below caught on a slow runner.
    """
    cdsp = start(control_cdsp, NO_RATE_ADJUST)
    frequency, magnitude = peak_of(spectrum(cdsp))
    assert frequency == pytest.approx(TONE_HZ, rel=BIN_TOLERANCE)
    # And with the level intact, so nothing scaled the samples on the way through.
    assert magnitude == pytest.approx(TONE_DB, abs=0.5)


@pytest.mark.xfail(
    strict=True,
    reason="capture spectrum bins are computed at samplerate, but the buffer holds "
    "capture_samplerate audio, so the frequencies come out scaled by the ratio",
)
def test_the_capture_spectrum_keeps_the_tone_frequency(control_cdsp):
    """The capture side of the spectrum is wrong whenever the two rates differ.

    Every backend pushes the chunk into the spectrum buffer before resampling it, see
    `src/alsa_backend/device.rs:1060` and the same two lines in the CoreAudio, WASAPI, file
    and dummy devices, so that buffer holds audio at `capture_samplerate`. But
    `handle_get_spectrum` takes its rate from `devices.samplerate` for both sides,
    `src/websocket_server/mod.rs:1994`, so the bin frequencies are scaled by
    samplerate / capture_samplerate: this 984 Hz tone reads as 492 Hz on a 96 kHz capture.

    Nothing in CamillaDSP is affected by it, only what a GUI draws, and the fix is a rate
    per side rather than one for both. Written as a strict xfail because the behaviour is
    wrong rather than intended, so this goes red and asks to be unmarked once it is fixed.
    """
    frequency, _ = peak_of(spectrum(start(control_cdsp), side="capture"))
    assert frequency == pytest.approx(TONE_HZ, rel=BIN_TOLERANCE)
