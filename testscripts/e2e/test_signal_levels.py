"""Signal levels and metering.

A known sine through a known gain makes every level getter assertable against a
computed value rather than against "something nonzero", so these are sharp tests.

The base config captures a 1 kHz sine at -6 dBFS and runs it through a -6 dB gain
filter. A sine's RMS is 3.01 dB below its peak, which gives the four numbers below.

Every level getter returns an empty list until the first record lands,
`src/websocket_server/utils.rs:476`, not only the since-last ones. So a predicate here
has to check the list before indexing it, and a predicate built on `all()` has to check
it too, since `all([])` is True and would report silence as soon as the engine starts.
"""

import pytest

CAPTURE_PEAK_DB = -6.0
CAPTURE_RMS_DB = -9.01
PLAYBACK_PEAK_DB = -12.0
PLAYBACK_RMS_DB = -15.01
# Generous next to the numbers above, which land within a hundredth of a dB, but the
# last chunk of a sine is not a whole number of periods so the RMS wobbles a little.
TOLERANCE = 0.2


def levels(cdsp, command):
    """Read a per-channel level getter once the devices have produced real numbers.

    Everything here reports per channel, and reads back as silence for the first
    fraction of a second after a start, so poll rather than sleep.
    """
    return cdsp.poll_until_true(
        command,
        lambda values: len(values) == 2 and all(level > -100.0 for level in values),
        timeout=5.0,
    )


def assert_close(values, expected):
    assert len(values) == 2
    assert all(level == pytest.approx(expected, abs=TOLERANCE) for level in values)


def test_signal_range(cdsp):
    """A -6 dBFS sine swings between -0.5 and +0.5, so the range is 1.0."""
    cdsp.poll_until_true("GetSignalRange", lambda value: value > 0.0)
    assert cdsp.send("GetSignalRange") == pytest.approx(1.0, abs=0.05)


def test_capture_levels(cdsp):
    """The capture side sees the generator's own level."""
    assert_close(levels(cdsp, "GetCaptureSignalPeak"), CAPTURE_PEAK_DB)
    assert_close(levels(cdsp, "GetCaptureSignalRms"), CAPTURE_RMS_DB)


def test_playback_levels_show_the_pipeline_gain(cdsp):
    """The playback side sees it after the -6 dB filter, which is the whole point."""
    assert_close(levels(cdsp, "GetPlaybackSignalPeak"), PLAYBACK_PEAK_DB)
    assert_close(levels(cdsp, "GetPlaybackSignalRms"), PLAYBACK_RMS_DB)


@pytest.mark.parametrize(
    "command, expected",
    [
        ("GetCaptureSignalPeakSince", CAPTURE_PEAK_DB),
        ("GetCaptureSignalRmsSince", CAPTURE_RMS_DB),
        ("GetPlaybackSignalPeakSince", PLAYBACK_PEAK_DB),
        ("GetPlaybackSignalRmsSince", PLAYBACK_RMS_DB),
    ],
)
def test_levels_over_a_time_window(cdsp, command, expected):
    """A windowed average of a steady signal is the same number as the last chunk."""
    levels(cdsp, "GetCaptureSignalPeak")
    assert_close(cdsp.send(command, 0.2), expected)


@pytest.mark.parametrize(
    "command, expected",
    [
        ("GetCaptureSignalPeakSinceLast", CAPTURE_PEAK_DB),
        ("GetCaptureSignalRmsSinceLast", CAPTURE_RMS_DB),
        ("GetPlaybackSignalPeakSinceLast", PLAYBACK_PEAK_DB),
        ("GetPlaybackSignalRmsSinceLast", PLAYBACK_RMS_DB),
    ],
)
def test_levels_since_the_last_call(cdsp, command, expected):
    """The since-last getters are per client, and empty when nothing new arrived."""
    assert_close(
        cdsp.poll_until_true(command, lambda values: values and values[0] > -100.0), expected
    )
    # Called again before another chunk has been analyzed there is nothing to report,
    # and the reply is an empty list rather than a stale repeat. A chunk lasts 21 ms
    # here and a round trip is a fraction of a millisecond, so a short burst is certain
    # to contain calls with no new data, while any single one of them is a coin toss.
    assert [] in [cdsp.send(command) for _ in range(5)]


