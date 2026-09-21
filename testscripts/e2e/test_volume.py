"""Volume, mute and the five faders.

Most of this is a round trip over the control plane, but the last few cases assert on
the playback level instead, so they prove the Main fader actually reaches the audio and
not only the status struct.
"""

import pytest

NUM_FADERS = 5
# The global range every fader is clamped to, see the SetVolume docs in
# src/websocket_server/datastructures.rs.
MIN_VOLUME = -150.0
MAX_VOLUME = 50.0

# The base config captures a sine at -6 dBFS and runs it through a -6 dB gain filter.
CAPTURE_PEAK_DB = -6.0
PLAYBACK_PEAK_DB = -12.0


def settled_playback_peak(cdsp, expected, tolerance=0.5):
    """Wait for the playback peak to reach `expected` dB and return it.

    Volume changes are ramped over volume_ramp_time_ms, 400 ms by default, so the level
    is polled rather than read once.
    """
    return cdsp.poll_until_true(
        "GetPlaybackSignalPeak",
        lambda peaks: peaks and all(abs(peak - expected) < tolerance for peak in peaks),
        timeout=5.0,
    )


def test_volume_round_trip(cdsp):
    cdsp.send("SetVolume", -12.5)
    assert cdsp.send("GetVolume") == pytest.approx(-12.5)


@pytest.mark.parametrize("asked, expected", [(100.0, MAX_VOLUME), (-400.0, MIN_VOLUME)])
def test_volume_is_clamped_to_the_global_range(cdsp, asked, expected):
    cdsp.send("SetVolume", asked)
    assert cdsp.send("GetVolume") == pytest.approx(expected)


def test_adjust_volume_returns_the_new_value(cdsp):
    cdsp.send("SetVolume", -10.0)
    assert cdsp.send("AdjustVolume", -5.0) == pytest.approx(-15.0)
    assert cdsp.send("GetVolume") == pytest.approx(-15.0)


def test_adjust_volume_respects_its_own_limits(cdsp):
    cdsp.send("SetVolume", -10.0)
    assert cdsp.send("AdjustVolume", -20.0, min=-12.0) == pytest.approx(-12.0)
    assert cdsp.send("AdjustVolume", 20.0, max=-2.0) == pytest.approx(-2.0)


def test_mute_round_trip(cdsp):
    assert cdsp.send("GetMute") is False
    cdsp.send("SetMute", True)
    assert cdsp.send("GetMute") is True
    cdsp.send("SetMute", False)
    assert cdsp.send("GetMute") is False


def test_toggle_mute_returns_the_new_state(cdsp):
    assert cdsp.send("ToggleMute") is True
    assert cdsp.send("GetMute") is True
    assert cdsp.send("ToggleMute") is False


@pytest.mark.parametrize("fader", range(NUM_FADERS))
def test_fader_volume_round_trip(cdsp, fader):
    """Every fader should hold its own value, Main and the four aux ones alike."""
    volume = -3.0 - fader
    assert cdsp.send("SetFaderVolume", volume, fader=fader) is None
    assert cdsp.send("GetFaderVolume", fader=fader) == [fader, pytest.approx(volume)]


@pytest.mark.parametrize("fader", range(NUM_FADERS))
def test_fader_mute_round_trip(cdsp, fader):
    cdsp.send("SetFaderMute", True, fader=fader)
    assert cdsp.send("GetFaderMute", fader=fader) == [fader, True]
    assert cdsp.send("ToggleFaderMute", fader=fader) == [fader, False]


def test_adjust_fader_volume(cdsp):
    cdsp.send("SetFaderVolume", -10.0, fader=2)
    assert cdsp.send("AdjustFaderVolume", -5.0, fader=2) == [2, pytest.approx(-15.0)]
    assert cdsp.send("AdjustFaderVolume", -20.0, fader=2, min=-20.0) == [
        2,
        pytest.approx(-20.0),
    ]


def test_set_fader_external_volume(cdsp):
    """The external volume setter feeds the same fader value back to GetVolume."""
    cdsp.send("SetFaderExternalVolume", -20.0, fader=0)
    assert cdsp.send("GetVolume") == pytest.approx(-20.0)


def test_get_faders_matches_the_individual_getters(cdsp):
    """The bulk getter is what a GUI polls, so it has to agree with the single ones."""
    for fader in range(NUM_FADERS):
        cdsp.send("SetFaderVolume", -2.0 * fader, fader=fader)
        cdsp.send("SetFaderMute", fader % 2 == 0, fader=fader)

    faders = cdsp.send("GetFaders")
    assert len(faders) == NUM_FADERS
    for fader, state in enumerate(faders):
        assert state["volume"] == pytest.approx(-2.0 * fader)
        assert state["mute"] is (fader % 2 == 0)


@pytest.mark.parametrize(
    "command, extra",
    [
        ("GetFaderVolume", {}),
        ("SetFaderVolume", {"value": -3.0}),
        ("GetFaderMute", {}),
        ("SetFaderMute", {"value": True}),
        ("ToggleFaderMute", {}),
        ("AdjustFaderVolume", {"value": -3.0}),
        ("SetFaderExternalVolume", {"value": -3.0}),
    ],
)
def test_an_unknown_fader_is_an_error(cdsp, command, extra):
    """Every fader command should refuse an index outside the five that exist."""
    reply = cdsp.send_raw(command, fader=NUM_FADERS, **extra)
    assert reply["result"] == "InvalidFaderError"


def test_the_main_fader_changes_the_playback_level(cdsp):
    """The Main fader is applied by a Volume filter the pipeline always ends with."""
    settled_playback_peak(cdsp, PLAYBACK_PEAK_DB)
    cdsp.send("SetVolume", -20.0)
    settled_playback_peak(cdsp, PLAYBACK_PEAK_DB - 20.0)


def test_mute_floors_the_playback_level(cdsp):
    """Muted means silent, not merely quiet."""
    settled_playback_peak(cdsp, PLAYBACK_PEAK_DB)
    cdsp.send("SetMute", True)
    cdsp.poll_until_true(
        "GetPlaybackSignalPeak",
        lambda peaks: peaks and all(peak < -100.0 for peak in peaks),
        timeout=5.0,
    )
    # And the capture side is untouched, so this really is the fader and not the signal.
    assert all(
        peak == pytest.approx(CAPTURE_PEAK_DB, abs=0.5)
        for peak in cdsp.send("GetCaptureSignalPeak")
    )


def test_volume_limit_clamps_the_playback_level(start_cdsp, config_file):
    """volume_limit caps what the fader can do, without capping the fader itself."""
    limit = -20.0
    cdsp = start_cdsp(
        config=config_file({"chunksize: 1024": f"chunksize: 1024\n  volume_limit: {limit}"})
    )
    cdsp.send("SetVolume", 0.0)
    # The fader reports what it was told, the audio gets the limited value.
    assert cdsp.send("GetVolume") == pytest.approx(0.0)
    settled_playback_peak(cdsp, PLAYBACK_PEAK_DB + limit)
