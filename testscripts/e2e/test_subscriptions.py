"""Pushed event subscriptions.

A subscribed connection accepts nothing but `StopSubscription`,
`src/websocket_server/mod.rs:625`, and holds one subscription at a time. So the tests
that want to both watch a stream and drive the engine subscribe on a second connection
from `cdsp.new_client()` and keep sending commands on the first.

The base config captures a 1 kHz sine at -6 dBFS through a -6 dB gain filter, the same
as in test_signal_levels.py, so the pushed levels are assertable against the same
computed numbers rather than against "something nonzero".
"""

import pytest

CAPTURE_PEAK_DB = -6.0
CAPTURE_RMS_DB = -9.01
PLAYBACK_PEAK_DB = -12.0
PLAYBACK_RMS_DB = -15.01
TOLERANCE = 0.2

# Chunksize 1024 at 48000 is a chunk every 21.3 ms, and the levels are published once
# per chunk.
CHUNK_S = 1024 / 48000
VU = {"max_rate": 50.0, "attack": 0.0, "release": 0.0}


def assert_close(values, expected):
    assert len(values) == 2
    assert all(level == pytest.approx(expected, abs=TOLERANCE) for level in values)


def subscribed(cdsp, command, value=None):
    """Subscribe on a second connection, leaving cdsp's own free for commands."""
    client = cdsp.new_client()
    reply = client.send_raw(command, value)
    assert reply["result"] == "Ok", reply
    return client


def test_signal_level_events_carry_the_known_levels(cdsp):
    """Subscribed to both sides, the events alternate between them."""
    client = subscribed(cdsp, "SubscribeSignalLevels", "both")
    events = [value for _, value in client.recv_events(6)]
    by_side = {value["side"]: value for value in events}
    assert set(by_side) == {"capture", "playback"}
    assert_close(by_side["capture"]["peak"], CAPTURE_PEAK_DB)
    assert_close(by_side["capture"]["rms"], CAPTURE_RMS_DB)
    assert_close(by_side["playback"]["peak"], PLAYBACK_PEAK_DB)
    assert_close(by_side["playback"]["rms"], PLAYBACK_RMS_DB)
    client.stop_subscription()


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_subscribing_to_one_side_only(cdsp, side):
    client = subscribed(cdsp, "SubscribeSignalLevels", side)
    events = [value for _, value in client.recv_events(6)]
    assert {value["side"] for value in events} == {side}
    client.stop_subscription()


def test_signal_levels_are_pushed_once_per_chunk(cdsp):
    """The stream follows the audio, so it cannot run faster than chunks arrive."""
    client = subscribed(cdsp, "SubscribeSignalLevels", "capture")
    times = [when for when, _ in client.recv_events(10)]
    # The pacer catches a backlog up in a burst, so a single gap can be half a chunk
    # while the next is one and a half. Only the total over several is steady, and it is
    # the lower bound that says the stream follows the audio instead of spinning.
    elapsed = times[-1] - times[0]
    assert elapsed > (len(times) - 1) * CHUNK_S * 0.8
    assert elapsed < (len(times) - 1) * CHUNK_S * 3
    client.stop_subscription()


def test_vu_events_carry_the_known_levels(cdsp):
    """One VU event carries both sides, unlike the signal level stream."""
    client = subscribed(cdsp, "SubscribeVuLevels", VU)
    _, value = client.recv_events(3)[-1]
    assert_close(value["capture_peak"], CAPTURE_PEAK_DB)
    assert_close(value["capture_rms"], CAPTURE_RMS_DB)
    assert_close(value["playback_peak"], PLAYBACK_PEAK_DB)
    assert_close(value["playback_rms"], PLAYBACK_RMS_DB)
    client.stop_subscription()


def test_vu_max_rate_caps_the_push_rate(cdsp):
    """max_rate is a cap, not a cadence: pushes still land on a chunk boundary."""
    client = subscribed(cdsp, "SubscribeVuLevels", {**VU, "max_rate": 10.0})
    # The first gap is measured from the subscribe, not from a previous push, so drop it.
    times = [when for when, _ in client.recv_events(6)][1:]
    gaps = [later - earlier for earlier, later in zip(times, times[1:])]
    assert min(gaps) > 0.1 - CHUNK_S
    client.stop_subscription()


