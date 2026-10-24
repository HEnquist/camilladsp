"""The sample rate CamillaDSP asks the PipeWire graph for.

Both nodes set `node.rate` to their own rate, the playback `samplerate` and the capture
`capture_samplerate`, the way pw-cat and the compatibility layers do. It is a request
only: PipeWire switches the driver to it when the rate is in `clock.allowed-rates`, and
that holds just 48 kHz by default, so on a stock install nothing changes. These tests
check the property, that the graph follows it once the rate is allowed, and that it
does not when it is not.

No feeder plays in these tests. pw-cat sets `node.rate` too, at 48 kHz, and the highest
request on a driver wins, so it would pull the graph back to 48 kHz.
"""

import pytest

from .graph import (
    CAPTURE_NODE,
    NOMINAL_RATE,
    PLAYBACK_NODE,
    get_setting,
    node_props,
    set_setting,
    wait_for_driver_rate,
    wait_for_node,
)

pytestmark = pytest.mark.pipewire

OTHER_RATE = 44100
ALLOWED_RATES = "clock.allowed-rates"


@pytest.fixture
def allowed_rates():
    """Let the graph run at OTHER_RATE as well as the nominal rate, for one test."""
    old = get_setting(ALLOWED_RATES)
    set_setting(ALLOWED_RATES, f"[ {OTHER_RATE} {NOMINAL_RATE} ]")
    yield
    set_setting(ALLOWED_RATES, old or f"[ {NOMINAL_RATE} ]")


def test_the_nodes_request_their_rates(start_cdsp, pw_config):
    """Each node asks for the rate of its own side, which differ when resampling."""
    config = pw_config(
        devices={
            "capture_samplerate": OTHER_RATE,
            "resampler": "{type: AsyncPoly, interpolation: Cubic}",
        },
    )
    start_cdsp(config=config)
    wait_for_node(CAPTURE_NODE)
    wait_for_node(PLAYBACK_NODE)
    assert node_props(CAPTURE_NODE)["node.rate"] == f"1/{OTHER_RATE}"
    assert node_props(PLAYBACK_NODE)["node.rate"] == f"1/{NOMINAL_RATE}"


def test_the_graph_stays_at_its_rate_by_default(start_cdsp, pw_config):
    """Without the rate in allowed-rates, the request changes nothing."""
    start_cdsp(config=pw_config(samplerate=OTHER_RATE))
    wait_for_node(CAPTURE_NODE)
    wait_for_driver_rate(CAPTURE_NODE, NOMINAL_RATE)
    wait_for_driver_rate(PLAYBACK_NODE, NOMINAL_RATE)


def test_the_graph_runs_at_the_config_rate_when_allowed(start_cdsp, pw_config, allowed_rates):
    """With the rate allowed, the driver switches to it."""
    start_cdsp(config=pw_config(samplerate=OTHER_RATE))
    wait_for_node(CAPTURE_NODE)
    wait_for_driver_rate(CAPTURE_NODE, OTHER_RATE)
    wait_for_driver_rate(PLAYBACK_NODE, OTHER_RATE)


def test_the_graph_follows_a_new_config(start_cdsp, pw_config, allowed_rates):
    """A config at another rate recreates the nodes, and the graph follows the new request.

    This is what a static WirePlumber rule cannot do. The driver only switches when it is
    idle, so this also checks that the old nodes are gone before the new ones start.
    """
    cdsp = start_cdsp(config=pw_config())
    wait_for_node(CAPTURE_NODE)
    wait_for_driver_rate(CAPTURE_NODE, NOMINAL_RATE)
    for rate in (OTHER_RATE, NOMINAL_RATE):
        with open(pw_config(samplerate=rate)) as conf:
            cdsp.send("SetConfig", conf.read())
        cdsp.poll_until_true("GetConfig", lambda text: f"samplerate: {rate}" in text)
        cdsp.poll_until("GetState", "Running")
        wait_for_driver_rate(CAPTURE_NODE, rate)
        wait_for_driver_rate(PLAYBACK_NODE, rate)
