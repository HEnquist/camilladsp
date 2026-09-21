"""Spectrum analysis, one shot and subscribed.

These run on a variant of the base config whose generator is moved from 1 kHz to
984.375 Hz, and the exact frequency is the point. `min_freq: 20` at 48 kHz gives a 4096
point FFT, `src/spectrum.rs:335`, so the bins are 11.71875 Hz apart and 984.375 Hz is
bin 84 exactly. A tone on a bin centre has no scalloping loss, so the peak reads the
generator's configured level to three decimals and every assertion below can be exact.

At 1 kHz it is not exact and not stable either. That sits a third of a bin off centre,
which costs 0.63 dB, and the loss varies with where the analysis window falls, so the
peak wanders between -6.63 and -7.96 dB. Measured at idle, 8 % of reads land low, which
is a flaky test rather than a bug: 1.4 dB is the Hann window's worst case scalloping
loss and the analyser is behaving as designed. Moving the tone onto a bin centre is what
makes the level assertable at all, so do not "simplify" this back to a round 1 kHz.
"""

import time

import pytest

TONE_HZ = 984.375
CAPTURE_PEAK_DB = -6.0
PLAYBACK_PEAK_DB = -12.0
# Exact, now that the tone is bin centred. The slack is for f32 printing, not for drift.
TOLERANCE = 0.05

# 64 log spaced bins from 20 Hz to 20 kHz is a ratio of 1.116 between neighbours, so a
# peak in the right bin is within 12 % of the tone and one bin out is already outside
# this. Sharp, over a range spanning three decades. min_freq is also what sets the FFT
# length, so leaving it at 20 is what keeps the tone bin centred.
REQUEST = {
    "side": "capture",
    "channel": None,
    "min_freq": 20.0,
    "max_freq": 20000.0,
    "n_bins": 64,
}
BIN_TOLERANCE = 0.15


@pytest.fixture
def cdsp(start_cdsp, config_file):
    """Overrides the shared fixture with a generator on an FFT bin centre.

    Only this module wants the odd frequency, and only for the reason in the module
    docstring, so the config stays here rather than in a checked in .yml of its own.
    """
    return start_cdsp(config=config_file({"freq: 1000": f"freq: {TONE_HZ}"}))


def spectrum(cdsp, **overrides):
    """Read a spectrum once the ring buffer has enough history to fill one.

    The buffer is not filled at all until something asks for spectrum data, and the
    request that asks is the one that sets the flag, so the first few come back as an
    error while the history accumulates.
    """
    request = {**REQUEST, **overrides}
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


def test_the_buffer_is_not_filled_until_asked(cdsp):
    """Filling costs a pass over every chunk, so the flag that starts it is sticky.

    The very first request is therefore guaranteed to find an empty buffer, whatever the
    engine has been doing until then, and it is the request itself that starts the fill.
    """
    cdsp.poll_until("GetState", "Running")
    first = cdsp.send_raw("GetSpectrum", REQUEST)
    assert first["result"] == "InvalidRequestError"
    assert first["message"] == "No audio data available"
    assert spectrum(cdsp)["magnitudes"]


def test_capture_spectrum_peaks_at_the_generator_frequency(cdsp):
    frequency, magnitude = peak_of(spectrum(cdsp))
    assert frequency == pytest.approx(TONE_HZ, rel=BIN_TOLERANCE)
    assert magnitude == pytest.approx(CAPTURE_PEAK_DB, abs=TOLERANCE)


def test_playback_spectrum_shows_the_pipeline_gain(cdsp):
    """Same tone, same bin, 6 dB down through the filter."""
    capture = peak_of(spectrum(cdsp))
    playback = peak_of(spectrum(cdsp, side="playback"))
    assert playback[0] == capture[0]
    assert playback[1] == pytest.approx(PLAYBACK_PEAK_DB, abs=TOLERANCE)
    # The two sides are read in separate calls, so this only holds because neither
    # reading depends on where its window fell. At 1 kHz it would not.
    assert capture[1] - playback[1] == pytest.approx(6.0, abs=2 * TOLERANCE)


def test_nothing_else_is_in_the_signal(cdsp):
    """A pure sine leaves every bin away from the tone far down in the noise."""
    data = spectrum(cdsp)
    peak_frequency, _ = peak_of(data)
    away = [
        magnitude
        for frequency, magnitude in zip(data["frequencies"], data["magnitudes"])
        if abs(frequency / peak_frequency - 1.0) > 0.5
    ]
    assert away
    assert max(away) < -60.0