def test_vu_release_smooths_the_decay(start_cdsp):
    """With release smoothing the meter falls over seconds, not over a chunk."""

    def events_until_quiet(cdsp, release, limit):
        client = subscribed(cdsp, "SubscribeVuLevels", {**VU, "release": release})
        client.recv_events(3)
        cdsp.send("SetMute", True)
        levels = [value["playback_rms"][0] for _, value in client.recv_events(limit)]
        client.stop_subscription()
        return levels

    # Two processes rather than two subscriptions on one, so the second run starts from
    # an unmuted engine and a meter that has not already been dragged down.
    instant = events_until_quiet(start_cdsp(), 0.0, 20)
    assert min(instant) < -40.0
    smoothed = events_until_quiet(start_cdsp(), 2000.0, 20)
    assert min(smoothed) > -40.0


@pytest.mark.parametrize(
    "overrides, message",
    [
        ({"attack": -1.0}, "attack must be between 0 and 60000 ms"),
        ({"release": 60001.0}, "release must be between 0 and 60000 ms"),
    ],
)
def test_a_bad_vu_subscription_is_refused(cdsp, overrides, message):
    reply = cdsp.send_raw("SubscribeVuLevels", {**VU, **overrides})
    assert reply["result"] == "InvalidValueError"
    assert reply["message"] == message
    # A refused subscription leaves the connection unsubscribed, so it still takes
    # ordinary commands.
    assert cdsp.send("GetState") == "Running"


def wait_for_state(client, state, limit=10):
    """Read state events until `state` arrives, and return it.

    The way back up from Inactive goes through Starting, `src/engine_pipeline.rs:157`,
    and whether the state monitor catches that intermediate depends on how fast the
    devices open, so a slow runner sees it and a fast one does not. Waiting for the
    state asked for, rather than asserting on the next event, is what keeps this from
    depending on the runner.
    """
    seen = []
    for _ in range(limit):
        _, value = client.recv_events(1)[0]
        seen.append(value["state"])
        if value["state"] == state:
            return value
    raise AssertionError(f"{state} never arrived, saw {seen}")


def test_state_events_follow_the_engine(start_cdsp):
    """Driven from the other connection, since a subscribed one takes no commands."""
    cdsp = start_cdsp(extra_args=["--wait"])
    client = subscribed(cdsp, "SubscribeState")

    cdsp.send("Stop")
    stopped = wait_for_state(client, "Inactive")
    # The stop reason rides along on the event, and only when Inactive.
    assert stopped["stop_reason"] == "None"

    cdsp.send("Reload")
    started = wait_for_state(client, "Running")
    assert "stop_reason" not in started
    client.stop_subscription()


def test_only_stop_subscription_is_accepted_while_streaming(cdsp):
    client = subscribed(cdsp, "SubscribeSignalLevels", "capture")
    reply = client.send_while_subscribed("GetState")
    assert reply["reply"] == "Invalid"
    assert "Only StopSubscription is accepted" in reply["error"]
    # Refusing a command must not end the stream.
    assert client.recv_events(2)
    client.stop_subscription()


def test_stopping_a_subscription_frees_the_connection(cdsp):
    client = subscribed(cdsp, "SubscribeSignalLevels", "capture")
    client.recv_events(2)
    client.stop_subscription()
    with pytest.raises(TimeoutError):
        client.recv(timeout=0.5)
    # And it is an ordinary connection again, not a spent one.
    assert client.send("GetState") == "Running"
    assert client.send_raw("SubscribeVuLevels", VU)["result"] == "Ok"
    client.stop_subscription()


def test_a_command_on_one_connection_shows_up_in_another_one_s_stream(cdsp):
    """The whole point of the second connection: watch what a command does to the audio."""
    client = subscribed(cdsp, "SubscribeSignalLevels", "playback")
    assert_close(client.recv_events(1)[0][1]["peak"], PLAYBACK_PEAK_DB)
    cdsp.send("SetMute", True)
    quiet = False
    for _, value in client.recv_events(40):
        if max(value["peak"]) < -100.0:
            quiet = True
            break
    assert quiet, "the mute never reached the playback level stream"
    client.stop_subscription()
