"""ALSA devices opening, closing, going away and coming back.

The Dummy devices cannot fail to open, cannot be busy, and have no far end that can
stop. A loopback has all three, and they are what a user of a real loopback meets: the
player quits, another program holds the device, the config names a device that is not
there. These are also where the backend's control events come in, since the loopback
tells the capture about its far end through `PCM Slave Active`.
"""

import subprocess
import time

import pytest

from .loopback import (
    CAPTURE,
    CAPTURE_CABLE,
    DUMMY,
    set_rate_shift,
    shifted_rate,
    wait_for_pcm_state,
)

pytestmark = pytest.mark.alsa

EXIT_OK = 0
RATE = 48000
LEVEL_DB = -6.0


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.2 for peak in peaks),
        timeout=timeout,
    )


def wait_for_silence(cdsp, timeout=10.0):
    return cdsp.poll_until_true(
        "GetCaptureSignalPeak",
        lambda peaks: len(peaks) == 2 and all(peak < -100.0 for peak in peaks),
        timeout=timeout,
    )


def wait_for_stop(cdsp):
    """Wait for the engine to have stopped, and return the reason. See test_failures.py."""
    reason = cdsp.poll_until_true("GetStopReason", lambda value: value != "None")
    cdsp.poll_until("GetState", "Inactive")
    return reason


@pytest.fixture
def holder():
    """Factory for an arecord that holds a capture device open, killed when the test ends."""
    started = []

    def _hold(device):
        process = subprocess.Popen(
            ["arecord", "-q", "-D", device, "-t", "raw", "-f", "S16_LE", "-r", str(RATE),
             "-c", "2", "/dev/null"],
            stderr=subprocess.PIPE,
        )
        started.append(process)
        _, dev, sub = device.split(",")
        wait_for_pcm_state(int(dev), "c", int(sub), "RUNNING")
        return process

    yield _hold

    for process in started:
        process.kill()
        process.wait(timeout=5)


def test_stop_on_inactive_ends_the_session_when_the_source_stops(
    start_cdsp, alsa_config, feeder
):
    """The player quitting is the end of the stream, when the config asks for that.

    The loopback reports it through the `PCM Slave Active` control, which the capture
    subscribes to, so this is the only test that exercises the backend's control event
    handling. It ends like a file reaching its end, with `Done` rather than an error.
    """
    feed = feeder()
    config = alsa_config(capture={"stop_on_inactive": True})
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    feed.stop()
    assert wait_for_stop(cdsp) == "Done"
    assert cdsp.exit() == EXIT_OK


def test_without_stop_on_inactive_a_stopped_source_is_silence(start_cdsp, alsa_config, feeder):
    """By default the capture outlives the player, and a new player is picked up again.

    With nothing playing into it the loopback capture keeps running on its own timer
    and delivers silence, so the engine stays up. A player that starts again has to
    match the format the capture holds the cable at, and then its audio comes through
    without anything being restarted.
    """
    feed = feeder()
    cdsp = start_cdsp(config=alsa_config())
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    feed.stop()
    wait_for_silence(cdsp)
    assert cdsp.send("GetState") == "Running"
    feeder()
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    assert cdsp.send("GetStopReason") == "None"


def test_a_new_config_reopens_the_devices(start_cdsp, alsa_config, feeder):
    """Changing the chunk size rebuilds both devices, and they have to open again.

    Each ALSA substream can only be open once, so a device that was not closed before
    its replacement opened shows up here as a busy device and a capture error, which the
    Dummy devices can never produce. Done a few times over, since a leak builds up.
    """
    feeder()
    config = alsa_config()
    cdsp = start_cdsp(config=config)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    for chunksize in (512, 2048, 1024):
        with open(alsa_config(chunksize=chunksize)) as conf:
            cdsp.send("SetConfig", conf.read())
        cdsp.poll_until_true("GetConfig", lambda text: f"chunksize: {chunksize}" in text)
        cdsp.poll_until("GetState", "Running")
        wait_for_peak(cdsp, "GetPlaybackSignalPeak")
        assert cdsp.send("GetStopReason") == "None"