def test_a_single_channel_matches_the_average(cdsp):
    """Both channels carry the same sine, so selecting one changes nothing."""
    frequency, magnitude = peak_of(spectrum(cdsp))
    for channel in (0, 1):
        one = peak_of(spectrum(cdsp, channel=channel))
        assert one[0] == frequency
        # Not an exact comparison: averaging the channels and taking one of them are
        # different sums, and they differ in the last f32 digit.
        assert one[1] == pytest.approx(magnitude, abs=TOLERANCE)


@pytest.mark.parametrize("n_bins", [2, 16, 64, 512])
def test_the_requested_number_of_bins_comes_back(cdsp, n_bins):
    data = spectrum(cdsp, n_bins=n_bins)
    assert len(data["frequencies"]) == n_bins
    assert len(data["magnitudes"]) == n_bins
    assert data["frequencies"][0] == pytest.approx(REQUEST["min_freq"])
    assert data["frequencies"][-1] == pytest.approx(REQUEST["max_freq"])


@pytest.mark.parametrize(
    "overrides, message",
    [
        ({"n_bins": 1}, "n_bins must be at least 2"),
        ({"min_freq": 0.0}, "Invalid frequency range"),
        ({"min_freq": 20000.0, "max_freq": 20.0}, "Invalid frequency range"),
        ({"channel": 5}, "Channel 5 out of range"),
    ],
)
def test_a_bad_request_is_refused(cdsp, overrides, message):
    spectrum(cdsp)
    reply = cdsp.send_raw("GetSpectrum", {**REQUEST, **overrides})
    assert reply["result"] == "InvalidRequestError"
    assert message in reply["message"]


def test_no_spectrum_without_a_running_pipeline(start_cdsp):
    cdsp = start_cdsp(extra_args=["--wait"])
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    # Both spectrum commands decide this from the active config's sample rate, not from
    # the state, and Stop clears the config only after the pipeline is down,
    # `src/engine.rs:159`. So the state reaches Inactive first, and a request made in
    # between still finds a rate and fails on the empty buffer instead. Gate on the
    # config, which is the thing they actually read.
    cdsp.poll_until_true("GetConfig", lambda text: text.strip() == "null")
    assert cdsp.send_raw("GetSpectrum", REQUEST)["result"] == "ProcessingNotRunningError"
    assert (
        cdsp.send_raw("SubscribeSpectrum", {**REQUEST, "max_rate": 20.0})["result"]
        == "ProcessingNotRunningError"
    )


def test_subscribed_spectra_peak_at_the_generator_frequency(cdsp):
    client = cdsp.new_client()
    assert client.send_raw("SubscribeSpectrum", {**REQUEST, "max_rate": 20.0})["result"] == "Ok"
    for _, data in client.recv_events(4, timeout=10.0):
        frequency, magnitude = peak_of(data)
        assert frequency == pytest.approx(TONE_HZ, rel=BIN_TOLERANCE)
        assert magnitude == pytest.approx(CAPTURE_PEAK_DB, abs=TOLERANCE)
    client.stop_subscription()


def test_max_rate_caps_the_push_rate(cdsp):
    """max_rate is a cap, not a cadence, so only the lower bound on the gap is fixed."""
    client = cdsp.new_client()
    client.send_raw("SubscribeSpectrum", {**REQUEST, "max_rate": 5.0})
    # The first gap is measured from the subscribe, not from a previous push, so drop it.
    times = [when for when, _ in client.recv_events(5, timeout=15.0)][1:]
    gaps = [later - earlier for earlier, later in zip(times, times[1:])]
    assert gaps
    # Pushes are quantized to the analysis hop, so the gap lands at or above 1/max_rate
    # rather than exactly on it. A tenth of a second of slack covers the hop.
    assert min(gaps) > 0.2 - 0.1
    client.stop_subscription()


@pytest.mark.parametrize(
    "overrides, message",
    [
        ({"n_bins": 1}, "n_bins must be at least 2"),
        ({"min_freq": 0.0}, "Invalid frequency range"),
        ({"max_rate": 0.0}, "max_rate must be > 0"),
    ],
)
def test_a_bad_subscription_is_refused(cdsp, overrides, message):
    request = {**REQUEST, "max_rate": 20.0, **overrides}
    reply = cdsp.send_raw("SubscribeSpectrum", request)
    assert reply["result"] == "InvalidRequestError"
    assert message in reply["message"]
    # A refused subscription must leave the connection unsubscribed, so an ordinary
    # command still works on it.
    assert cdsp.send("GetState") == "Running"
