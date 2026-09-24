"""The ASIO backend listing, probing, loading and unloading the Steinberg driver.

An ASIO driver is a COM object loaded into the process, one at a time, so a stop has to
unload it and a reload has to load it again, and a probe cannot run while a stream has
it loaded.
"""

import time

import pytest

from .cable import (
    EXIT_OK,
    STEINBERG,
    asio_block,
    wait_for_peak,
    wait_for_stop,
)

pytestmark = [pytest.mark.wasapi, pytest.mark.usefixtures("steinberg")]


def standby(start_cdsp):
    return start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)


def playback_config(win_config):
    return win_config(direction="playback", playback=asio_block("playback"))


def test_the_driver_is_listed(start_cdsp):
    cdsp = standby(start_cdsp)
    for direction in ("Capture", "Playback"):
        names = [name for name, _ in cdsp.send(f"GetAvailable{direction}Devices", backend="Asio")]
        assert STEINBERG in names


@pytest.mark.parametrize("direction", ["Capture", "Playback"])
def test_the_capabilities_of_the_driver(start_cdsp, direction):
    """Float only, stereo, and the common rates, in the one unified set ASIO has."""
    cdsp = standby(start_cdsp)
    descriptor = cdsp.send(f"Get{direction}DeviceCapabilities", backend="Asio", device=STEINBERG)
    [capability_set] = descriptor["capability_sets"]
    assert capability_set["mode"] == "Unified"
    formats, rates, channels = set(), set(), set()
    for capability in capability_set["capabilities"]:
        channels.add(capability["channels"])
        for entry in capability["samplerates"]:
            rates.add(entry["samplerate"])
            formats.update(entry["formats"])
    assert formats == {"F32_LE"}
    assert channels == {2}
    assert {44100, 48000, 96000, 192000} <= rates


def test_the_capabilities_of_a_driver_that_does_not_exist(start_cdsp):
    cdsp = standby(start_cdsp)
    reply = cdsp.send_raw("GetCaptureDeviceCapabilities", backend="Asio", device="No Such Driver")
    assert reply["result"] != "Ok"


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_driver_that_does_not_exist(start_cdsp, win_config, side):
    config = win_config(direction=side, **{side: asio_block(side, device="No Such Driver")})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.exit() == EXIT_OK


def test_stop_and_reload(start_cdsp, win_config):
    """Stop unloads the driver and Reload loads it again, in the same process, twice."""
    cdsp = start_cdsp(config=playback_config(win_config), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    for _ in range(2):
        cdsp.send("Stop")
        cdsp.poll_until("GetState", "Inactive")
        cdsp.send("Reload")
        cdsp.poll_until("GetState", "Running")
        wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert cdsp.exit() == EXIT_OK


def test_the_driver_can_be_probed_after_a_stop(start_cdsp, win_config):
    """A probe is refused while a stream has the driver loaded, and works once it stops."""
    cdsp = start_cdsp(config=playback_config(win_config), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    reply = cdsp.send_raw("GetPlaybackDeviceCapabilities", backend="Asio", device=STEINBERG)
    assert reply["result"] != "Ok"
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    reply = cdsp.send_raw("GetPlaybackDeviceCapabilities", backend="Asio", device=STEINBERG)
    assert reply["result"] == "Ok"
    assert cdsp.exit() == EXIT_OK


def test_the_process_exits_cleanly_while_audio_is_flowing(start_cdsp, win_config):
    cdsp = start_cdsp(config=playback_config(win_config))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    started = time.monotonic()
    assert cdsp.exit() == EXIT_OK
    assert time.monotonic() - started < 5.0
