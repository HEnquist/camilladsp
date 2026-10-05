"""The built-in controller following a simulated source, and waiting for it.

The dummy capture asks a `SimulatedSource` run by the test what its source is doing,
the way the ALSA backend asks a loopback. That covers the parts of following that need
the source to be asked rather than to report a change: the query at startup, and waiting
for the source when no provider has a config for its format (rules 14, 21 and 22 of the
controller plan). While following, the capture stops for a source at another rate or
channel count, like a loopback capture the kernel stops, so this runs the whole following
loop on any machine.

The entry config is 2 channels at 48 kHz, and Adapt is the only provider, so a source at
4 channels has no config and the controller waits.
"""

import time

import pytest

from conftest import CAPTURE_PORT_PLACEHOLDER, PLAYBACK_PORT_PLACEHOLDER, free_port
from dummyctl import SimulatedSource

FOLLOW = ["--wait", "--follow_adapt"]


@pytest.fixture
def source():
    src = SimulatedSource()
    yield src
    src.close()


@pytest.fixture
def source_cdsp(start_cdsp, config_file, source):
    """Factory for a CamillaDSP whose dummy capture asks `source`."""

    def _start(extra_args=FOLLOW, **kwargs):
        config = config_file(source_edits(source), base="dummy_control.yml")
        return start_cdsp(config=config, extra_args=extra_args, **kwargs)

    return _start


def source_edits(source):
    """dummy_control.yml with the capture asking `source`, and no capture control socket."""
    return {
        CAPTURE_PORT_PLACEHOLDER: f"source_port: {source.port}",
        PLAYBACK_PORT_PLACEHOLDER: f"control_port: {free_port()}",
    }


def config_value(cdsp, pointer):
    """A value of the running config, or None while nothing runs."""
    reply = cdsp.send_raw("GetConfigValue", pointer)
    return reply.get("value") if reply.get("result") == "Ok" else None


def wait_for_rate(cdsp, rate, timeout=10.0):
    deadline = time.monotonic() + timeout
    while config_value(cdsp, "/devices/samplerate") != rate:
        if time.monotonic() > deadline:
            raise TimeoutError(f"not running at {rate} after {timeout} s")
        time.sleep(0.05)
    cdsp.poll_until("GetState", "Running", timeout=timeout)


def waiting_for(cdsp, timeout=10.0):
    """Wait for the controller to wait for the source, and return the format it waits on."""
    return cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is not None, timeout=timeout
    )["waiting_for_source"]


def start_waiting(source_cdsp, source):
    """A session at 48 kHz whose source switches to 4 channels, which has no config."""
    source.play(48000)
    cdsp = source_cdsp()
    wait_for_rate(cdsp, 48000)
    source.play(48000, channels=4)
    assert waiting_for(cdsp) == {"samplerate": 48000, "channels": 4, "format": "S32_LE"}
    cdsp.poll_until("GetState", "Inactive")
    return cdsp


def test_startup_asks_the_source_first(source_cdsp, source):
    """Rule 14: the first session opens at the source's rate, not the entry's."""
    source.play(44100)
    cdsp = source_cdsp()
    wait_for_rate(cdsp, 44100)
    assert cdsp.send("GetStopReason") == "None"


def test_a_source_change_is_followed(source_cdsp, source):
    source.play(48000)
    cdsp = source_cdsp()
    wait_for_rate(cdsp, 48000)
    for rate in (96000, 44100):
        source.play(rate)
        wait_for_rate(cdsp, rate)


def test_an_inactive_source_runs_the_entry_config(source_cdsp, source):
    """Nothing plays at startup, so the entry runs as is, and captures silence."""
    cdsp = source_cdsp()
    wait_for_rate(cdsp, 48000)
    cdsp.poll_until_true(
        "GetCaptureSignalPeak", lambda peaks: all(peak < -100.0 for peak in peaks)
    )


def test_an_unsupported_format_waits_and_keeps_waiting(source_cdsp, source):
    cdsp = start_waiting(source_cdsp, source)
    # Asked once a second, and still the same format.
    time.sleep(2.5)
    assert waiting_for(cdsp)["channels"] == 4
    assert cdsp.send("GetState") == "Inactive"
    # The stop reason carries the whole format the capture reported.
    assert cdsp.send("GetStopReason") == {
        "CaptureFormatChange": {"samplerate": 48000, "channels": 4, "format": "S32_LE"}
    }


def test_a_supported_format_ends_the_wait(source_cdsp, source):
    cdsp = start_waiting(source_cdsp, source)
    source.play(96000)
    wait_for_rate(cdsp, 96000, timeout=5.0)
    assert cdsp.send("GetControllerStatus")["waiting_for_source"] is None


def test_an_inactive_source_ends_the_wait_with_the_entry_config(source_cdsp, source):
    """The capture is then open and ready for the next player."""
    cdsp = start_waiting(source_cdsp, source)
    source.stop_playing()
    wait_for_rate(cdsp, 48000, timeout=5.0)
    assert cdsp.send("GetControllerStatus")["waiting_for_source"] is None
    # And the next player is followed.
    source.play(44100)
    wait_for_rate(cdsp, 44100)


def test_an_unknown_source_ends_the_wait_and_goes_idle(source_cdsp, source):
    """Polling can't help with a source that can't tell, so the controller stops waiting."""
    cdsp = start_waiting(source_cdsp, source)
    source.unknown()
    cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is None, timeout=5.0
    )
    source.play(48000)
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Inactive"


@pytest.mark.parametrize("command", ["Stop", "Exit"])
def test_a_command_ends_the_wait(source_cdsp, source, command):
    cdsp = start_waiting(source_cdsp, source)
    cdsp.send(command)
    if command == "Exit":
        assert cdsp.process.wait(timeout=5) == 0
        return
    cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is None, timeout=5.0
    )
    source.play(48000)
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Inactive"


def test_a_new_entry_ends_the_wait(source_cdsp, source, config_file):
    """An entry written for the source's 4 channels runs as soon as it is loaded."""
    cdsp = start_waiting(source_cdsp, source)
    edits = source_edits(source)
    edits.update({"    channels: 2\n": "    channels: 4\n", "[0, 1]": "[0, 1, 2, 3]"})
    four = config_file(edits, base="dummy_control.yml")
    with open(four) as conf:
        cdsp.send("SetConfig", conf.read())
    cdsp.poll_until("GetState", "Running", timeout=5.0)
    assert config_value(cdsp, "/devices/capture/channels") == 4
    assert cdsp.send("GetControllerStatus")["waiting_for_source"] is None


def test_without_following_the_source_is_ignored(source_cdsp, source):
    source.play(44100)
    cdsp = source_cdsp(extra_args=["--wait"])
    wait_for_rate(cdsp, 48000)
    source.play(96000)
    time.sleep(1.0)
    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetStopReason") == "None"
