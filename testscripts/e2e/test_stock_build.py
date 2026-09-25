"""What a stock build has, and what it must refuse.

Every other file in this suite would pass just as well against a `dummy-backend` build,
since the feature only adds devices and changes nothing on the shared path. These are
the ones that would not: they assert that the test-only devices are absent, which is the
statement the whole suite is built on and the only thing standing between "never enable
this in a release build" and someone finding out the hard way.

The rest is the unhappy path of the software devices themselves. A config naming a
device that does not exist, and a filter pointing at a file that is not there, are both
caught before any device is opened, which is worth pinning down because it decides
whether the user sees a startup error with the reason in it or a process that comes up
and then fails.
"""

import pytest

from conftest import FILE_CAPTURE, FILE_PLAYBACK
from swdevices import file_playback, raw_capture

pytestmark = pytest.mark.stock

EXIT_BAD_CONFIG = 101

DUMMY_CAPTURE = """  capture:
    type: Dummy
    channels: 2
    signal:
      type: Sine
      freq: 1000
      level: -6.0"""
DUMMY_PLAYBACK = """  playback:
    type: Dummy
    channels: 2"""


def blocks(tmp_path, capture=None, playback=None):
    """Both device blocks, with whichever one is not given filled in as a real file device.

    These configs are meant to be rejected, so the filenames in them are never opened.
    Filling them in anyway is what keeps the `PLAYBACK_FILE` placeholder from becoming a
    real file in the working directory the day one of them is accepted by mistake, which
    is exactly what happens when the suite is pointed at the wrong build.
    """
    return {
        FILE_CAPTURE: capture or raw_capture(str(tmp_path / "in.raw"), "F64_LE"),
        FILE_PLAYBACK: playback or file_playback(str(tmp_path / "out.raw"), "F64_LE"),
    }


@pytest.fixture
def standby(start_cdsp):
    """A stock CamillaDSP standing by with no config, for the commands that need no audio.

    `--wait` with no config file starts in `Inactive` and stays there, which is all
    `ValidateConfig` and the getters below need, and it means these tests open no device
    at all.
    """
    return start_cdsp(config=None, extra_args=["--wait"], wait_for_running=False)


def test_the_dummy_devices_are_absent_from_a_stock_build(standby):
    """`GetSupportedDeviceTypes` is the binary describing itself, so it is the check.

    If this ever lists Dummy, the feature is on in a build that should not have it and
    every `stock` marked test in this suite has been running against the wrong binary
    without saying so.
    """
    playback, capture = standby.send("GetSupportedDeviceTypes")
    assert "Dummy" not in capture
    assert "Dummy" not in playback
    # And the software devices the rest of this suite uses really are all there.
    assert {"RawFile", "WavFile", "Stdin", "SignalGenerator"} <= set(capture)
    assert {"File", "Stdout"} <= set(playback)


@pytest.mark.parametrize(
    "block", [DUMMY_CAPTURE, DUMMY_PLAYBACK], ids=["capture", "playback"]
)
def test_a_dummy_config_is_rejected_with_the_variant_named(
    standby, config_file, tmp_path, block
):
    """A config asking for a device this build lacks says which one, and what it does have.

    `deny_unknown_fields` and serde's untagged variant list do the work, so the error
    names the device and lists the alternatives. Asserted over the websocket rather than
    from the process exit code because that is where the message is readable: a GUI
    sending a config gets this string, and "unknown variant" is what tells its user the
    build is wrong rather than the config.
    """
    side = "capture" if block is DUMMY_CAPTURE else "playback"
    with open(
        config_file(blocks(tmp_path, **{side: block}), base="file_devices.yml")
    ) as conf:
        text = conf.read()

    reply = standby.send_raw("ValidateConfig", text)
    assert reply["result"] == "ConfigReadError"
    assert "unknown variant `Dummy`" in reply["value"]
    assert "RawFile" in reply["value"] or "File" in reply["value"]


def test_a_device_type_that_exists_nowhere_is_rejected_the_same_way(
    standby, config_file, tmp_path
):
    """And a plain typo gets the same treatment, which is what makes the check above useful.

    Otherwise the Dummy rejection could be a special case rather than the general one,
    and a future build that happened to accept the name would still pass it.
    """
    unknown = "  capture:\n    type: Nonexistent\n    channels: 2"
    with open(
        config_file(blocks(tmp_path, capture=unknown), base="file_devices.yml")
    ) as conf:
        text = conf.read()

    reply = standby.send_raw("ValidateConfig", text)
    assert reply["result"] == "ConfigReadError"
    assert "unknown variant `Nonexistent`" in reply["value"]


def test_a_dummy_config_on_the_command_line_stops_the_process(
    spawn_cdsp, config_file, tmp_path
):
    """The same config given at startup never gets as far as the websocket.

    A bad config on disk is `EXIT_BAD_CONFIG` before anything else happens, so a user who
    points a release build at a config written for a test build gets an exit and a
    message rather than a process that appears to start.
    """
    config = config_file(
        blocks(tmp_path, capture=DUMMY_CAPTURE), base="file_devices.yml"
    )
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=20) == EXIT_BAD_CONFIG


def test_a_missing_coefficient_file_is_caught_before_the_devices_open(
    spawn_cdsp, config_file, tmp_path
):
    """A Conv filter whose coefficients are not there is a config error, not a device one.

    Same shape as the missing capture file in `test_file_devices.py`: validation reads
    the file, so the process exits before the websocket comes up and the pipeline never
    gets built. Worth its own test because the filter is several steps further from the
    config loader than a capture device is, and the answer is not obviously the same.
    """
    source = str(tmp_path / "in.raw")
    open(source, "wb").write(b"\x00" * 1024)
    replacements = blocks(tmp_path, capture=raw_capture(source, "F64_LE"))
    replacements.update(
        {
            "pipeline: []": f"""filters:
  missing:
    type: Conv
    parameters:
      type: Raw
      filename: {tmp_path / "nosuch.raw"}
      format: F64_LE

pipeline:
  - type: Filter
    channels: [0, 1]
    names: [missing]""",
        }
    )
    config = config_file(replacements, base="file_devices.yml")
    process, _ = spawn_cdsp(config=config)
    assert process.wait(timeout=20) == EXIT_BAD_CONFIG
