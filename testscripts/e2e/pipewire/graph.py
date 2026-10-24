"""The PipeWire nodes the PipeWire suite runs on, and the tools that drive them.

Two null sinks, which the suite creates itself when they are missing. A null sink is an
`Audio/Sink` with nothing behind it, and like every sink it has monitor ports that carry
whatever is played into it.

- `FEED_NODE` is the capture side. The test plays into it with a `Feeder`, and
  CamillaDSP captures its monitor, with `loopback: true` and `autoconnect_to` naming it.
- `SINK_NODE` is the playback side. CamillaDSP plays into it with `autoconnect_to`, and
  the test records its monitor with `record` when it wants to see the output.

So every test goes through autoconnect on both ends and through loopback capture, which
is the setup backend_pipewire.md describes for processing what another program plays.

Four properties of PipeWire decide how the tests are written:

- Everything in the graph is 32 bit float, and the CamillaDSP nodes always ask for F32.
  With the stream at the graph rate there is nothing to convert, so an empty pipeline
  should hand back exactly what the feeder played.
- The graph runs at one rate, 48 kHz unless told otherwise, and a stream at another rate
  is resampled in its own adapter. A config at another rate still runs. CamillaDSP asks
  for its rate with `node.rate`, but `clock.allowed-rates` holds only 48 kHz by default,
  so the graph is never switched unless a test allows it, as test_pipewire_rate.py does.
- A null sink is a driver on the system clock, and nodes in one `node.group` are
  scheduled by one driver, so both ends of CamillaDSP run on the same clock and nothing
  drifts.
- The backend sets `node.dont-fallback` and `node.linger` with `autoconnect_to`, so a
  target that does not exist leaves the node unlinked rather than on the default
  device, and WirePlumber links it once the target appears. Those are WirePlumber 0.5
  keys, which is why CI runs on a distro that ships it.

The graph is read with `pw-dump` and changed with `pw-cli`, and the audio goes through
`pw-cat` in raw mode on stdin and stdout, so no Python binding is needed.
"""

import json
import shutil
import subprocess
import threading
import time

import numpy as np

FEED_NODE = "cdsp-e2e-feed"
SINK_NODE = "cdsp-e2e-sink"
NOMINAL_RATE = 48000

# CamillaDSP's own node names and group in the suite's configs, rather than the
# defaults, so a CamillaDSP running on the same desktop is never touched.
CAPTURE_NODE = "cdsp-e2e-capture"
PLAYBACK_NODE = "cdsp-e2e-playback"
GROUP = "cdsp-e2e"

_NODE = "PipeWire:Interface:Node"
_LINK = "PipeWire:Interface:Link"


def _run(*args, timeout=5.0):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=True)


def daemon_present():
    """Whether the tools are installed and a PipeWire daemon answers them."""
    if not all(shutil.which(tool) for tool in ("pw-cli", "pw-dump", "pw-cat")):
        return False
    try:
        _run("pw-cli", "info", "0")
    except (subprocess.SubprocessError, OSError):
        return False
    return True


def dump():
    """Every object in the graph, as `pw-dump` describes it."""
    return json.loads(_run("pw-dump").stdout)


def _node_ids(objects, name):
    return [
        obj["id"]
        for obj in objects
        if obj["type"] == _NODE and obj["info"]["props"].get("node.name") == name
    ]


def node_props(name):
    """The properties of the node called `name`, or None if there is none."""
    for obj in dump():
        if obj["type"] == _NODE and obj["info"]["props"].get("node.name") == name:
            return obj["info"]["props"]
    return None


def linked_nodes(name, objects=None):
    """The names of the nodes linked to the node called `name`, in either direction.

    The links of every node with that name are merged, so this cannot tell one node
    from two. `wait_for_links` checks the count as well.
    """
    if objects is None:
        objects = dump()
    ids = set(_node_ids(objects, name))
    names = {
        obj["id"]: obj["info"]["props"].get("node.name")
        for obj in objects
        if obj["type"] == _NODE
    }
    linked = set()
    for obj in objects:
        if obj["type"] != _LINK:
            continue
        output, input_ = obj["info"]["output-node-id"], obj["info"]["input-node-id"]
        if output in ids:
            linked.add(names.get(input_))
        elif input_ in ids:
            linked.add(names.get(output))
    return linked


