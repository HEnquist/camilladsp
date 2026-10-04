"""The built-in controller: error recovery, following a rate change, and its settings.

The controller decides what runs after a session ends. With its features off it goes
idle, which `test_failures.py` covers. These tests turn them on: error recovery retries
the config that was running with a growing backoff, and following picks the config for
the rate a capture reports.

The dummy control socket makes the device events reachable, see `test_failures.py`. A
failed dummy device is rebuilt with its flags clear, so a retry finds it working again.

The settings live in the `controller` section of the statefile, and the CLI args and
`SetControllerSettings` write them there.
"""

import subprocess
import time

import pytest
import yaml

EXIT_OK = 0
CLAP_USAGE_ERROR = 2
EXIT_BAD_CONFIG = 101

# A short measurement window, so a rate change is noticed in about a second, see
# test_failures.py.
MEASURE_SETTINGS = {
    "  chunksize: 1024\n": "  chunksize: 1024\n  rate_measure_interval_s: 0.2\n",
}
STOPPING_MEASURE_SETTINGS = {
    "  chunksize: 1024\n": (
        "  chunksize: 1024\n  rate_measure_interval_s: 0.2\n  stop_on_rate_change: true\n"
    ),
}

# A playback that fails every time it opens, since its directory isn't there. Validation
# doesn't look at the output path, so the config is accepted.
FAILING_PLAYBACK = {
    "  playback:\n    type: Dummy\n    channels: 2\n": (
        "  playback:\n    type: File\n    channels: 2\n"
        "    filename: /no/such/dir/out.raw\n    format: S32_LE\n"
    ),
}


@pytest.fixture
def statefile(tmp_path):
    return str(tmp_path / "state.yml")


def status(cdsp):
    return cdsp.send("GetControllerStatus")


def read_state(path):
    with open(path) as state:
        return yaml.safe_load(state)


@pytest.mark.parametrize("device", ["capture", "playback"])
def test_recovery_restarts_after_a_device_error(control_cdsp, device):
    cdsp = control_cdsp(extra_args=["--wait", "--error_recovery"])
    assert status(cdsp)["error_recovery"] is True
    control = cdsp.capture_control if device == "capture" else cdsp.playback_control
    control.set("error", 1)
    retry = cdsp.poll_until_true("GetControllerStatus", lambda s: s["recovering"])
    assert retry["attempts"] == 1
    assert list(retry["stop_reason"]) == [
        "CaptureError" if device == "capture" else "PlaybackError"
    ]
    # The first retry comes after a second, and the rebuilt device works.
    cdsp.poll_until("GetState", "Running", timeout=5.0)
    cdsp.poll_until("GetStopReason", "None")
    assert status(cdsp)["recovering"] is False


def test_without_recovery_an_error_goes_idle(control_cdsp):
    cdsp = control_cdsp(extra_args=["--wait"])
    cdsp.capture_control.set("error", 1)
    cdsp.poll_until_true("GetStopReason", lambda v: v != "None")
    cdsp.poll_until("GetState", "Inactive")
    time.sleep(1.5)
    assert cdsp.send("GetState") == "Inactive"
    assert status(cdsp)["recovering"] is False


def test_a_device_that_keeps_failing_backs_off(start_cdsp, config_file):
    """A device that fails at open is retried after 1 s, then 2 s, and so on."""
    config = config_file(FAILING_PLAYBACK)
    cdsp = start_cdsp(
        config=config, extra_args=["--wait", "--error_recovery"], wait_for_running=False
    )
    second = cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["attempts"] == 2 and s["next_retry_s"] is not None
    )
    assert 1.0 < second["next_retry_s"] <= 2.0
    assert second["recovering"] is True
    assert list(second["stop_reason"]) == ["PlaybackError"]
    assert cdsp.send("GetState") == "Inactive"


def test_a_stop_cancels_the_recovery(start_cdsp, config_file):
    cdsp = start_cdsp(
        config=config_file(FAILING_PLAYBACK),
        extra_args=["--wait", "--error_recovery"],
        wait_for_running=False,
    )
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["recovering"])
    cdsp.send("Stop")
    stopped = cdsp.poll_until_true("GetControllerStatus", lambda s: not s["recovering"])
    assert stopped["attempts"] == 0
    assert stopped["next_retry_s"] is None


def test_a_new_config_cancels_the_recovery(start_cdsp, config_file):
    cdsp = start_cdsp(
        config=config_file(FAILING_PLAYBACK),
        extra_args=["--wait", "--error_recovery"],
        wait_for_running=False,
    )
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["recovering"])
    with open(config_file()) as conf:
        cdsp.send("SetConfig", conf.read())
    cdsp.poll_until("GetState", "Running")
    assert status(cdsp)["attempts"] == 0


def test_the_end_of_a_stream_is_not_retried(control_cdsp):
    cdsp = control_cdsp(extra_args=["--wait", "--error_recovery"])
    cdsp.capture_control.set("eof", 1)
    cdsp.poll_until("GetStopReason", "Done")
    cdsp.poll_until("GetState", "Inactive")
    time.sleep(1.5)
    assert cdsp.send("GetState") == "Inactive"
    assert status(cdsp)["recovering"] is False


