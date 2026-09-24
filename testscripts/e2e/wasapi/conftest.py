"""Fixtures for the WASAPI and ASIO suite, which runs on VB-Cable.

In CI these run on a Windows runner with the cable installed from the vendor's driver
pack, see the `wasapi` job in .github/workflows/e2e.yml. Anywhere the cable is missing
the whole directory skips, so selecting it by accident is harmless.

The shared fixtures in the parent conftest.py still do the process handling:
`start_cdsp` and `spawn_cdsp` take the absolute config path `win_config` returns.
"""

import pytest

from .cable import (
    RATE,
    Feeder,
    capture_endpoint,
    devices_present,
    generator_block,
    render_endpoint,
    steinberg_present,
    stdout_block,
    wasapi_block,
)


@pytest.fixture(autouse=True)
def cable():
    if not devices_present():
        pytest.skip("needs VB-Cable installed, see wasapi/conftest.py")


@pytest.fixture
def steinberg():
    """Skip without the Steinberg built-in ASIO Driver. For the ASIO tests, on top of the
    cable, which the autouse fixture above already asks for."""
    if not steinberg_present():
        pytest.skip("needs the Steinberg built-in ASIO Driver installed")


@pytest.fixture(scope="session")
def dummy_backend(camilladsp_bin):
    """Skip without the Dummy devices, which only a dummy-backend build has. For the rate
    adjust tests, where a drifting Dummy is the second clock."""
    import subprocess

    out = subprocess.run([camilladsp_bin, "--help"], capture_output=True, text=True).stdout
    if "Dummy" not in out:
        pytest.skip("needs a build with the dummy-backend feature")


@pytest.fixture
def feeder():
    """Factory for a running Feeder on the cable's render endpoint, stopped when the test
    ends. With no arguments it plays the suite's usual tone."""
    started = []

    def _start(**kwargs):
        feed = Feeder(**kwargs)
        started.append(feed)
        return feed

    yield _start

    for feed in started:
        feed.stop()


@pytest.fixture
def win_config(tmp_path):
    """Factory for a config written to tmp_path, one way through the cable.

    `direction` picks the default ends: "playback" is a generated tone into the cable's
    render endpoint, "capture" is CABLE Output into stdout. `capture` and `playback` take
    the lines of a side from the block helpers in cable.py to replace a default.
    """
    count = [0]

    def _build(
        direction="playback",
        capture=None,
        playback=None,
        samplerate=RATE,
        chunksize=1024,
        devices=None,
    ):
        if capture is None:
            capture = (
                generator_block()
                if direction == "playback"
                else wasapi_block("capture", capture_endpoint()[1])
            )
        if playback is None:
            playback = (
                wasapi_block("playback", render_endpoint()[1])
                if direction == "playback"
                else stdout_block()
            )
        lines = ["devices:", f"  samplerate: {samplerate}", f"  chunksize: {chunksize}"]
        lines += [f"  {key}: {value}" for key, value in (devices or {}).items()]
        lines += capture + playback + ["", "pipeline: []", ""]
        count[0] += 1
        path = tmp_path / f"win{count[0]}.yml"
        path.write_text("\n".join(lines))
        return str(path)

    return _build
