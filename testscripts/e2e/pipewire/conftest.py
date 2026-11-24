"""Fixtures for the PipeWire suite, which runs on two null sinks.

PipeWire is userspace, so in CI these run in a Debian container on a stock runner with
the daemons started by hand, see the `pipewire` job in .github/workflows/e2e_pipewire.yml.
On a desktop already running PipeWire and WirePlumber they run as they are, and create
the two sinks they need. The whole directory skips when no daemon answers, and when the
binary was built without the `pipewire-backend` feature, so selecting it by accident is
harmless.

The shared fixtures in the parent conftest.py still do the process handling:
`start_cdsp` and `spawn_cdsp` take the absolute config path `pw_config` returns.
"""

import socket
import subprocess
import time

import pytest

from wsclient import Client

from .graph import (
    CAPTURE_NODE,
    FEED_NODE,
    GROUP,
    NOMINAL_RATE,
    PLAYBACK_NODE,
    SINK_NODE,
    Feeder,
    daemon_present,
    ensure_nodes,
    sine,
)


@pytest.fixture(scope="session")
def pipewire_ready(request):
    """Why the suite cannot run here, or None if it can.

    The feature is optional, so a daemon alone is not enough: a dummy build tested on a
    Linux desktop would otherwise fail every test here rather than skip. The binary is
    asked with `GetSupportedDeviceTypes`, as test_stock_build.py does, from a standby
    that opens no device. It is only looked up once there is a daemon, so a machine
    without PipeWire skips without needing a binary at all.
    """
    if not daemon_present():
        return "needs a running PipeWire daemon and pw-cli, pw-dump and pw-cat"
    camilladsp_bin = request.getfixturevalue("camilladsp_bin")
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    process = subprocess.Popen([camilladsp_bin, "-p", str(port), "--wait"])
    try:
        deadline = time.monotonic() + 20.0
        while True:
            try:
                client = Client(port)
                break
            except OSError:
                if process.poll() is not None or time.monotonic() > deadline:
                    raise
                time.sleep(0.05)
        _, capture = client.send("GetSupportedDeviceTypes")
        client.close()
    finally:
        process.kill()
        process.wait(timeout=10.0)
    if "PipeWire" not in capture:
        return "needs a build with the pipewire-backend feature"
    return None


@pytest.fixture(autouse=True)
def pipewire_graph(pipewire_ready):
    """Skip unless the suite can run, and make sure both null sinks are there.

    Checked before every test rather than once, since a test that destroys a node and
    fails before putting it back would otherwise take every test after it along.
    """
    if pipewire_ready is not None:
        pytest.skip(f"{pipewire_ready}, see pipewire/conftest.py")
    ensure_nodes()


@pytest.fixture
def feeder():
    """Factory for a running Feeder into the feed sink, stopped when the test ends.

    With no block given it plays the suite's usual tone, a 1 kHz sine at -6 dB, at the
    graph rate.
    """
    started = []

    def _start(block=None, target=FEED_NODE):
        if block is None:
            block = sine(NOMINAL_RATE // 10)
        feed = Feeder(block, target=target)
        started.append(feed)
        return feed

    yield _start

    for feed in started:
        feed.stop()


@pytest.fixture
def pw_config(tmp_path):
    """Factory for a config with PipeWire on both ends, written to tmp_path.

    Built as text, like the ALSA suite's. The capture captures the feed sink's monitor
    and the playback plays into the other sink, both through `autoconnect_to`, and the
    two targets are parameters so a test can point one at a node that is not there.
    `devices`, `capture` and `playback` add keys to those mappings.
    """
    count = [0]

    def _build(
        capture_target=FEED_NODE,
        playback_target=SINK_NODE,
        samplerate=NOMINAL_RATE,
        chunksize=1024,
        devices=None,
        capture=None,
        playback=None,
    ):
        lines = [
            "devices:",
            f"  samplerate: {samplerate}",
            f"  chunksize: {chunksize}",
        ]
        lines += [f"  {key}: {_yaml(value)}" for key, value in (devices or {}).items()]
        lines += _device("capture", CAPTURE_NODE, capture_target, {"loopback": True, **(capture or {})})
        lines += _device("playback", PLAYBACK_NODE, playback_target, playback)
        lines += ["", "pipeline: []", ""]
        count[0] += 1
        path = tmp_path / f"pipewire{count[0]}.yml"
        path.write_text("\n".join(lines))
        return str(path)

    return _build


def _device(side, node_name, target, extra):
    lines = [
        f"  {side}:",
        "    type: PipeWire",
        "    channels: 2",
        f"    node_name: {node_name}",
        f"    node_group_name: {GROUP}",
        f'    autoconnect_to: "{target}"',
    ]
    lines += [f"    {key}: {_yaml(value)}" for key, value in (extra or {}).items()]
    return lines


def _yaml(value):
    if isinstance(value, bool):
        return str(value).lower()
    return str(value)
