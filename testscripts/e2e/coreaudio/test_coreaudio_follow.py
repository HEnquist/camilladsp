"""The built-in controller following the rate of a BlackHole capture device.

BlackHole is the macOS stand-in for a loopback: a player sets the device's nominal rate,
and the capture has to follow. A rate change on the device fires the backend's rate
listener, which stops the session with a CaptureFormatChange, and the controller adapts
the config to the new rate and starts it again. While nothing runs, the controller asks
the device for its nominal rate instead, see `query_capture_source`.

The feeder stays at the rate it was opened at and the HAL converts, so the tone keeps
arriving whatever rate the device is switched to.
"""

import time

import pytest

from .blackhole import FEED_DEVICE, set_nominal_rate, wait_for_nominal_rate

pytestmark = pytest.mark.coreaudio

EXIT_OK = 0
FOLLOW = ["--wait", "--follow_capture"]
# The rates the tests switch to run natively, any other rate is resampled to 48 kHz.
NATIVE = {"alternate_samplerates": [44100, 96000]}

# A delay in samples is only right at the config's own rate, so Adapt refuses to run it
# natively at an alternate rate, and the controller waits for the source instead.
DELAY_IN_SAMPLES = """filters:
  delay:
    type: Delay
    parameters: {delay: 10, delay_unit: samples}
pipeline:
  - type: Filter
    channels: [0]
    names: [delay]
"""


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
    """Wait for the tone to come through on the capture side, at any level."""
    return cdsp.poll_until_true(
        "GetCaptureSignalPeak",
        lambda peaks: len(peaks) == 2 and all(peak > -20.0 for peak in peaks),
        timeout=timeout,
    )


def switch_source(rate):
    """Change the capture device's rate, the way a player opening it at `rate` does."""
    set_nominal_rate(FEED_DEVICE, rate)
    wait_for_nominal_rate(FEED_DEVICE, rate)


def wait_for_rate(cdsp, rate, timeout=10.0):
    """Wait for a session at `rate` to be up and running."""
    poll_config_value(cdsp, "/devices/samplerate", rate, timeout=timeout)
    cdsp.poll_until("GetState", "Running", timeout=timeout)


def status(cdsp):
    return cdsp.send("GetControllerStatus")


def with_delay_in_samples(config):
    """The config at `config`, with its empty pipeline replaced by a delay in samples."""
    with open(config) as conf:
        text = conf.read()
    assert "pipeline: []" in text
    with open(config, "w") as conf:
        conf.write(text.replace("pipeline: []", DELAY_IN_SAMPLES))
    return config


def test_adapt_follows_each_rate_change(start_cdsp, ca_config, feeder):
    """Adapt changes the startup config to every rate the device is switched to."""
    feeder()
    entry = ca_config(devices=NATIVE)
    cdsp = start_cdsp(config=entry, extra_args=FOLLOW)
    wait_for_signal(cdsp)
    assert status(cdsp)["following"] is True
    for rate in (44100, 96000, 48000):
        switch_source(rate)
        wait_for_rate(cdsp, rate)
        wait_for_signal(cdsp)
        # The chunk size scales with the rate.
        expected_chunksize = {44100: 1024, 48000: 1024, 96000: 2048}[rate]
        assert config_value(cdsp, "/devices/chunksize") == expected_chunksize
    assert (status(cdsp)["provider"], status(cdsp)["config_file"]) == ("Adapt", entry)
    assert cdsp.exit() == EXIT_OK


def test_adapt_with_a_resampler_changes_only_the_capture_rate(start_cdsp, ca_config, feeder):
    feeder()
    config = ca_config(
        devices={"resampler": "{type: AsyncSinc, profile: Fast}", "capture_samplerate": 48000}
    )
    cdsp = start_cdsp(config=config, extra_args=FOLLOW)
    wait_for_signal(cdsp)
    switch_source(96000)
    poll_config_value(cdsp, "/devices/capture_samplerate", 96000)
    cdsp.poll_until("GetState", "Running")
    assert config_value(cdsp, "/devices/samplerate") == 48000
    wait_for_signal(cdsp)


