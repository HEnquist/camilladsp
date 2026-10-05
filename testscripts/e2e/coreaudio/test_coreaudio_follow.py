"""The built-in controller following the rate of a BlackHole capture device.

BlackHole is the macOS stand-in for a loopback: a player sets the device's nominal rate,
and the capture has to follow. A rate change on the device fires the backend's rate
listener, which stops the session with a CaptureFormatChange, and the controller picks
the config for the new rate and starts it again. While nothing runs, the controller asks
the device for its nominal rate instead, see `query_capture_source`.

The feeder stays at the rate it was opened at and the HAL converts, so the tone keeps
arriving whatever rate the device is switched to.
"""

import time

import pytest

from .blackhole import FEED_DEVICE, set_nominal_rate, wait_for_nominal_rate

pytestmark = pytest.mark.coreaudio

EXIT_OK = 0


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


@pytest.fixture
def variants(tmp_path, ca_config):
    """Factory for Specific variant files named by rate, returning the template.

    With Specific, the entry config is one of the variants, the one for the rate it
    starts at. Get its path with `variant(template, rate)`.
    """

    def _write(*rates, **kwargs):
        for rate in rates:
            with open(ca_config(samplerate=rate, **kwargs)) as conf:
                (tmp_path / f"variant_{rate}.yml").write_text(conf.read())
        return str(tmp_path / "variant_$samplerate$.yml")

    return _write


def variant(template, rate):
    return template.replace("$samplerate$", str(rate))


def test_adapt_follows_each_rate_change(start_cdsp, ca_config, feeder):
    """Adapt changes the entry config to every rate the device is switched to."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait", "--follow_adapt"])
    wait_for_signal(cdsp)
    assert status(cdsp)["following"] is True
    for rate in (44100, 96000, 48000):
        switch_source(rate)
        wait_for_rate(cdsp, rate)
        wait_for_signal(cdsp)
        # The chunk size scales with the rate.
        expected_chunksize = {44100: 1024, 48000: 1024, 96000: 2048}[rate]
        assert config_value(cdsp, "/devices/chunksize") == expected_chunksize
    assert status(cdsp)["active_config_file"] is None
    assert cdsp.exit() == EXIT_OK


def test_adapt_with_a_resampler_changes_only_the_capture_rate(start_cdsp, ca_config, feeder):
    feeder()
    config = ca_config(
        devices={"resampler": "{type: AsyncSinc, profile: Fast}", "capture_samplerate": 48000}
    )
    cdsp = start_cdsp(config=config, extra_args=["--wait", "--follow_adapt"])
    wait_for_signal(cdsp)
    switch_source(96000)
    poll_config_value(cdsp, "/devices/capture_samplerate", 96000)
    cdsp.poll_until("GetState", "Running")
    assert config_value(cdsp, "/devices/samplerate") == 48000
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
    cdsp = start_cdsp(
        config=ca_config(samplerate=44100), extra_args=["--wait", "--follow_adapt"]
    )
    wait_for_rate(cdsp, 96000)
    wait_for_signal(cdsp)


def test_specific_switches_between_files(start_cdsp, feeder, variants):
    template = variants(44100, 48000, 96000)
    entry = variant(template, 48000)
    feeder()
    cdsp = start_cdsp(config=entry, extra_args=["--wait", "--follow_specific", template])
    wait_for_signal(cdsp)
    # The entry is selected for its own rate like any other variant.
    assert status(cdsp)["active_config_file"].endswith("variant_48000.yml")
    assert cdsp.send("CheckControllerFiles") == [
        {"file": variant(template, rate), "samplerate": rate, "channels": None,
         "format": None, "problem": None}
        for rate in (44100, 48000, 96000)
    ]
    for rate in (44100, 96000, 48000):
        switch_source(rate)
        wait_for_rate(cdsp, rate)
        wait_for_signal(cdsp)
        assert status(cdsp)["active_config_file"].endswith(f"variant_{rate}.yml")
        # The entry stays the config file, so a restart starts from it again.
        assert cdsp.send("GetConfigFilePath") == entry


def test_specific_with_a_missing_file_waits_for_the_source(
    start_cdsp, ca_config, feeder, variants
):
    """Rules 10 and 22: no config for the rate, so wait, and follow once there is one."""
    template = variants(48000, 96000)
    feeder()
    cdsp = start_cdsp(
        config=variant(template, 48000), extra_args=["--wait", "--follow_specific", template]
    )
    wait_for_signal(cdsp)
    switch_source(44100)
    waiting = cdsp.poll_until_true(
        "GetControllerStatus", lambda s: s["waiting_for_source"] is not None
    )["waiting_for_source"]
    assert waiting["samplerate"] == 44100
    cdsp.poll_until("GetState", "Inactive")
    # The query keeps reporting the same rate, so it keeps waiting.
    assert status(cdsp)["waiting_for_source"]["samplerate"] == 44100
    switch_source(96000)
    wait_for_rate(cdsp, 96000, timeout=5.0)
    assert status(cdsp)["waiting_for_source"] is None
    assert status(cdsp)["active_config_file"].endswith("variant_96000.yml")


def test_a_stop_ends_the_wait_for_the_source(start_cdsp, feeder, variants):
    template = variants(48000, 96000)
    feeder()
    cdsp = start_cdsp(
        config=variant(template, 48000), extra_args=["--wait", "--follow_specific", template]
    )
    wait_for_signal(cdsp)
    switch_source(44100)
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is not None)
    cdsp.send("Stop")
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is None)
    # Switching to a rate with a file no longer starts anything.
    switch_source(96000)
    time.sleep(2.0)
    assert cdsp.send("GetState") == "Inactive"


def test_specific_falls_back_to_adapt(start_cdsp, ca_config, feeder, variants):
    """With both providers, a rate with no file adapts the entry, which then doesn't
    have to follow the naming scheme. The preflight still points it out."""
    template = variants(96000)
    entry = ca_config()
    feeder()
    cdsp = start_cdsp(
        config=entry,
        extra_args=["--wait", "--follow_specific", template, "--follow_adapt"],
    )
    wait_for_signal(cdsp)
    misnamed = [c for c in cdsp.send("CheckControllerFiles") if c["file"] == entry]
    assert "template" in misnamed[0]["problem"]
    switch_source(44100)
    wait_for_rate(cdsp, 44100)
    assert status(cdsp)["active_config_file"] is None
    switch_source(96000)
    wait_for_rate(cdsp, 96000)
    assert status(cdsp)["active_config_file"].endswith("variant_96000.yml")


def test_a_variant_at_the_wrong_rate_is_rejected(start_cdsp, ca_config, feeder, variants):
    """Rule 18: a file whose rate doesn't match its name would select itself forever."""
    template = variants(48000)
    with open(ca_config(samplerate=48000)) as conf:
        open(variant(template, 44100), "w").write(conf.read())
    feeder()
    cdsp = start_cdsp(
        config=variant(template, 48000), extra_args=["--wait", "--follow_specific", template]
    )
    wait_for_signal(cdsp)
    problems = {c["samplerate"]: c["problem"] for c in cdsp.send("CheckControllerFiles")}
    assert problems[48000] is None
    assert "rate" in problems[44100]
    switch_source(44100)
    cdsp.poll_until_true("GetControllerStatus", lambda s: s["waiting_for_source"] is not None)


