"""Fixtures for the CoreAudio suite, which runs on two BlackHole devices.

In CI these run on a macOS runner with the latest blackhole-2ch and blackhole-16ch
casks, see the `coreaudio` job in .github/workflows/e2e_coreaudio.yml. Anywhere the two devices
are missing the whole directory skips, so selecting it by accident is harmless.

The shared fixtures in the parent conftest.py still do the process handling:
`start_cdsp` and `spawn_cdsp` take the absolute config path `ca_config` returns.
"""

import pytest

from .blackhole import (
    FEED_DEVICE,
    NOMINAL_RATE,
    SINK_DEVICE,
    Feeder,
    devices_present,
    reset,
    sine,
)


@pytest.fixture(autouse=True)
def blackhole_devices():
    """Skip without the devices, and put both back as installed around every test.

    The rate, the clock source and the pitch belong to the device and outlive the
    process that set them, so a test that leaves one behind would skew every test after
    it. The feeder's device is reset last, since a rate change on it is what a stopped
    feeder would otherwise leave for the next test to find.
    """
    if not devices_present():
        pytest.skip("needs BlackHole 2ch and 16ch installed, see coreaudio/conftest.py")
    for device in (SINK_DEVICE, FEED_DEVICE):
        reset(device)
    yield
    for device in (SINK_DEVICE, FEED_DEVICE):
        reset(device)


@pytest.fixture
def feeder():
    """Factory for a running Feeder on BlackHole 2ch, stopped when the test ends.

    With no block given it plays the suite's usual tone, a 1 kHz sine at -6 dB. It
    opens at `rate`, which should be the rate the device is at, see `Feeder`.
    """
    started = []

    def _start(block=None, rate=NOMINAL_RATE):
        if block is None:
            block = sine(rate // 10, rate=rate)
        feed = Feeder(block, rate=rate)
        started.append(feed)
        return feed

    yield _start

    for feed in started:
        feed.stop()


@pytest.fixture
def ca_config(tmp_path):
    """Factory for a config with CoreAudio on both ends, written to tmp_path.

    Built as text, like the ALSA suite's, since nearly every test varies something
    different. `devices`, `capture` and `playback` add keys to those mappings, and a
    format of None leaves the key out, which has the backend leave the physical format
    alone and only set the rate.
    """
    count = [0]

    def _build(
        capture_format=None,
        playback_format=None,
        capture_device=FEED_DEVICE,
        playback_device=SINK_DEVICE,
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
        lines += _device("capture", capture_device, capture_format, capture)
        lines += _device("playback", playback_device, playback_format, playback)
        lines += ["", "pipeline: []", ""]
        count[0] += 1
        path = tmp_path / f"coreaudio{count[0]}.yml"
        path.write_text("\n".join(lines))
        return str(path)

    return _build


def _device(side, device, fmt, extra):
    lines = [
        f"  {side}:",
        "    type: CoreAudio",
        "    channels: 2",
        f'    device: "{device}"',
    ]
    if fmt is not None:
        lines.append(f"    format: {fmt}")
    lines += [f"    {key}: {_yaml(value)}" for key, value in (extra or {}).items()]
    return lines


def _yaml(value):
    if isinstance(value, bool):
        return str(value).lower()
    return str(value)
