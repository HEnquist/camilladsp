"""The dummy devices' test control socket.

These tests are about the socket itself: the protocol, the counters, and the listener's
lifetime. What the knobs do to the audio is tested where that behaviour belongs.

The socket is how the later tests make a device misbehave while it is running, so it is
worth proving on its own first. The listener is owned by the device and dies with it, and
that teardown, plus the rebind that follows it on a reload, is the part most likely to
break.
"""

import json
import socket
import time

import pytest

SAMPLERATE = 48000
# Long enough for the frame counters to average out the chunk granularity.
MEASURE_SECONDS = 1.5


@pytest.fixture
def controlled(control_cdsp):
    """A running CamillaDSP with a control socket on each dummy device."""
    return control_cdsp()


def both_controls(cdsp):
    return {"capture": cdsp.capture_control, "playback": cdsp.playback_control}


def test_a_written_key_reads_back(controlled):
    """Writing a knob should change what reading it gives, on that device only."""
    capture = controlled.capture_control
    playback = controlled.playback_control
    assert capture.get("stall") == "0"
    capture.set("stall", 1)
    assert capture.get("stall") == "1"
    # Each device has its own socket and its own state, so a command never has to say
    # which device it meant.
    assert playback.get("stall") == "0"
    capture.set("stall", 0)
    assert capture.get("stall") == "0"
    capture.set("silence", 1)
    assert capture.get("silence") == "1"
    # Drift is signed, and the reply carries the sign back.
    capture.set("drift", -250)
    assert capture.get("drift") == "-250"


def test_a_connection_takes_more_than_one_command(controlled):
    """The client opens a connection per command, but the listener does not require it."""
    with socket.create_connection(("127.0.0.1", controlled.capture_control.port)) as conn:
        conn.sendall(b"stall:1\nstall\n")
        conn.settimeout(5.0)
        replies = b""
        while replies.count(b"\n") < 2:
            replies += conn.recv(256)
    assert replies.decode().split() == ["ok", "stall=1"]


def test_bad_input_is_rejected_with_a_reason(controlled):
    """A mistyped key should fail as an obvious error, not as a confusing timeout."""
    capture = controlled.capture_control
    assert capture.raw("nonsense") == "unknown key: nonsense"
    assert capture.raw("nonsense:1") == "unknown key: nonsense"
    assert capture.raw("stall:maybe") == "bad value: maybe"
    assert capture.raw("drift:lots") == "bad value: lots"


def test_counters_are_read_only(controlled):
    """The counters are keys like any other, but nothing may write them."""
    capture = controlled.capture_control
    before = capture.get_int("frames")
    assert capture.raw("frames:0") == "read only: frames"
    assert capture.raw("resyncs:0") == "read only: resyncs"
    assert capture.get_int("frames") >= before


@pytest.mark.pacing
@pytest.mark.parametrize("device", ["capture", "playback"])
def test_a_normal_run_has_no_pauses_and_no_resyncs(controlled, device):
    """Nothing should be dropped or paused when the devices are left alone.

    A resync is the dummy equivalent of an overrun or underrun, so this is also a check
    that the suite's own machine is keeping up.
    """
    time.sleep(MEASURE_SECONDS)
    counters = both_controls(controlled)[device].counters()
    assert counters["pauses"] == 0
    assert counters["resyncs"] == 0
    assert counters["frames"] > 0


def test_the_socket_comes_back_after_a_reload(control_cdsp, config_file):
    """The listener dies with the device, and the next one rebinds the same port.

    This is both halves of the lifetime in one test: a listener that did not stop would
    leak its thread and hold the port, and a bind that did not retry would lose the race
    against the device it is replacing.
    """
    cdsp = control_cdsp()
    capture = cdsp.capture_control
    capture.wait_until_ready()
    # Let the counter get well clear of zero, so a restart is unmistakable.
    time.sleep(MEASURE_SECONDS)
    before = capture.get_int("frames")
    assert before > SAMPLERATE

    # A changed chunksize is a device change, so both devices are torn down and started
    # again. The ports stay the same, which is the point.
    reloaded = config_file(
        {
            "chunksize: 1024": "chunksize: 2048",
            "control_port: 11111": f"control_port: {capture.port}",
            "control_port: 22222": f"control_port: {cdsp.playback_control.port}",
        },
        base="dummy_control.yml",
    )
    cdsp.send("SetConfigFilePath", reloaded)
    cdsp.send("Reload")

    # Poll from the moment the reload is asked for, since the new counter climbs past
    # `before` a second and a half after it starts. Connections are refused while the
    # port changes hands, which is part of what is being tested.
    deadline = time.monotonic() + 10.0
    lowest = before
    while True:
        try:
            lowest = min(lowest, capture.get_int("frames"))
        except OSError:
            pass
        if lowest < before:
            break
        assert time.monotonic() < deadline, f"frames never restarted, still at {lowest}"
        time.sleep(0.02)

    cdsp.poll_until("GetState", "Running")
    assert json.loads(cdsp.send("GetConfigJson"))["devices"]["chunksize"] == 2048
    # The playback listener made the same move, on its own port.
    cdsp.playback_control.wait_until_ready()
