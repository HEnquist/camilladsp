"""Spectrum analysis, one shot and subscribed.

The base config captures a 1 kHz sine at -6 dBFS and runs it through a -6 dB gain
filter, so both halves of the assertion are sharp: the peak has to land in the bin that
covers 1 kHz, and its magnitude has to be the level the generator was configured with.
"""

import time

import pytest

TONE_HZ = 1000.0
CAPTURE_PEAK_DB = -6.0
PLAYBACK_PEAK_DB = -12.0
# The tone does not sit on an FFT bin centre, so the Hann window's scalloping loss puts
# the reading a little over half a dB low. It is stable at that, not noisy.
TOLERANCE = 1.0

# 64 log spaced bins from 20 Hz to 20 kHz is a ratio of 1.116 between neighbours, so a
# peak in the right bin is within 12 % of the tone and one bin out is already outside
# this. Sharp, over a range spanning three decades.
REQUEST = {
    "side": "capture",
    "channel": None,
    "min_freq": 20.0,
    "max_freq": 20000.0,
    "n_bins": 64,
}
BIN_TOLERANCE = 0.15


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
    assert capture[1] - playback[1] == pytest.approx(6.0, abs=0.01)


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
    both = peak_of(spectrum(cdsp))
    assert peak_of(spectrum(cdsp, channel=0)) == both
    assert peak_of(spectrum(cdsp, channel=1)) == both


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
