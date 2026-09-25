"""WASAPI endpoints listed, probed, opening, closing and being held.

The Dummy devices cannot fail to open and cannot be held by another process. A WASAPI
endpoint can do both, and exclusive mode is the one that holds it: while CamillaDSP has
the endpoint exclusively nobody else may open it, and that has to end with the session,
not with the process.
"""

import time

import pytest

from .cable import (
    EXIT_OK,
    SIXTEEN,
    capture_endpoint,
    opens_exclusive,
    render_endpoint,
    wait_for_peak,
    wait_for_stop,
    wasapi_block,
)

pytestmark = pytest.mark.wasapi


def wait_for_exclusive(output, expected, timeout=5.0):
    """Wait for PortAudio's exclusive open of an endpoint to succeed or fail as expected."""
    deadline = time.monotonic() + timeout
    while opens_exclusive(output) != expected:
        if time.monotonic() > deadline:
            state = "opens" if not expected else "is refused"
            raise AssertionError(f"exclusive open still {state} after {timeout} s")
        time.sleep(0.05)


def standby(start_cdsp):
    return start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)


def test_the_endpoints_are_listed(start_cdsp):
    """Both render endpoints and CABLE Output, by the names PortAudio sees too."""
    cdsp = standby(start_cdsp)
    capture = [name for name, _ in cdsp.send("GetAvailableCaptureDevices", backend="Wasapi")]
    playback = [name for name, _ in cdsp.send("GetAvailablePlaybackDevices", backend="Wasapi")]
    assert capture_endpoint()[1] in capture
    assert render_endpoint()[1] in playback
    assert any(SIXTEEN in name for name in playback)


def capability_sets(descriptor):
    """Per mode, the formats, rates and channel counts a capabilities reply names."""
    summary = {}
    for capability_set in descriptor["capability_sets"]:
        formats, rates, channels = set(), set(), set()
        for capability in capability_set["capabilities"]:
            channels.add(capability["channels"])
            for entry in capability["samplerates"]:
                rates.add(entry["samplerate"])
                formats.update(entry["formats"])
        summary[capability_set["mode"]] = (formats, rates, channels)
    return summary


@pytest.mark.parametrize("direction", ["Capture", "Playback"])
def test_the_capabilities_of_the_cable(start_cdsp, direction):
    """Shared is the mix format, float, stereo at 48 kHz. Exclusive is integer only, over
    a wide range of rates, at any channel count the cable has up to 16."""
    device = capture_endpoint()[1] if direction == "Capture" else render_endpoint()[1]
    cdsp = standby(start_cdsp)
    descriptor = cdsp.send(f"Get{direction}DeviceCapabilities", backend="Wasapi", device=device)
    summary = capability_sets(descriptor)
    assert summary["Shared"] == ({"F32"}, {48000}, {2})
    formats, rates, channels = summary["Exclusive"]
    assert formats == {"S16", "S24"}
    assert {11025, 44100, 48000, 96000, 192000} <= rates
    assert {1, 2} <= channels
    assert max(channels) <= 16


def test_the_capabilities_of_a_device_that_does_not_exist(start_cdsp):
    cdsp = standby(start_cdsp)
    reply = cdsp.send_raw("GetCaptureDeviceCapabilities", backend="Wasapi", device="No Such Device")
    assert reply["result"] != "Ok"


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_device_that_does_not_exist(start_cdsp, win_config, feeder, side):
    """A device name nothing answers to, reported against its own side."""
    feeder()
    missing = wasapi_block(side, "No Such Device")
    config = win_config(direction=side, **{side: missing})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.exit() == EXIT_OK


def test_exclusive_playback_holds_the_endpoint_and_lets_it_go(start_cdsp, win_config):
    """While playing exclusively nobody else may open the endpoint, and only while playing.

    An endpoint held past the session locks every other program out of it until
    CamillaDSP exits, so it has to be released on Stop as well as on exit, and taken
    again on a reload.
    """
    assert opens_exclusive(output=True)
    playback = wasapi_block("playback", render_endpoint()[1], exclusive=True, fmt="S16")
    config = win_config(direction="playback", playback=playback)
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    wait_for_exclusive(output=True, expected=False)
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    wait_for_exclusive(output=True, expected=True)
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_exclusive(output=True, expected=False)
    assert cdsp.exit() == EXIT_OK
    wait_for_exclusive(output=True, expected=True)


def test_exclusive_capture_holds_the_endpoint_and_lets_it_go(start_cdsp, win_config, feeder):
    """The same for CABLE Output, captured exclusively."""
    feeder()
    assert opens_exclusive(output=False)
    capture = wasapi_block("capture", capture_endpoint()[1], exclusive=True, fmt="S16")
    # Into a File on the null device, since nobody drains a stdout pipe here.
    null = ["  playback:", "    type: File", "    channels: 2", "    filename: NUL"]
    null.append("    format: F32_LE")
    cdsp = start_cdsp(
        config=win_config(direction="capture", capture=capture, playback=null),
        extra_args=["--wait"],
    )
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    wait_for_exclusive(output=False, expected=False)
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    wait_for_exclusive(output=False, expected=True)
    assert cdsp.exit() == EXIT_OK


def test_stop_and_reload(start_cdsp, win_config):
    """Stop releases the endpoint and Reload opens it again, in the same process."""
    cdsp = start_cdsp(config=win_config(direction="playback"), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert cdsp.exit() == EXIT_OK


def test_the_process_exits_cleanly_while_audio_is_flowing(start_cdsp, win_config):
    """Exit has to get both device threads out of their event waits and stop the streams."""
    cdsp = start_cdsp(config=win_config(direction="playback"))
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    started = time.monotonic()
    assert cdsp.exit() == EXIT_OK
    assert time.monotonic() - started < 5.0