def wait_for_node(name, present=True, timeout=5.0):
    """Wait for a node to appear, or with `present` False to be gone."""
    deadline = time.monotonic() + timeout
    while (node_props(name) is not None) != present:
        if time.monotonic() > deadline:
            state = "did not appear" if present else "is still there"
            raise TimeoutError(f"node {name} {state} after {timeout} s")
        time.sleep(0.05)


def wait_for_links(name, expected, timeout=5.0):
    """Wait for one node called `name`, linked to exactly the nodes in `expected`.

    One node, since a node left behind by an earlier session would link to the same
    targets and look like the new one otherwise. Waited for rather than checked once,
    so the old node going away just after the new one appears is not a failure.
    """
    deadline = time.monotonic() + timeout
    while True:
        objects = dump()
        count = len(_node_ids(objects, name))
        linked = linked_nodes(name, objects)
        if count == 1 and linked == set(expected):
            return
        if time.monotonic() > deadline:
            raise TimeoutError(
                f"{count} nodes called {name} are linked to {linked}, not one to {set(expected)}"
            )
        time.sleep(0.05)


def driver_rate(name):
    """The rate of the driver that schedules the node called `name`, or None.

    Read from `pw-top`, since the rate a driver runs at is not a property and `pw-dump`
    does not show it. pw-top lists each driver followed by its followers, which carry a
    `+` before the name, and only the last of its two iterations is used, since the
    first has no timing yet. RATE is the fourth column, and the name is always last.
    """
    lines = _run("pw-top", "-b", "-n", "2").stdout.splitlines()
    headers = [index for index, line in enumerate(lines) if line.split()[:2] == ["S", "ID"]]
    rate = None
    for line in lines[headers[-1] + 1 :]:
        fields = line.split()
        if len(fields) < 4:
            continue
        if fields[-2] != "+":
            rate = int(fields[3])
        if fields[-1] == name:
            return rate
    return None


def wait_for_driver_rate(name, expected, timeout=5.0):
    """Wait for the node called `name` to be scheduled by a driver at `expected` Hz."""
    deadline = time.monotonic() + timeout
    while (rate := driver_rate(name)) != expected:
        if time.monotonic() > deadline:
            raise TimeoutError(f"the driver of {name} runs at {rate}, not {expected}")
        time.sleep(0.1)


def get_setting(key):
    """The value of `key` in PipeWire's settings metadata, as `pw-metadata` prints it."""
    for line in _run("pw-metadata", "-n", "settings", "0", key).stdout.splitlines():
        if f"key:'{key}'" in line:
            return line.split("value:'", 1)[1].split("'", 1)[0]
    return None


def set_setting(key, value):
    """Set `key` in PipeWire's settings metadata.

    There is deliberately no way to delete one: deleting `clock.allowed-rates` takes the
    daemon down in PipeWire 1.4.2, so a test puts the old value back instead.
    """
    _run("pw-metadata", "-n", "settings", "0", key, value)


def create_null_sink(name):
    """Create a 2 channel null sink that outlives the pw-cli that made it."""
    props = (
        "{ factory.name = support.null-audio-sink"
        f" node.name = {name} node.description = {name}"
        " media.class = Audio/Sink object.linger = true"
        " audio.channels = 2 audio.position = [ FL FR ] }"
    )
    _run("pw-cli", "create-node", "adapter", props)
    wait_for_node(name)


def destroy_node(name):
    for node_id in _node_ids(dump(), name):
        _run("pw-cli", "destroy", str(node_id))
    wait_for_node(name, present=False)


def ensure_nodes():
    """Create the two null sinks if they are missing, which they are on a fresh daemon."""
    for name in (FEED_NODE, SINK_NODE):
        if node_props(name) is None:
            create_null_sink(name)