def test_all_levels_in_one_request(cdsp):
    """GetSignalLevels has to agree with the four getters it bundles."""
    both = cdsp.poll_until_true(
        "GetSignalLevels",
        lambda value: value["capture_peak"] and value["capture_peak"][0] > -100.0,
    )
    assert_close(both["capture_peak"], CAPTURE_PEAK_DB)
    assert_close(both["capture_rms"], CAPTURE_RMS_DB)
    assert_close(both["playback_peak"], PLAYBACK_PEAK_DB)
    assert_close(both["playback_rms"], PLAYBACK_RMS_DB)


def test_all_levels_over_a_window(cdsp):
    levels(cdsp, "GetCaptureSignalPeak")
    both = cdsp.send("GetSignalLevelsSince", 0.2)
    assert_close(both["capture_peak"], CAPTURE_PEAK_DB)
    assert_close(both["playback_rms"], PLAYBACK_RMS_DB)


def test_all_levels_since_the_last_call(cdsp):
    # Polled faster than chunks arrive this comes back empty, so the predicate has to
    # tolerate that, the same as the single since-last getters do.
    # The two sides are filled in independently, so one can be ready while the other is
    # still empty, and polling on only one of them races the other.
    both = cdsp.poll_until_true(
        "GetSignalLevelsSinceLast",
        lambda value: all(value[side] for side in value) and value["capture_peak"][0] > -100.0,
    )
    assert_close(both["capture_peak"], CAPTURE_PEAK_DB)
    assert_close(both["playback_peak"], PLAYBACK_PEAK_DB)


def test_peaks_since_start_are_amplitudes(cdsp):
    """This one reports linear amplitude, unlike every other level getter."""
    peaks = cdsp.poll_until_true(
        "GetSignalPeaksSinceStart",
        lambda value: value["capture"] and value["capture"][0] > 0.0,
    )
    # -6 dBFS is an amplitude of 0.501, and -12 dBFS is 0.251.
    assert peaks["capture"] == pytest.approx([0.501, 0.501], abs=0.01)
    assert peaks["playback"] == pytest.approx([0.251, 0.251], abs=0.01)


def test_resetting_the_peaks_since_start(cdsp):
    """After a reset the peaks have to climb back up from silence."""
    cdsp.poll_until_true(
        "GetSignalPeaksSinceStart",
        lambda value: value["capture"] and value["capture"][0] > 0.0,
    )
    cdsp.send("SetMute", True)
    # Wait out the volume ramp, otherwise the reset catches the tail of it.
    cdsp.poll_until_true(
        "GetPlaybackSignalPeak",
        lambda peaks: peaks and all(peak < -100.0 for peak in peaks),
        timeout=5.0,
    )
    cdsp.send("ResetSignalPeaksSinceStart")
    peaks = cdsp.send("GetSignalPeaksSinceStart")
    assert max(peaks["playback"]) < 0.01
    # The capture side is upstream of the fader, so it keeps climbing.
    assert cdsp.poll_until_true(
        "GetSignalPeaksSinceStart",
        lambda value: value["capture"] and value["capture"][0] > 0.4,
    )


def test_channel_labels_are_absent_by_default(cdsp):
    labels = cdsp.send("GetChannelLabels")
    assert labels == {"playback": None, "capture": None}


def test_channel_labels_from_the_capture_device(start_cdsp, config_file):
    """Labels configured on the capture device should come back over the socket."""
    labelled = '    channels: 2\n    labels: ["left", "right"]\n    signal:'
    cdsp = start_cdsp(config=config_file({"    channels: 2\n    signal:": labelled}))
    labels = cdsp.send("GetChannelLabels")
    assert labels["capture"] == ["left", "right"]


def test_channel_labels_from_a_mixer(start_cdsp):
    """A mixer's labels describe the playback side, which is what it produced."""
    cdsp = start_cdsp(config="dummy_mixer.yml")
    assert cdsp.send("GetChannelLabels")["capture"] is None
