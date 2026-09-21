"""Lifecycle: coming up, going down, and the signals in between.

Unglamorous and high value per line. Every one of these catches a "the binary is broken"
regression that nothing in the unit tests can see, because none of it exists until the
process, the supervisor and the control plane are all real.
"""

import os
import signal
import sys

import pytest

# CamillaDSP registers SIGHUP and SIGUSR1 only on non-Windows, and on Windows it polls a
# flag for SIGINT alone, see src/engine_process_signals.rs. Delivering even that one from
# Python needs a console control event against a separate process group, which would
# reach pytest itself, so the signal cases are all Unix here.
ON_WINDOWS = sys.platform == "win32"
unix_only = pytest.mark.skipif(ON_WINDOWS, reason="no unix signals on Windows")

EXIT_OK = 0
EXIT_BAD_CONFIG = 101


def test_starts_and_reaches_running(cdsp):
    """A config on the command line should take the engine all the way to Running."""
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"


def test_wait_starts_inactive_and_runs_on_set_config(start_cdsp):
    """With --wait and no config, the engine should stand by until one arrives."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    assert cdsp.send("GetState") == "Inactive"
    assert cdsp.send("GetConfigFilePath") is None

    with open(os.path.join(os.path.dirname(__file__), "dummy_sine.yml")) as conf:
        cdsp.send("SetConfig", conf.read())
    cdsp.poll_until("GetState", "Running")


def test_stop_returns_to_inactive(start_cdsp):
    """With --wait, Stop should close the devices but leave the process alive.

    Without --wait the supervisor exits once Stop clears the active config, so the
    waiting behaviour only exists in this mode.
    """
    cdsp = start_cdsp(extra_args=["--wait"])
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


def test_stop_without_wait_exits(start_cdsp):
    """Without --wait there is nothing left to do after Stop, so the process ends."""
    cdsp = start_cdsp()
    cdsp.send("Stop")
    assert cdsp.process.wait(timeout=10) == EXIT_OK


def test_stop_leaves_the_stop_reason_alone(start_cdsp):
    """A stop asked for over the websocket is not a reason for stopping."""
    cdsp = start_cdsp(extra_args=["--wait"])
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    assert cdsp.send("GetStopReason") == "None"


def test_exit_is_clean(cdsp):
    """Exit should shut the process down with the clean exit code."""
    assert cdsp.exit() == EXIT_OK


def test_exit_from_standby(start_cdsp):
    """Exit should also work when there is no config and nothing is running."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    assert cdsp.exit() == EXIT_OK


def test_bad_config_on_startup(spawn_cdsp, config_file):
    """A config that does not parse should stop the process before it ever runs."""
    broken = config_file({"chunksize: 1024": "chunksize: this is not a number"})
    process, _port = spawn_cdsp(config=broken)
    assert process.wait(timeout=20) == EXIT_BAD_CONFIG


def test_missing_config_on_startup(spawn_cdsp, tmp_path):
    """So should a config file that is not there at all."""
    process, _port = spawn_cdsp(config=str(tmp_path / "nothing_here.yml"))
    assert process.wait(timeout=20) == EXIT_BAD_CONFIG


@unix_only
@pytest.mark.parametrize("sig", ["SIGINT", "SIGTERM"])
def test_termination_signals_exit_cleanly(cdsp, sig):
    """Both terminating signals should take the same clean path as Exit."""
    cdsp.process.send_signal(getattr(signal, sig))
    assert cdsp.process.wait(timeout=10) == EXIT_OK


@unix_only
def test_sigusr1_stops_processing(start_cdsp):
    """SIGUSR1 is a Stop, so under --wait it should end in standby."""
    cdsp = start_cdsp(extra_args=["--wait"])
    cdsp.process.send_signal(signal.SIGUSR1)
    cdsp.poll_until("GetState", "Inactive")
    assert cdsp.is_running()
    assert cdsp.exit() == EXIT_OK


@unix_only
def test_sighup_reloads_the_config_from_disk(start_cdsp, config_file):
    """SIGHUP should reread the active path, the same as Reload."""
    path = config_file()
    cdsp = start_cdsp(config=path)
    config_file({"gain: -6.0": "gain: -10.0"}, path=path)

    cdsp.process.send_signal(signal.SIGHUP)
    cdsp.poll_until_true(
        "GetConfigJson", lambda text: '"gain":-10.0' in text.replace(" ", "")
    )
    assert cdsp.send("GetState") == "Running"


@unix_only
def test_sighup_with_a_broken_config_keeps_the_running_one(start_cdsp, config_file):
    """A reload that cannot be read should be logged and dropped, not applied."""
    path = config_file()
    cdsp = start_cdsp(config=path)
    with open(path, "w") as conf:
        conf.write("devices:\n  samplerate: 48000\n")

    cdsp.process.send_signal(signal.SIGHUP)
    # Nothing observable changes on a rejected reload, so give the signal a moment to be
    # handled before checking that it was not, then confirm the engine is untouched.
    cdsp.poll_until("GetState", "Running", timeout=2.0)
    assert cdsp.send("GetConfigValue", "/devices/samplerate") == 48000
    assert cdsp.is_running()


@unix_only
def test_sighup_without_a_config_path(start_cdsp):
    """With nothing to reload, SIGHUP should log the problem and change nothing."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    cdsp.process.send_signal(signal.SIGHUP)
    cdsp.poll_until("GetState", "Inactive", timeout=2.0)
    assert cdsp.is_running()
