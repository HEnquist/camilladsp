"""Config churn: hammering the engine with back to back config changes.

The port of the old `testscripts/config_load_test`, which needed a CamillaDSP someone had
already started by hand against CoreAudio hardware, and which this replaces. Everything
here runs on the dummy devices in CI instead, and the fixed settle sleeps are polls, which
is what took the suite from about five minutes to well under one.

The four configs differ in chunksize as well as in the filter, so every change here is a
`ConfigChange::Devices` and takes the full stop and restart path, `src/engine.rs:145`. That
is the path worth hammering: the cheap in place pipeline update is covered separately at
the bottom of this file.
"""

import collections
import json
import os
import shutil
import signal
import sys
import time

import pytest

unix_only = pytest.mark.skipif(sys.platform == "win32", reason="no unix signals on Windows")

# Repetitions per rapid churn case, and the delays between the sets within one.
# The hardware suite used 50 at each delay. 20 is the same shape of test, and keeps the
# nine rapid cases together at well under a minute even on the slowest CI runner.
REPS = 20
DELAYS = [0.1, 0.01, 0.001]

# How many times the hot reload cases cycle through their three configs.
ROUNDS = 4

# Distinct per variant. The chunksize is what makes the change a device change, the gain
# is there so the reload has real work to do, and the title is the cheapest thing to poll.
CHUNKSIZES = [512, 1024, 2048, 4096]
GAINS = [-6.0, -5.0, -4.0, -3.0]

# Generous, because one settle can cover a full device restart on a loaded runner.
SETTLE_TIMEOUT = 20.0


def edits(n):
    """The replacements that turn dummy_sine.yml into variant `n`, counted from zero."""
    return {
        "---": f'---\ntitle: "nbr {n + 1}"',
        "chunksize: 1024": f"chunksize: {CHUNKSIZES[n]}",
        "gain: -6.0": f"gain: {GAINS[n]}",
    }


@pytest.fixture
def variants(config_file):
    """The four configs the churn cycles through, as (text, path) pairs."""
    built = []
    for n in range(4):
        path = config_file(edits(n))
        with open(path) as conf:
            built.append((conf.read(), path))
    return built


def settle(cdsp, n):
    """Wait until variant `n` is the active config and the engine is running again.

    This is what replaces the fixed 0.5 s sleep. Two polls, because a device change
    publishes the new config as soon as the old pipeline has stopped and before the new
    one is up, so the title arrives well ahead of the restart finishing.

    The title is a safe thing to poll on for anything but a byte identical config, which
    is the one case `config_diff` answers `ConfigChange::None` for, `src/config/utils.rs:478`.
    A config that differs only in its title falls through to an empty `FilterParameters`
    and is published like any other.
    """
    cdsp.poll_until("GetConfigTitle", f"nbr {n + 1}", timeout=SETTLE_TIMEOUT)
    cdsp.poll_until("GetState", "Running", timeout=SETTLE_TIMEOUT)
    assert cdsp.send("GetConfigValue", "/devices/chunksize") == CHUNKSIZES[n]
    assert cdsp.send("GetConfigValue", "/filters/testgain/parameters/gain") == GAINS[n]


def send_retrying(cdsp, command, value=None):
    """Send a config changing command, retrying while the controller queue is full.

    That queue holds ten messages and is not read at all while the devices restart,
    `src/engine.rs:107`, so churn at these delays can legitimately be told to back off.
    A real client retries, and so does this, rather than the suite going red on correct
    behaviour. In practice it almost never trips at 20 repetitions.
    """
    deadline = time.monotonic() + SETTLE_TIMEOUT
    while True:
        reply = cdsp.send_raw(command, value)
        if reply["result"] != "RateLimitExceededError":
            assert reply["result"] == "Ok", reply
            return
        assert time.monotonic() < deadline, f"{command} was rate limited for {SETTLE_TIMEOUT} s"
        time.sleep(0.01)


def set_via_ws(cdsp, variants, n):
    """Upload variant `n` over the websocket."""
    send_retrying(cdsp, "SetConfig", variants[n][0])


def set_via_path(cdsp, variants, n):
    """Point the active path at variant `n` and reload from it."""
    cdsp.send("SetConfigFilePath", variants[n][1])
    send_retrying(cdsp, "Reload")


def set_via_sighup(cdsp, variants, n, active_path):
    """Swap variant `n` in under the active path and signal a reload.

    Written through a rename rather than copied in place: the rename is atomic, so the
    signal handler cannot read a file that is half written. With a plain copy that is a
    real race at a 1 ms delay, and it surfaces as a reload silently dropped with a config
    error in the log.
    """
    shutil.copy(variants[n][1], active_path + ".new")
    os.replace(active_path + ".new", active_path)
    cdsp.process.send_signal(signal.SIGHUP)


@pytest.fixture
def sighup_cdsp(start_cdsp, variants, tmp_path):
    """A CamillaDSP started on a config file the SIGHUP cases are free to overwrite."""
    active = str(tmp_path / "active.yml")
    shutil.copy(variants[0][1], active)
    return start_cdsp(config=active), active


# ---------- The four configs applied one at a time ----------


def test_slow_sequence_via_set_config(cdsp, variants):
    """Each of the four uploaded in turn, waiting for each to land."""
    for n in range(4):
        set_via_ws(cdsp, variants, n)
        settle(cdsp, n)


def test_slow_sequence_via_path(cdsp, variants):
    """The same four, reached by moving the config path and reloading."""
    for n in range(4):
        set_via_path(cdsp, variants, n)
        settle(cdsp, n)


