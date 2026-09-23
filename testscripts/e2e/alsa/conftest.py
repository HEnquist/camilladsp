"""Fixtures for the ALSA suite, which runs on snd-aloop and snd-dummy.

The GitHub runners' kernel is built without sound, so in CI these run inside a VM, see
the `alsa` job in .github/workflows/e2e.yml. Anywhere the two cards are not loaded the
whole directory skips, so selecting it by accident on another machine is harmless.

The shared fixtures in the parent conftest.py still do the process handling:
`start_cdsp` and `spawn_cdsp` take the absolute config path `alsa_config` returns.
"""

import pytest

from .loopback import (
    CAPTURE,
    CAPTURE_CABLE,
    NOMINAL_SHIFT,
    PLAYBACK,
    PLAYBACK_CABLE,
    Feeder,
    devices_present,
    encode,
    set_rate_shift,
    sine,
)


@pytest.fixture(autouse=True)
def loopback_devices():
    """Skip without the cards, and put both cables back on nominal around every test.

    The rate shift outlives the process that set it, since it belongs to the card, so a
    test that leaves one behind would skew every test after it.
    """
    if not devices_present():
        pytest.skip("needs snd-aloop and snd-dummy loaded, see alsa/conftest.py")
    for cable in (CAPTURE_CABLE, PLAYBACK_CABLE):
        set_rate_shift(cable, NOMINAL_SHIFT)
    yield
    for cable in (CAPTURE_CABLE, PLAYBACK_CABLE):
        set_rate_shift(cable, NOMINAL_SHIFT)


@pytest.fixture
def feeder():
    """Factory for a running Feeder on the capture cable, stopped when the test ends.

    With no block given it plays the suite's usual tone, a 1 kHz sine at -6 dB, in the
    requested format. It returns once the cable end is running, so the format is locked
    in before CamillaDSP opens the other end.
    """
    started = []

    def _start(block=None, fmt="S16_LE", rate=48000, channels=2):
        if block is None:
            block = encode(fmt, sine(fmt, rate // 10, rate=rate, channels=channels))
        feed = Feeder(block, fmt=fmt, rate=rate, channels=channels)
        started.append(feed)
        return feed.wait_until_running()

    yield _start

    for feed in started:
        feed.stop()


@pytest.fixture
def alsa_config(tmp_path):
    """Factory for a config with ALSA on both ends, written to tmp_path.

    Built as text rather than edited from a checked in file, since nearly every test
    varies something different and a replacement per key would be longer than the
    config. `devices` and `capture` add keys to those mappings, and a format of None
    leaves the key out so the backend has to pick one.
    """
    count = [0]

    def _build(
        capture_format="S16_LE",
        playback_format="S16_LE",
        capture_device=CAPTURE,
        playback_device=PLAYBACK,
        samplerate=48000,
        chunksize=1024,
        devices=None,
        capture=None,
    ):
        lines = [
            "devices:",
            f"  samplerate: {samplerate}",
            f"  chunksize: {chunksize}",
        ]
        lines += [f"  {key}: {_yaml(value)}" for key, value in (devices or {}).items()]
        lines += [
            "  capture:",
            "    type: Alsa",
            "    channels: 2",
            f'    device: "{capture_device}"',
        ]
        if capture_format is not None:
            lines.append(f"    format: {capture_format}")
        lines += [f"    {key}: {_yaml(value)}" for key, value in (capture or {}).items()]
        lines += [
            "  playback:",
            "    type: Alsa",
            "    channels: 2",
            f'    device: "{playback_device}"',
        ]
        if playback_format is not None:
            lines.append(f"    format: {playback_format}")
        lines += ["", "pipeline: []", ""]
        count[0] += 1
        path = tmp_path / f"alsa{count[0]}.yml"
        path.write_text("\n".join(lines))
        return str(path)

    return _build


def _yaml(value):
    if isinstance(value, bool):
        return str(value).lower()
    return str(value)