def test_a_format_change_without_following_is_not_retried(control_cdsp):
    """Rule 6: a format change is following's job, recovery leaves it alone."""
    cdsp = control_cdsp(STOPPING_MEASURE_SETTINGS, extra_args=["--wait", "--error_recovery"])
    cdsp.capture_control.set("rate", 44100)
    # The reason carries the measured rate, which is only near 44100.
    reason = cdsp.poll_until_true("GetStopReason", lambda v: v != "None")
    assert list(reason) == ["CaptureFormatChange"]
    cdsp.poll_until("GetState", "Inactive")
    time.sleep(1.5)
    assert cdsp.send("GetState") == "Inactive"


def test_adapt_follows_a_measured_rate_change(control_cdsp):
    """The dummy capture measures its rate, like ALSA without the loopback controls.
    Following forces stop_on_rate_change on, so the config doesn't need it."""
    cdsp = control_cdsp(MEASURE_SETTINGS, extra_args=["--wait", "--follow_adapt"])
    cdsp.capture_control.set("rate", 44100)
    deadline = time.monotonic() + 10.0
    while cdsp.send_raw("GetConfigValue", "/devices/samplerate").get("value") != 44100:
        assert time.monotonic() < deadline, "did not follow to 44100"
        time.sleep(0.05)
    cdsp.poll_until("GetState", "Running")
    cdsp.poll_until("GetStopReason", "None")


def test_settings_round_trip_and_persist(start_cdsp, statefile):
    cdsp = start_cdsp(extra_args=["--statefile", statefile])
    assert cdsp.send("GetControllerSettings") is None
    assert "controller" not in read_state(statefile)
    settings = {
        "follow_capture": {"specific": "/configs/conf_$samplerate$.yml", "adapt": True},
        "error_recovery": True,
    }
    cdsp.send("SetControllerSettings", settings)
    assert cdsp.send("GetControllerSettings") == settings
    cdsp.poll_until("GetStateFileUpdated", True)
    assert read_state(statefile)["controller"] == settings
    assert cdsp.exit() == EXIT_OK

    restarted = start_cdsp(config=None, extra_args=["--statefile", statefile])
    assert restarted.send("GetControllerSettings") == settings
    # Not in wait mode, so neither feature is on.
    current = status(restarted)
    assert current["following"] is False
    assert current["error_recovery"] is False


@pytest.mark.parametrize(
    "settings",
    [
        {"follow_capture": {"specific": "/configs/conf.yml"}},
        {"follow_capture": {"adapt": False}},
        {"follow_capture": {}},
    ],
)
def test_invalid_settings_are_rejected(cdsp, settings):
    reply = cdsp.send_raw("SetControllerSettings", settings)
    assert "InvalidValueError" in reply["result"]
    assert cdsp.send("GetControllerSettings") is None


def test_settings_can_be_removed(start_cdsp, statefile):
    cdsp = start_cdsp(extra_args=["--statefile", statefile, "--wait", "--follow_adapt"])
    assert cdsp.send("GetControllerSettings") == {
        "follow_capture": {"specific": None, "adapt": True},
        "error_recovery": None,
    }
    # The client leaves a None value out, so send the null explicitly.
    reply = cdsp.send_text('{"command": "SetControllerSettings", "value": null}')
    assert reply["result"] == "Ok"
    assert cdsp.send("GetControllerSettings") is None


@pytest.mark.parametrize(
    "args", [["--follow_adapt"], ["--follow_specific", "c_$samplerate$.yml"], ["--error_recovery"]]
)
def test_the_cli_args_need_wait_mode(camilladsp_bin, args):
    result = subprocess.run(
        [camilladsp_bin, "-p", "1234", *args, "dummy_sine.yml"], capture_output=True
    )
    assert result.returncode == CLAP_USAGE_ERROR


def test_a_template_without_tokens_is_refused_at_startup(camilladsp_bin, statefile):
    result = subprocess.run(
        [camilladsp_bin, "-p", "1234", "-w", "--follow_specific", "conf.yml"],
        capture_output=True,
        timeout=10,
    )
    assert result.returncode == EXIT_BAD_CONFIG


def test_the_cli_args_are_written_to_the_statefile(start_cdsp, statefile):
    """Each arg sets its own field, on top of what the statefile already has."""
    first = start_cdsp(extra_args=["--statefile", statefile, "--wait", "--error_recovery"])
    assert first.exit() == EXIT_OK
    assert read_state(statefile)["controller"] == {
        "follow_capture": None,
        "error_recovery": True,
    }
    second = start_cdsp(
        config=None, extra_args=["--statefile", statefile, "--wait", "--follow_adapt"]
    )
    assert second.send("GetControllerSettings") == {
        "follow_capture": {"specific": None, "adapt": True},
        "error_recovery": True,
    }
    assert second.exit() == EXIT_OK
    assert read_state(statefile)["controller"]["error_recovery"] is True