def test_a_rate_that_is_not_native_is_resampled(start_cdsp, ca_config, feeder):
    """Without alternate rates, a resampler is added and the playback stays at 48 kHz."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    switch_source(96000)
    poll_config_value(cdsp, "/devices/capture_samplerate", 96000)
    cdsp.poll_until("GetState", "Running")
    assert config_value(cdsp, "/devices/samplerate") == 48000
    assert config_value(cdsp, "/devices/resampler") == {"type": "Synchronous"}
    wait_for_signal(cdsp)


def test_without_following_a_rate_change_goes_idle(start_cdsp, ca_config, feeder):
    """Rule 3: wait mode with following off stops and stays stopped, as before."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait"])
    wait_for_signal(cdsp)
    switch_source(44100)
    reason = cdsp.poll_until_true("GetStopReason", lambda v: v != "None")
    assert reason["CaptureFormatChange"]["samplerate"] == 44100
    cdsp.poll_until("GetState", "Inactive")
    assert status(cdsp)["following"] is False
    assert config_value(cdsp, "/devices/samplerate") is None


def test_startup_follows_the_rate_the_source_is_at(start_cdsp, ca_config, feeder):
    """Rule 14: the source is asked first, so the first session opens at its rate."""
    switch_source(96000)
    feeder(rate=96000)
    cdsp = start_cdsp(config=ca_config(samplerate=44100, devices=NATIVE), extra_args=FOLLOW)
    wait_for_rate(cdsp, 96000)
    wait_for_signal(cdsp)


def test_a_rate_adapt_refuses_waits_for_the_source(start_cdsp, ca_config, feeder):
    """Rules 10 and 22: no config for the rate, so wait, and follow once there is one."""
    entry = with_delay_in_samples(ca_config(devices=NATIVE))
    feeder()
    cdsp = start_cdsp(config=entry, extra_args=FOLLOW)
    wait_for_signal(cdsp)
    adapt = cdsp.send("CheckControllerFiles")["adapt"]
    assert adapt["file"] == entry
    assert "filter 'delay': delay in samples" in adapt["problem"]
    switch_source(44100)
    waiting = cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is not None
    )["waiting_for_source"]
    assert waiting["samplerate"] == 44100
    cdsp.poll_until("GetState", "Inactive")
    # The query keeps reporting the same rate, so it keeps waiting.
    assert status(cdsp)["waiting_for_source"]["samplerate"] == 44100
    switch_source(48000)
    wait_for_rate(cdsp, 48000, timeout=5.0)
    assert status(cdsp)["waiting_for_source"] is None
    assert (status(cdsp)["provider"], status(cdsp)["config_file"]) == ("Adapt", entry)


def test_a_stop_ends_the_wait_for_the_source(start_cdsp, ca_config, feeder):
    feeder()
    cdsp = start_cdsp(config=with_delay_in_samples(ca_config(devices=NATIVE)), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    switch_source(44100)
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is not None)
    cdsp.send("Stop")
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is None)
    # Switching to a rate the config runs at no longer starts anything.
    switch_source(48000)
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Inactive"


def test_a_loaded_entry_starts_at_the_followed_rate(start_cdsp, ca_config, feeder):
    """A config written for 48 kHz, loaded while the source runs at 96 kHz, is adapted
    before it starts rather than starting at 48 kHz and following back."""
    feeder()
    cdsp = start_cdsp(config=ca_config(devices=NATIVE), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    switch_source(96000)
    wait_for_rate(cdsp, 96000)
    cdsp.send("SetConfigFilePath", ca_config(chunksize=512, devices=NATIVE))
    cdsp.send("Reload")
    poll_config_value(cdsp, "/devices/chunksize", 1024)
    assert config_value(cdsp, "/devices/samplerate") == 96000
    cdsp.poll_until("GetState", "Running")


def test_a_patch_keeps_the_rate_and_is_not_followed(start_cdsp, ca_config, feeder):
    """A patch applies to the running config, so the adapted rate stays. It switches to
    the websocket source, so the next rate change stops processing rather than adapting
    the patched config."""
    feeder()
    cdsp = start_cdsp(config=ca_config(devices=NATIVE), extra_args=FOLLOW)
    wait_for_signal(cdsp)
    switch_source(96000)
    wait_for_rate(cdsp, 96000)
    cdsp.send("SetConfigValue", pointer="/devices/chunksize", value=512)
    poll_config_value(cdsp, "/devices/chunksize", 512)
    assert config_value(cdsp, "/devices/samplerate") == 96000
    assert status(cdsp)["source"] == "Websocket"
    # The chunk size change restarts the devices. A switch while the capture is opening
    # can be undone by the open setting the device rate, so let it come up first.
    cdsp.poll_until("GetState", "Running")
    wait_for_signal(cdsp)
    switch_source(48000)
    reason = cdsp.poll_until_true("GetStopReason", lambda v: v != "None")
    assert reason["CaptureFormatChange"]["samplerate"] == 48000
    cdsp.poll_until("GetState", "Inactive")
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Inactive"