@unix_only
def test_slow_sequence_via_sighup(sighup_cdsp, variants):
    """And again, by rewriting the file on disk and signalling."""
    cdsp, active = sighup_cdsp
    for n in range(4):
        set_via_sighup(cdsp, variants, n, active)
        settle(cdsp, n)


# ---------- The same, as fast as the engine will take them ----------


def churn(cdsp, setter, delay):
    """Run the rapid burst REPS times, alternating which config each burst lands on.

    The alternation matters more than it looks. A burst that always ends where the previous
    one did can collapse into doing nothing at all: the two configs in the middle are
    dropped the moment something is queued behind them, `src/engine.rs:117`, and the last
    one is then identical to the running config, so `config_diff` reports `None` and the
    engine never restarts. Landing on nbr 4 and nbr 1 by turns forces a real device change
    out of every single burst, which is the thing under test.
    """
    setter(0)
    settle(cdsp, 0)
    for rep in range(REPS):
        order = [1, 2, 3] if rep % 2 == 0 else [2, 1, 0]
        for n in order[:-1]:
            setter(n)
            time.sleep(delay)
        setter(order[-1])
        settle(cdsp, order[-1])


@pytest.mark.parametrize("delay", DELAYS, ids=lambda d: f"{int(1000 * d)}ms")
def test_rapid_churn_via_set_config(cdsp, variants, delay):
    """Uploads back to back, checking that each burst leaves the last config running."""
    churn(cdsp, lambda n: set_via_ws(cdsp, variants, n), delay)


@pytest.mark.parametrize("delay", DELAYS, ids=lambda d: f"{int(1000 * d)}ms")
def test_rapid_churn_via_path(cdsp, variants, delay):
    """The same burst through the path and reload route, which is two commands per set."""
    churn(cdsp, lambda n: set_via_path(cdsp, variants, n), delay)


@unix_only
@pytest.mark.parametrize("delay", DELAYS, ids=lambda d: f"{int(1000 * d)}ms")
def test_rapid_churn_via_sighup(sighup_cdsp, variants, delay):
    """And through the signal, where the reload reads whatever is on disk when it runs.

    Signals coalesce, so a burst of three can reach the handler as one. It still reads the
    file after the last write, so the burst still converges on the last config.
    """
    cdsp, active = sighup_cdsp
    churn(cdsp, lambda n: set_via_sighup(cdsp, variants, n, active), delay)


def test_an_unpaced_hammer_is_refused_and_recovered_from(cdsp, variants):
    """A burst with no pacing at all should hit backpressure, not break anything.

    Every case above pauses on every third set, so none of them ever fills the controller
    queue. This one deliberately does. One device restart stops the queue being read for
    tens of milliseconds, `src/engine.rs:107`, which is far longer than it takes to push
    another ten configs in, so most of these come back refused rather than applied: about
    seven in ten locally. What matters is that the refusal is the documented
    `RateLimitExceededError` and nothing else, and that the engine is still there and
    still takes a config afterwards.
    """
    results = collections.Counter()
    for _ in range(REPS):
        for n in range(4):
            results[cdsp.send_raw("SetConfig", variants[n][0])["result"]] += 1
    assert set(results) <= {"Ok", "RateLimitExceededError"}, results
    assert results["RateLimitExceededError"] > 0, (
        "the queue never filled, so this tested nothing"
    )

    send_retrying(cdsp, "SetConfig", variants[2][0])
    settle(cdsp, 2)
    assert cdsp.is_running()


# ---------- Changes that do not touch the devices, so no restart ----------


def pipeline_with(names):
    """A pipeline holding one filter step with these names, or no steps at all."""
    return [{"type": "Filter", "channels": [0, 1], "names": names}] if names else []


def active_pipeline_names(cdsp):
    """The filter names in each step of the running pipeline."""
    return [step["names"] for step in json.loads(cdsp.send("GetConfigJson"))["pipeline"]]


@pytest.mark.parametrize("command", ["SetConfig", "SetConfigJson"])
def test_pipeline_only_changes(cdsp, command):
    """Configs differing only in the pipeline are updated in place, not restarted.

    Both commands carry the same JSON document, since JSON is valid YAML. The point of
    running it twice is the two command handlers, not the two formats.
    """
    for round_nbr in range(ROUNDS):
        for step, names in enumerate([["testgain"], ["testgain", "testgain"], []]):
            title = f"pipeline {round_nbr}.{step}"
            config = json.loads(cdsp.send("GetConfigJson"))
            config["pipeline"] = pipeline_with(names)
            config["title"] = title
            cdsp.send(command, json.dumps(config))
            cdsp.poll_until("GetConfigTitle", title)
            assert active_pipeline_names(cdsp) == ([names] if names else [])
            assert cdsp.send("GetState") == "Running"


def test_filter_only_changes(cdsp):
    """Configs differing only in a filter parameter take the cheapest path of all.

    `config_diff` returns `FilterParameters` here, which hands the new coefficients to the
    running filter without rebuilding anything, `src/engine.rs:120`.
    """
    for round_nbr in range(ROUNDS):
        for step, gain in enumerate([-6.0, -5.0, -4.0]):
            title = f"filter {round_nbr}.{step}"
            config = json.loads(cdsp.send("GetConfigJson"))
            config["filters"]["testgain"]["parameters"]["gain"] = gain
            config["title"] = title
            cdsp.send("SetConfigJson", json.dumps(config))
            cdsp.poll_until("GetConfigTitle", title)
            assert cdsp.send("GetConfigValue", "/filters/testgain/parameters/gain") == gain
            assert cdsp.send("GetState") == "Running"
