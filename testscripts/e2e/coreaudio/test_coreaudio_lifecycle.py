"""CoreAudio devices opening, closing, changing rate and being held.

The Dummy devices cannot fail to open, cannot change rate under the engine and cannot
be held by another process. BlackHole can do all three, and they are what a user of a
real interface meets: the config names a device that is not there, the interface is
switched to another rate in Audio MIDI Setup, CamillaDSP holds the playback device in
hog mode. The rate change is also the only way to reach the backend's property
listeners, which is what #531 singles out.
"""

import time

import pytest

from .blackhole import (
    FEED_DEVICE,
    SINK_DEVICE,
    hog_owner,
    set_nominal_rate,
)

pytestmark = pytest.mark.coreaudio

EXIT_OK = 0
LEVEL_DB = -6.0


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.2 for peak in peaks),
        timeout=timeout,
    )


def wait_for_stop(cdsp):
    """Wait for the engine to have stopped, and return the reason. See test_failures.py."""
    reason = cdsp.poll_until_true("GetStopReason", lambda value: value != "None")
    cdsp.poll_until("GetState", "Inactive")
    return reason


def wait_for_hog_owner(pid, timeout=5.0):
    deadline = time.monotonic() + timeout
    while hog_owner(SINK_DEVICE) != pid:
        if time.monotonic() > deadline:
            raise AssertionError(f"{SINK_DEVICE} is hogged by {hog_owner(SINK_DEVICE)}, not {pid}")
        time.sleep(0.02)


@pytest.mark.parametrize(
    "device,reason",
    [(FEED_DEVICE, "CaptureFormatChange"), (SINK_DEVICE, "PlaybackFormatChange")],
)
def test_a_rate_change_on_a_device_stops_the_session(
    start_cdsp, ca_config, feeder, device, reason
):
    """Another program switching a device's rate has to stop the session and say so.

    CoreAudio stops delivering to a capture whose device changed rate, so carrying on is
    not an option, see backend_coreaudio.md. The backend hears about it through a
    property listener on the device, and the stop reason carries the new rate so a GUI
    can reload with it.
    """
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    set_nominal_rate(device, 44100)
    stopped = wait_for_stop(cdsp)
    assert stopped == {reason: 44100}
    assert cdsp.exit() == EXIT_OK


def test_a_new_config_reopens_the_devices(start_cdsp, ca_config, feeder):
    """Changing the chunk size rebuilds both devices, and they have to open again.

    Done a few times over, since an AudioUnit or a listener that is not disposed of
    builds up, and a listener left behind from an earlier session would react to the
    next one's rate change.
    """
    feeder()
    cdsp = start_cdsp(config=ca_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    for chunksize in (512, 2048, 1024):
        with open(ca_config(chunksize=chunksize)) as conf:
            cdsp.send("SetConfig", conf.read())
        cdsp.poll_until_true("GetConfig", lambda text: f"chunksize: {chunksize}" in text)
        cdsp.poll_until("GetState", "Running")
        wait_for_peak(cdsp, "GetPlaybackSignalPeak")
        assert cdsp.send("GetStopReason") == "None"


def test_stop_and_reload(start_cdsp, ca_config, feeder):
    """After Stop the devices are released, and a reload has to get them back."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_device_that_does_not_exist(start_cdsp, ca_config, feeder, side):
    """A device name nothing answers to, reported against its own side."""
    feeder()
    config = ca_config(**{f"{side}_device": "No Such Device"})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.exit() == EXIT_OK


def test_exclusive_mode_holds_the_playback_device_and_lets_it_go(start_cdsp, ca_config, feeder):
    """With `exclusive` the playback device is hogged by CamillaDSP, and only while running.

    A hog that outlives the session locks every other program out of the device until
    CamillaDSP exits, so it has to be released on Stop as well as on exit, and taken
    again on a reload.
    """
    feeder()
    config = ca_config(playback={"exclusive": True})
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    wait_for_hog_owner(cdsp.pid)
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    wait_for_hog_owner(-1)
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_hog_owner(cdsp.pid)
    assert cdsp.exit() == EXIT_OK
    wait_for_hog_owner(-1)


def test_without_exclusive_the_playback_device_is_not_held(start_cdsp, ca_config, feeder):
    feeder()
    cdsp = start_cdsp(config=ca_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    assert hog_owner(SINK_DEVICE) == -1


def test_the_devices_are_listed(start_cdsp):
    """Both BlackHole devices, in both directions, by the names Audio MIDI Setup shows."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    capture = [name for name, _ in cdsp.send("GetAvailableCaptureDevices", backend="CoreAudio")]
    playback = [name for name, _ in cdsp.send("GetAvailablePlaybackDevices", backend="CoreAudio")]
    for name in (FEED_DEVICE, SINK_DEVICE):
        assert name in capture
        assert name in playback


def capability_summary(descriptor):
    """The formats, rates and channel counts a capabilities reply names anywhere in it."""
    formats, rates, channels = set(), set(), set()
    for capability_set in descriptor["capability_sets"]:
        for capability in capability_set["capabilities"]:
            channels.add(capability["channels"])
            for entry in capability["samplerates"]:
                rates.add(entry["samplerate"])
                formats.update(entry["formats"])
    return formats, rates, channels


@pytest.mark.parametrize("device,channels", [(FEED_DEVICE, 2), (SINK_DEVICE, 16)])
@pytest.mark.parametrize("direction", ["Capture", "Playback"])
def test_the_capabilities_of_blackhole(start_cdsp, device, channels, direction):
    """Float only, the device's own channel count, and the common rates up to 192 kHz."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    descriptor = cdsp.send(f"Get{direction}DeviceCapabilities", backend="CoreAudio", device=device)
    formats, rates, found_channels = capability_summary(descriptor)
    assert formats == {"F32"}
    assert found_channels == {channels}
    assert {44100, 48000, 96000, 192000} <= rates


def test_the_capabilities_of_a_device_that_does_not_exist(start_cdsp):
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    reply = cdsp.send_raw(
        "GetCaptureDeviceCapabilities", backend="CoreAudio", device="No Such Device"
    )
    assert reply["result"] != "Ok"


def test_the_process_exits_cleanly_while_audio_is_flowing(start_cdsp, ca_config, feeder):
    """Exit has to get both device threads out of their waits and stop the AudioUnits."""
    feeder()
    cdsp = start_cdsp(config=ca_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    started = time.monotonic()
    assert cdsp.exit() == EXIT_OK
    assert time.monotonic() - started < 5.0
