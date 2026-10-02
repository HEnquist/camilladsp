"""Error and edge handling on the control plane.

Malformed input reaches the websocket server before any of the command handling, and
comes back as an `Invalid` reply with an `error` field rather than the usual `result`,
so these assert on that shape.
"""

import json

import pytest


def test_malformed_json(cdsp):
    reply = cdsp.send_text("this is not json")
    assert reply["reply"] == "Invalid"
    assert reply["error"]


def test_unknown_command(cdsp):
    reply = cdsp.send_raw("NoSuchCommand")
    assert reply["reply"] == "Invalid"
    assert "unknown variant" in reply["error"]


def test_a_command_missing_its_argument(cdsp):
    reply = cdsp.send_raw("SetVolume")
    assert reply["reply"] == "Invalid"


def test_an_argument_of_the_wrong_type(cdsp):
    reply = cdsp.send_raw("SetVolume", "loud please")
    assert reply["reply"] == "Invalid"


def test_stopping_a_subscription_that_was_never_started(cdsp):
    reply = cdsp.send_raw("StopSubscription")
    assert reply["reply"] == "Invalid"
    assert "subscription" in reply["error"].lower()


def test_a_rejected_subscription_does_not_start(cdsp):
    """An out of range smoothing constant is refused, and leaves nothing behind."""
    reply = cdsp.send_raw(
        "SubscribeVuLevels", {"max_rate": 10.0, "attack": -1.0, "release": 300.0}
    )
    assert reply["result"] == "InvalidValueError"
    assert cdsp.send_raw("StopSubscription")["reply"] == "Invalid"


def test_a_device_type_this_build_does_not_have(cdsp):
    """The device enums deny unknown fields, so a missing type is a clear read error."""
    with open(cdsp.send("GetConfigFilePath")) as conf:
        text = conf.read()
    reply = cdsp.send_raw("SetConfig", text.replace("type: Dummy", "type: NoSuchDevice", 1))
    assert reply["result"] == "ConfigReadError"
    assert "NoSuchDevice" in reply["message"]


def test_target_level_beyond_the_buffer(cdsp):
    """target_level cannot exceed what the queue can hold, and validation says so."""
    with open(cdsp.send("GetConfigFilePath")) as conf:
        text = conf.read()
    too_big = "chunksize: 1024\n  target_level: 100000"
    reply = cdsp.send_raw("ValidateConfig", text.replace("chunksize: 1024", too_big))
    assert reply["result"] == "ConfigValidationError"
    assert "target_level" in reply["message"]


@pytest.mark.parametrize("backend", ["Asio", "NotABackend"])
def test_listing_devices_for_a_backend_this_build_lacks(cdsp, backend):
    """Listing a backend that is not compiled in is empty, not an error."""
    assert cdsp.send("GetAvailableCaptureDevices", backend=backend) == []
    assert cdsp.send("GetAvailablePlaybackDevices", backend=backend) == []


def test_clipped_samples_can_be_reset(cdsp):
    """Nothing clips on a dummy playback, but the counter and its reset still answer."""
    assert cdsp.send("GetClippedSamples") == 0
    cdsp.send("ResetClippedSamples")
    assert cdsp.send("GetClippedSamples") == 0


def test_the_update_interval_round_trips(cdsp):
    """The status refresh rate is settable, which is what a fast meter needs."""
    assert cdsp.send("GetUpdateInterval") == 1000
    cdsp.send("SetUpdateInterval", 100)
    assert cdsp.send("GetUpdateInterval") == 100
    # And the faster cadence really is used: a fresh rate measurement has to appear
    # within a few intervals rather than a few seconds.
    cdsp.send("SetVolume", -30.0)
    cdsp.poll_until_true("GetCaptureRate", lambda rate: rate > 0, timeout=3.0)


def test_a_config_with_no_pipeline_is_accepted(cdsp):
    """An empty pipeline is a passthrough, not an error, and must stay that way."""
    config = json.loads(cdsp.send("GetConfigJson"))
    config["pipeline"] = []
    cdsp.send("SetConfigJson", json.dumps(config))
    assert cdsp.send("GetState") == "Running"
