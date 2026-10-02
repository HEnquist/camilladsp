"""PipeWire nodes appearing, linking up, going away and coming back.

The Dummy devices have no names and nothing to connect to. The PipeWire backend creates
nodes in a graph that other programs see, and they are what a user meets: the names and
group in the config have to reach the nodes, `autoconnect_to` has to link them to the
right targets and to nothing else, and the nodes have to be gone once CamillaDSP has
stopped, rather than lingering in the graph.
"""

import time

import pytest

from .graph import (
    CAPTURE_NODE,
    FEED_NODE,
    GROUP,
    PLAYBACK_NODE,
    SINK_NODE,
    create_null_sink,
    destroy_node,
    linked_nodes,
    node_props,
    record,
    wait_for_links,
    wait_for_node,
)

pytestmark = pytest.mark.pipewire

EXIT_OK = 0
LEVEL_DB = -6.0
LATE_NODE = "cdsp-e2e-late"


def wait_for_peak(cdsp, command, level=LEVEL_DB, timeout=10.0):
    return cdsp.poll_until_true(
        command,
        lambda peaks: len(peaks) == 2 and all(abs(peak - level) < 0.2 for peak in peaks),
        timeout=timeout,
    )


@pytest.fixture
def late_node():
    """The name of a sink that does not exist yet, destroyed again if the test made it."""
    if node_props(LATE_NODE) is not None:
        destroy_node(LATE_NODE)
    yield LATE_NODE
    if node_props(LATE_NODE) is not None:
        destroy_node(LATE_NODE)


def test_the_nodes_carry_the_configured_properties(start_cdsp, pw_config, feeder):
    """The name, description and group from the config, on nodes of the right class."""
    feeder()
    config = pw_config(
        capture={"node_description": "E2E capture"},
        playback={"node_description": "E2E playback"},
    )
    start_cdsp(config=config)
    capture = node_props(CAPTURE_NODE)
    playback = node_props(PLAYBACK_NODE)
    assert capture["node.description"] == "E2E capture"
    assert playback["node.description"] == "E2E playback"
    assert capture["node.group"] == GROUP
    assert playback["node.group"] == GROUP
    assert capture["media.class"] == "Stream/Input/Audio"
    assert playback["media.class"] == "Stream/Output/Audio"


def test_the_nodes_are_linked_to_their_targets(start_cdsp, pw_config, feeder):
    """The capture to the feed sink's monitor, the playback to the other sink, and only that."""
    feeder()
    cdsp = start_cdsp(config=pw_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    wait_for_links(CAPTURE_NODE, {FEED_NODE})
    wait_for_links(PLAYBACK_NODE, {SINK_NODE})


@pytest.mark.parametrize("side", ["capture", "playback"])
def test_a_missing_target_is_linked_when_it_appears(start_cdsp, pw_config, feeder, late_node, side):
    """A target that is not there leaves the node unlinked, until it shows up.

    This is what backend_pipewire.md promises: no fallback to the default device, so a
    misspelled name gives silence rather than audio from somewhere unexpected, and the
    link is made once the target appears. The two null sinks are both candidates for a
    fallback, so a node left alone for a second that is still unlinked did not fall back.
    """
    node = CAPTURE_NODE if side == "capture" else PLAYBACK_NODE
    config = pw_config(**{f"{side}_target": late_node})
    cdsp = start_cdsp(config=config, wait_for_running=False)
    wait_for_node(node)
    time.sleep(1.0)
    assert linked_nodes(node) == set()

    create_null_sink(late_node)
    wait_for_links(node, {late_node})
    # The feeder plays into whichever sink the capture now listens to.
    feeder(target=late_node if side == "capture" else FEED_NODE)
    cdsp.poll_until("GetState", "Running", timeout=10.0)
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    if side == "playback":
        # And the audio really reaches the sink that appeared, not only the meters. Half
        # a second, since the playback ring buffer filled up with silence while nothing
        # was taking from it, and the meters run that far ahead of the sink.
        recorded = record(24000, target=late_node)
        assert abs(recorded).max() > 0.4


def test_a_new_config_reopens_the_devices(start_cdsp, pw_config, feeder):
    """Changing the chunk size rebuilds both nodes, and they have to link up again.

    Done a few times over, since a stream or a main loop that is not torn down leaves a
    node behind in the graph, and the next one would then share its name.
    """
    feeder()
    cdsp = start_cdsp(config=pw_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    for chunksize in (512, 2048, 1024):
        with open(pw_config(chunksize=chunksize)) as conf:
            cdsp.send("SetConfig", conf.read())
        cdsp.poll_until_true("GetConfig", lambda text: f"chunksize: {chunksize}" in text)
        cdsp.poll_until("GetState", "Running")
        wait_for_peak(cdsp, "GetPlaybackSignalPeak")
        assert cdsp.send("GetStopReason") == "None"
        wait_for_links(CAPTURE_NODE, {FEED_NODE})
        wait_for_links(PLAYBACK_NODE, {SINK_NODE})


def test_stop_removes_the_nodes_and_reload_brings_them_back(start_cdsp, pw_config, feeder):
    """After Stop the nodes are gone from the graph, and a reload has to make them again."""
    feeder()
    cdsp = start_cdsp(config=pw_config(), extra_args=["--wait"])
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    wait_for_node(CAPTURE_NODE, present=False)
    wait_for_node(PLAYBACK_NODE, present=False)
    cdsp.send("Reload")
    cdsp.poll_until("GetState", "Running")
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    wait_for_links(CAPTURE_NODE, {FEED_NODE})
    wait_for_links(PLAYBACK_NODE, {SINK_NODE})


def test_the_process_exits_cleanly_while_audio_is_flowing(start_cdsp, pw_config, feeder):
    """Exit has to get both device threads out of their main loops, and the nodes go too."""
    feeder()
    cdsp = start_cdsp(config=pw_config())
    wait_for_peak(cdsp, "GetPlaybackSignalPeak")
    started = time.monotonic()
    assert cdsp.exit() == EXIT_OK
    assert time.monotonic() - started < 5.0
    wait_for_node(CAPTURE_NODE, present=False)
    wait_for_node(PLAYBACK_NODE, present=False)