def test_stop_releases_the_devices(start_cdsp, alsa_config, feeder):
    """After Stop, another program has to be able to open the devices.

    Checked from procfs, so what it proves is that the substreams are really closed,
    not just that the engine says it is inactive. Then a reload has to get them back.
    """
    feeder()
    cdsp = start_cdsp(config=alsa_config(), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    wait_for_pcm_state(1, "c", 0, "closed")
    wait_for_pcm_state(0, "p", 1, "closed")
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")


@pytest.mark.parametrize(
    "side,device",
    [
        ("capture", "hw:Loopback,1,7"),
        ("capture", "hw:NoSuchCard"),
        ("playback", "hw:NoSuchCard"),
    ],
)
def test_a_device_that_does_not_exist(start_cdsp, alsa_config, feeder, side, device):
    """A missing subdevice and a missing card, each reported against its own side."""
    feeder()
    config = alsa_config(**{f"{side}_device": device})
    cdsp = start_cdsp(config=config, extra_args=["--wait"], wait_for_running=False)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureError" if side == "capture" else "PlaybackError"]
    assert cdsp.exit() == EXIT_OK


def test_a_busy_capture_device(start_cdsp, alsa_config, feeder, holder):
    """Another program holding the capture is a clean capture error, not a hang.

    Also what `GetCaptureDeviceCapabilities` has its own busy error for, which is what
    lets a GUI say why a device it lists cannot be used.
    """
    feeder()
    holder(CAPTURE)
    cdsp = start_cdsp(config=alsa_config(), extra_args=["--wait"], wait_for_running=False)
    assert list(wait_for_stop(cdsp)) == ["CaptureError"]
    reply = cdsp.send_raw("GetCaptureDeviceCapabilities", backend="Alsa", device=CAPTURE)
    assert reply["result"] == "DeviceBusyError"


def test_the_cards_are_listed(start_cdsp):
    """Both ends of both cables, and the snd-dummy playback, by their hw names."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    capture = [name for name, _ in cdsp.send("GetAvailableCaptureDevices", backend="Alsa")]
    playback = [name for name, _ in cdsp.send("GetAvailablePlaybackDevices", backend="Alsa")]
    for name in ("hw:Loopback,1,0", "hw:Loopback,1,1", "hw:Loopback,0,0"):
        assert name in capture
    for name in ("hw:Loopback,0,0", "hw:Loopback,0,1", "hw:Dummy,0,0"):
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


def test_the_capabilities_of_snd_dummy(start_cdsp):
    """snd-dummy is small enough to know exactly: S16_LE, one or two channels, 48 kHz top."""
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    descriptor = cdsp.send("GetPlaybackDeviceCapabilities", backend="Alsa", device=DUMMY)
    formats, rates, channels = capability_summary(descriptor)
    assert formats == {"S16_LE"}
    assert 48000 in rates
    assert max(rates) == 48000
    assert channels <= {1, 2}


def test_the_capabilities_of_a_locked_loopback(start_cdsp, feeder):
    """With a player on the far end, the loopback only offers what the player runs.

    And without one it offers every format it has but the float64 it lacks, which is
    the difference between the two probes the backend makes.
    """
    cdsp = start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)
    formats, _, _ = capability_summary(
        cdsp.send("GetCaptureDeviceCapabilities", backend="Alsa", device=CAPTURE)
    )
    assert {"S16_LE", "S24_3_LE", "S32_LE", "F32_LE"} <= formats
    assert "F64_LE" not in formats

    feeder(fmt="S32_LE", rate=44100)
    formats, rates, channels = capability_summary(
        cdsp.send("GetCaptureDeviceCapabilities", backend="Alsa", device=CAPTURE)
    )
    assert formats == {"S32_LE"}
    assert rates == {44100}
    assert channels == {2}


def test_a_capture_clock_that_jumps_stops_the_session(start_cdsp, alsa_config, feeder):
    """A source whose rate moves by far more than a clock drifts is a format change.

    The loopback's rate shift is what moves it: 10 % slow is well past the 4 % the
    watcher allows, so with `stop_on_rate_change` set the engine has to stop and report
    the rate it measured. On real hardware this is an S/PDIF source changing rate.

    The reported rate is one short measurement window, 0.2 s here, and that holds only
    eight or nine chunks, so it is quantised by about a chunk and lands several percent
    either side of the true rate. What it has to be is clearly on the far side of the
    threshold, not close to the shifted rate.
    """
    feeder()
    config = alsa_config(
        devices={"stop_on_rate_change": True, "rate_measure_interval_s": 0.2}
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait"])
    wait_for_peak(cdsp, "GetCaptureSignalPeak")
    shift = 110000
    set_rate_shift(CAPTURE_CABLE, shift)
    reason = wait_for_stop(cdsp)
    assert list(reason) == ["CaptureFormatChange"]
    reported = reason["CaptureFormatChange"]
    assert 0.85 * shifted_rate(RATE, shift) < reported < RATE / 1.04


def test_the_process_exits_cleanly_while_audio_is_flowing(start_cdsp, alsa_config, feeder):
    """Exit has to get both device threads out of their waits and close the devices."""
    feeder()
    cdsp = start_cdsp(config=alsa_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    started = time.monotonic()
    assert cdsp.exit() == EXIT_OK
    assert time.monotonic() - started < 5.0
    wait_for_pcm_state(1, "c", 0, "closed")
    wait_for_pcm_state(0, "p", 1, "closed")

