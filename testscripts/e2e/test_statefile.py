"""The state file: what survives a restart.

The saving thread is notified on a change and then debounced, so a test must poll
GetStateFileUpdated rather than guess at a delay. That getter is what makes this
testable at all without waiting out a fixed second.
"""

import pytest


@pytest.fixture
def statefile(tmp_path):
    return str(tmp_path / "state.yml")


def test_no_state_file_by_default(cdsp):
    assert cdsp.send("GetStateFilePath") is None


def test_state_file_path_is_reported(start_cdsp, statefile):
    cdsp = start_cdsp(extra_args=["--statefile", statefile])
    assert cdsp.send("GetStateFilePath") == statefile


def test_volume_and_mute_survive_a_restart(start_cdsp, statefile):
    """The whole point of the state file: come back up where you left off."""
    cdsp = start_cdsp(extra_args=["--statefile", statefile])
    cdsp.send("SetVolume", -12.5)
    cdsp.send("SetFaderVolume", -7.5, fader=3)
    cdsp.send("SetFaderMute", True, fader=2)
    cdsp.poll_until("GetStateFileUpdated", True)
    assert cdsp.exit() == 0

    # Only the state file this time, so the config path has to come from it as well.
    restarted = start_cdsp(config=None, extra_args=["--statefile", statefile])
    assert restarted.send("GetVolume") == pytest.approx(-12.5)
    faders = restarted.send("GetFaders")
    assert faders[3]["volume"] == pytest.approx(-7.5)
    assert faders[2]["mute"] is True


def test_the_config_path_is_remembered(start_cdsp, statefile, config_file):
    """A restart on the state file alone should reload the config it was last using."""
    path = config_file()
    cdsp = start_cdsp(config=path, extra_args=["--statefile", statefile])
    cdsp.poll_until("GetStateFileUpdated", True)
    assert cdsp.exit() == 0

    restarted = start_cdsp(config=None, extra_args=["--statefile", statefile])
    assert restarted.send("GetConfigFilePath") == path
    assert restarted.send("GetState") == "Running"


def test_no_config_ignores_the_remembered_path(start_cdsp, statefile):
    """--no_config is how a restart is kept in standby despite a saved config."""
    cdsp = start_cdsp(extra_args=["--statefile", statefile])
    cdsp.poll_until("GetStateFileUpdated", True)
    assert cdsp.exit() == 0

    restarted = start_cdsp(
        config=None,
        extra_args=["--statefile", statefile, "--wait", "--no_config"],
        wait_for_running=False,
    )
    assert restarted.send("GetState") == "Inactive"
    assert restarted.send("GetConfigFilePath") is None


def test_pending_changes_are_reported_as_unsaved(start_cdsp, statefile):
    """GetStateFileUpdated is what a shutdown script would poll before pulling power."""
    cdsp = start_cdsp(extra_args=["--statefile", statefile])
    cdsp.poll_until("GetStateFileUpdated", True)
    cdsp.send("SetVolume", -20.0)
    assert cdsp.send("GetStateFileUpdated") is False
    cdsp.poll_until("GetStateFileUpdated", True)