def noise(frames, channels=2, seed=1):
    """Random float32 samples inside +/-0.9, so nothing clips.

    Random rather than a tone because a window of noise matches the input at exactly one
    offset, which is what lets a test find where a recording sits in the loop the feeder
    plays.
    """
    rng = np.random.default_rng(seed)
    return rng.uniform(-0.9, 0.9, (frames, channels)).astype(np.float32)


def sine(frames, level_db=-6.0, freq=1000.0, rate=NOMINAL_RATE, channels=2):
    """A float32 sine at `level_db` peak. `frames` should hold a whole number of periods."""
    amplitude = 10 ** (level_db / 20)
    wave = amplitude * np.sin(2 * np.pi * freq * np.arange(frames) / rate)
    return np.repeat(wave[:, None], channels, axis=1).astype(np.float32)


def find_in_loop(block, window):
    """Where `window` starts in `block` played in a loop, or None if it is not in it.

    The window has to match exactly and in full, so this is the bit exactness check as
    well as the alignment.
    """
    doubled = np.concatenate([block, block])
    candidates = np.flatnonzero((block == window[0]).all(axis=1))
    for start in candidates:
        if np.array_equal(doubled[start : start + len(window)], window):
            return int(start)
    return None


def exact_fraction(block, window, piece):
    """The fraction of `piece` frame pieces of `window` found exactly in `block` looped.

    A dropout leaves a gap in the output, and one can land anywhere on a busy runner, so
    a window that has to match in one piece would fail on timing rather than on the
    audio. Cut into pieces, a conversion bug still fails every piece, while a gap only
    breaks the piece it falls in.
    """
    pieces = [window[start : start + piece] for start in range(0, len(window) - piece + 1, piece)]
    found = sum(1 for part in pieces if find_in_loop(block, part) is not None)
    return found / len(pieces)


class Feeder:
    """A block played into a node in a loop by pw-cat, until stopped.

    pw-cat reads stdin only as fast as the graph takes the audio, so the writer thread
    is paced by PipeWire and never runs ahead by more than the pipe holds. It plays at
    the graph rate, so nothing is resampled on the way in.
    """

    def __init__(self, block, rate=NOMINAL_RATE, target=FEED_NODE):
        self._data = np.ascontiguousarray(block, dtype=np.float32).tobytes()
        self._stop = threading.Event()
        self.process = subprocess.Popen(
            [
                "pw-cat", "--playback", "--raw", "--format", "f32",
                "--rate", str(rate), "--channels", str(block.shape[1]),
                "--target", target, "-",
            ],
            stdin=subprocess.PIPE,
        )
        self._thread = threading.Thread(target=self._write, daemon=True)
        self._thread.start()

    def _write(self):
        try:
            while not self._stop.is_set():
                self.process.stdin.write(self._data)
                self.process.stdin.flush()
        except (BrokenPipeError, ValueError, OSError):
            pass

    def stop(self):
        self._stop.set()
        self.process.kill()
        self.process.wait(timeout=5.0)
        self._thread.join(timeout=5.0)


def record(frames, rate=NOMINAL_RATE, target=SINK_NODE, channels=2):
    """Record `frames` from the monitor of a sink, as float32."""
    process = subprocess.Popen(
        [
            "pw-cat", "--record", "--raw", "--format", "f32",
            "--rate", str(rate), "--channels", str(channels),
            "--target", target, "-P", "{ stream.capture.sink = true }", "-",
        ],
        stdout=subprocess.PIPE,
    )
    wanted = frames * channels * 4
    data = bytearray()
    try:
        while len(data) < wanted:
            chunk = process.stdout.read1(wanted - len(data))
            if not chunk:
                raise RuntimeError(f"pw-cat stopped after {len(data)} of {wanted} bytes")
            data += chunk
    finally:
        process.kill()
        process.wait(timeout=5.0)
    return np.frombuffer(bytes(data), dtype=np.float32).reshape(frames, channels)
