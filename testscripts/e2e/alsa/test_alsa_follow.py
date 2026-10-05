"""The built-in controller following a player on the loopback.

While following, the capture sets `PCM Notify` on its cable. A player can then start at
any format, and when it differs from the capture's, the kernel stops the capture. The
backend reads the new format from the `PCM Slave` controls and stops the session with a
CaptureFormatChange, and the controller starts the config for it. While nothing runs,
the controller asks the device instead: the capture's hw params constraint gives the
format a running player holds the cable to, see `query_capture_source`.

This needs the snd-aloop notify fix (Linux 7.4, or a stable kernel with the backport).
Without it the player is held to the capture's format, so the module skips unless the
`notify_works` fixture sees a player escape that.
"""

import subprocess
import time

import pytest

from .loopback import (
    CAPTURE,
    CAPTURE_CABLE,
    FEED,
    Feeder,
    devices_present,
    encode,
    hw_params,
    notify,
    pcm_state,
    set_notify,
    sine,
    trigger_time,
    wait_for_pcm_state,
)

pytestmark = pytest.mark.alsa

EXIT_OK = 0
LEVEL_DB = -6.0
FOLLOW = ["--wait", "--follow_adapt"]


@pytest.fixture(scope="module")
def notify_works():
    """Skip the module on a kernel where a notify player is still held to the capture."""
    if not devices_present():
        pytest.skip("needs snd-aloop and snd-dummy loaded, see alsa/conftest.py")
    set_notify(CAPTURE_CABLE, True)
    capture = subprocess.Popen(
        ["arecord", "-q", "-D", CAPTURE, "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for_pcm_state(1, "c", CAPTURE_CABLE, "RUNNING")
        player = Feeder(encode("S16_LE", sine("S16_LE", 4410, rate=44100)), rate=44100)
        try:
            player.wait_until_running()
            rate = hw_params(0, "p", CAPTURE_CABLE).get("rate")
        finally:
            player.stop()
    finally:
        capture.kill()
        capture.wait(timeout=5)
        set_notify(CAPTURE_CABLE, False)
    if rate != "44100":
        pytest.skip("needs a kernel with the snd-aloop PCM Notify fix")


@pytest.fixture(autouse=True)
def notify_off(notify_works):
    """Start every test with `PCM Notify` off, the module default, and leave it so."""
    set_notify(CAPTURE_CABLE, False)
    yield
    set_notify(CAPTURE_CABLE, False)


def config_value(cdsp, pointer):
    """A value of the running config, or None while nothing runs."""
    reply = cdsp.send_raw("GetConfigValue", pointer)
    return reply.get("value") if reply.get("result") == "Ok" else None


def poll_config_value(cdsp, pointer, expected, timeout=10.0):
    deadline = time.monotonic() + timeout
    while (value := config_value(cdsp, pointer)) != expected:
        if time.monotonic() > deadline:
            raise TimeoutError(f"{pointer} is {value!r}, expected {expected!r}")
        time.sleep(0.05)


def wait_for_signal(cdsp, timeout=10.0):
    """Wait for the feeder's tone on both capture channels."""
    return cdsp.poll_until_true(
        "GetCaptureSignalPeak",
        lambda peaks: len(peaks) == 2 and all(abs(peak - LEVEL_DB) < 0.2 for peak in peaks),
        timeout=timeout,
    )


def wait_for_rate(cdsp, rate, timeout=10.0):
    """Wait for a session at `rate` to be running, with the capture open at that rate."""
    poll_config_value(cdsp, "/devices/samplerate", rate, timeout=timeout)
    cdsp.poll_until("GetState", "Running", timeout=timeout)
    assert hw_params(1, "c", CAPTURE_CABLE)["rate"] == str(rate)


def wait_for_capture(key, value, timeout=10.0):
    """Wait for the capture substream to run with a hw param at `value`."""
    deadline = time.monotonic() + timeout
    while hw_params(1, "c", CAPTURE_CABLE).get(key) != value:
        if time.monotonic() > deadline:
            raise TimeoutError(f"capture {key} is not {value}: {hw_params(1, 'c', 0)}")
        time.sleep(0.02)


def status(cdsp):
    return cdsp.send("GetControllerStatus")


def test_adapt_follows_back_to_back_rate_changes(start_cdsp, alsa_config, feeder):
    """Each new player stops the capture, and the session restarts at its rate.

    The players replace each other with no gap, the way a player reopens the device
    between tracks, and the measured rate agrees with the config every time.
    """
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    assert notify(CAPTURE_CABLE)
    current = None
    for rate in (44100, 96000, 48000):
        if current is not None:
            current.stop()
        current = feeder(rate=rate)
        wait_for_rate(cdsp, rate)
        wait_for_signal(cdsp)
        cdsp.poll_until_true(
            "GetCaptureRate", lambda measured: abs(measured - rate) < 0.005 * rate, timeout=10.0
        )
        assert hw_params(0, "p", CAPTURE_CABLE)["rate"] == str(rate)


def test_a_player_at_the_same_rate_does_not_restart(start_cdsp, alsa_config, feeder):
    """Rule 13: the kernel only stops the capture for a different format."""
    first = feeder(rate=44100)
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_rate(cdsp, 44100)
    wait_for_signal(cdsp)
    started = trigger_time(1, "c", CAPTURE_CABLE)
    first.stop()
    feeder(rate=44100)
    wait_for_signal(cdsp)
    assert trigger_time(1, "c", CAPTURE_CABLE) == started
    assert cdsp.send("GetStopReason") == "None"


def test_startup_follows_a_player_that_already_runs(start_cdsp, alsa_config, feeder):
    """Rule 14: the source is asked first, so the first session opens at its rate."""
    feeder(rate=96000)
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_rate(cdsp, 96000)
    wait_for_signal(cdsp)


def test_the_open_time_check_follows_a_player_that_has_not_started(start_cdsp, alsa_config):
    """A player that has set up its stream but not started holds the cable to its
    format, while the source reports inactive. So the entry config opens at its own
    rate, and the open has to see that the device only offers the player's."""
    player = subprocess.Popen(
        ["aplay", "-q", "-D", FEED, "-t", "raw", "-f", "S16_LE", "-r", "44100", "-c", "2"],
        stdin=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        # With nothing written, aplay prepares the stream and waits for data.
        wait_for_pcm_state(0, "p", CAPTURE_CABLE, "PREPARED")
        cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
        wait_for_rate(cdsp, 44100)
    finally:
        player.kill()
        player.wait(timeout=5)


def test_a_format_change_reopens_with_an_automatic_format(start_cdsp, alsa_config, feeder):
    """With no capture format in the config, the same config reopens at the new one."""
    first = feeder(fmt="S16_LE")
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    wait_for_capture("format", "S16_LE")
    first.stop()
    feeder(fmt="S32_LE")
    wait_for_capture("format", "S32_LE")
    cdsp.poll_until("GetState", "Running")
    wait_for_signal(cdsp)
    assert config_value(cdsp, "/devices/capture/format") is None


def test_adapt_rewrites_an_explicit_format(start_cdsp, alsa_config, feeder):
    """Rule 11: an explicit capture format is changed to the one the player uses."""
    first = feeder(fmt="S16_LE")
    cdsp = start_cdsp(config=alsa_config(capture_format="S16_LE"), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    first.stop()
    feeder(fmt="S24_3_LE")
    poll_config_value(cdsp, "/devices/capture/format", "S24_3_LE")
    cdsp.poll_until("GetState", "Running")
    wait_for_signal(cdsp)


def test_a_channel_change_without_a_variant_waits_for_the_source(
    start_cdsp, alsa_config, feeder
):
    """Adapt can't change the channel count, so it waits, and follows the next player."""
    first = feeder()
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    first.stop()
    four = feeder(channels=4)
    waiting = cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is not None
    )["waiting_for_source"]
    assert waiting == {"samplerate": 48000, "channels": 4, "format": "S16_LE"}
    cdsp.poll_until("GetState", "Inactive")
    # Polling the source doesn't hold the cable, the player keeps its 4 channels.
    time.sleep(1.5)
    assert hw_params(0, "p", CAPTURE_CABLE)["channels"] == "4"
    four.stop()
    feeder(channels=2)
    cdsp.poll_until("GetState", "Running", timeout=5.0)
    wait_for_signal(cdsp)
    assert status(cdsp)["waiting_for_source"] is None


def test_the_entry_config_starts_when_the_source_stops(start_cdsp, alsa_config, feeder):
    """Rule 22: an inactive source while waiting starts the entry config, and the
    capture stays open for the next player."""
    first = feeder()
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    first.stop()
    four = feeder(channels=4)
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is not None)
    four.stop()
    cdsp.poll_until("GetState", "Running", timeout=5.0)
    wait_for_pcm_state(1, "c", CAPTURE_CABLE, "RUNNING")
    # And the next player is followed as usual.
    feeder(rate=44100)
    wait_for_rate(cdsp, 44100)
    wait_for_signal(cdsp)


def test_the_capture_stays_open_when_the_player_stops(start_cdsp, alsa_config, feeder):
    first = feeder()
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    started = trigger_time(1, "c", CAPTURE_CABLE)
    first.stop()
    time.sleep(1.0)
    assert cdsp.send("GetState") in ("Running", "Paused")
    assert pcm_state(1, "c", CAPTURE_CABLE) == "RUNNING"
    assert trigger_time(1, "c", CAPTURE_CABLE) == started


def test_notify_is_cleared_again_on_exit(start_cdsp, alsa_config, feeder):
    feeder()
    cdsp = start_cdsp(config=alsa_config(capture_format=None), extra_args=FOLLOW)
    assert notify(CAPTURE_CABLE)
    assert cdsp.exit() == EXIT_OK
    assert not notify(CAPTURE_CABLE)


def test_without_following_notify_is_left_alone(start_cdsp, alsa_config, feeder):
    """Following off: the player is held to the capture's rate, as before."""
    cdsp = start_cdsp(config=alsa_config(), extra_args=["--wait"])
    assert not notify(CAPTURE_CABLE)
    player = Feeder(encode("S16_LE", sine("S16_LE", 4410, rate=44100)), rate=44100)
    try:
        player.wait_until_running()
        assert hw_params(0, "p", CAPTURE_CABLE)["rate"] == "48000"
        time.sleep(0.5)
        assert cdsp.send("GetState") == "Running"
        assert cdsp.send("GetStopReason") == "None"
    finally:
        player.stop()


@pytest.fixture
def variants(tmp_path, alsa_config):
    """Factory for Specific variant files named by rate and channels.

    Returns the template. The entry config is one of the variants, see `variant`.
    """

    def _write(*formats):
        for rate, channels in formats:
            config = alsa_config(capture_format=None, samplerate=rate, channels=channels)
            with open(config) as conf:
                (tmp_path / f"conf_{rate}_{channels}.yml").write_text(conf.read())
        return str(tmp_path / "conf_$samplerate$_$channels$.yml")

    return _write


def variant(template, rate, channels):
    return template.replace("$samplerate$", str(rate)).replace("$channels$", str(channels))


def test_specific_switches_files_on_a_channel_change(start_cdsp, feeder, variants):
    """A player going from 2 to 4 channels at the same rate selects the 4 channel file,
    while the entry stays the config file path. A Reload starts from the entry and
    selects the 4 channel file again."""
    template = variants((48000, 2), (48000, 4))
    entry = variant(template, 48000, 2)
    first = feeder()
    cdsp = start_cdsp(config=entry, extra_args=["--wait", "--follow_specific", template])
    wait_for_signal(cdsp)
    assert status(cdsp)["active_config_file"] == entry
    first.stop()
    feeder(channels=4)
    cdsp.poll_until_true(
        "GetControllerStatus",
        lambda s: s["active_config_file"] == variant(template, 48000, 4),
    )
    cdsp.poll_until("GetState", "Running")
    wait_for_capture("channels", "4")
    assert cdsp.send("GetConfigFilePath") == entry
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    assert status(cdsp)["active_config_file"] == variant(template, 48000, 4)
    wait_for_capture("channels", "4")


def test_specific_and_adapt_chain_through_rates(start_cdsp, alsa_config, feeder, variants):
    """A rate with its own file uses it, any other rate adapts the entry."""
    template = variants((96000, 2))
    entry = alsa_config(capture_format=None)
    current = feeder()
    cdsp = start_cdsp(
        config=entry, extra_args=["--wait", "--follow_specific", template, "--follow_adapt"]
    )
    wait_for_signal(cdsp)
    for rate in (96000, 44100, 96000):
        current.stop()
        current = feeder(rate=rate)
        wait_for_rate(cdsp, rate)
        wait_for_signal(cdsp)
        expected = variant(template, 96000, 2) if rate == 96000 else None
        assert status(cdsp)["active_config_file"] == expected