def test_a_loaded_entry_starts_at_the_followed_rate(start_cdsp, ca_config, feeder):
    """A config written for 48 kHz, loaded while the source runs at 96 kHz, is adapted
    before it starts rather than starting at 48 kHz and following back."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait", "--follow_adapt"])
    wait_for_signal(cdsp)
    switch_source(96000)
    wait_for_rate(cdsp, 96000)
    with open(ca_config(chunksize=512)) as conf:
        cdsp.send("SetConfig", conf.read())
    poll_config_value(cdsp, "/devices/chunksize", 1024)
    assert config_value(cdsp, "/devices/samplerate") == 96000
    cdsp.poll_until("GetState", "Running")


def test_a_patch_adapts_from_the_raw_config(start_cdsp, ca_config, feeder):
    """PatchConfig works on the entry behind the adapted config, so following keeps
    working and the patch survives the next rate change."""
    feeder()
    cdsp = start_cdsp(config=ca_config(), extra_args=["--wait", "--follow_adapt"])
    wait_for_signal(cdsp)
    switch_source(96000)
    wait_for_rate(cdsp, 96000)
    cdsp.send("SetConfigValue", pointer="/devices/chunksize", value=512)
    # Adapted from the patched entry: 512 at 48 kHz is 1024 at 96 kHz.
    poll_config_value(cdsp, "/devices/chunksize", 1024)
    assert config_value(cdsp, "/devices/samplerate") == 96000
    # The chunk size change restarts the devices. A switch while the capture is opening
    # can be undone by the open setting the device rate, so let it come up first.
    cdsp.poll_until("GetState", "Running")
    wait_for_signal(cdsp)
    switch_source(48000)
    wait_for_rate(cdsp, 48000)
    assert config_value(cdsp, "/devices/chunksize") == 512
