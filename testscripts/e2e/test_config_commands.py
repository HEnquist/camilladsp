"""The config command surface: reading, writing, patching and validating.

These are pure control plane, so they are cheap, and they catch the serialization
regressions that otherwise stay invisible until a GUI breaks on them.
"""

import json
import os

import pytest

HERE = os.path.dirname(os.path.abspath(__file__))


def base_config():
    with open(os.path.join(HERE, "dummy_sine.yml")) as conf:
        return conf.read()


def titled(title):
    """The base config with a title, which is the cheapest thing to assert on."""
    return base_config().replace("---", f'---\ntitle: "{title}"', 1)


def test_get_config_returns_yaml(cdsp):
    """GetConfig should hand back the active config with its defaults filled in."""
    text = cdsp.send("GetConfig")
    assert "type: Dummy" in text
    # A field that is not in the file on disk, so this is the parsed config and not a
    # copy of the input.
    assert "queuelimit:" in text


def test_get_config_json_returns_the_same_config(cdsp):
    """GetConfigJson should be the same config in the other format."""
    config = json.loads(cdsp.send("GetConfigJson"))
    assert config["devices"]["samplerate"] == 48000
    assert config["devices"]["capture"]["type"] == "Dummy"


def test_set_config_applies_immediately(cdsp):
    """A config uploaded as YAML should become the active one."""
    cdsp.send("SetConfig", titled("from yaml"))
    cdsp.poll_until("GetConfigTitle", "from yaml")
    assert cdsp.send("GetState") == "Running"


def test_set_config_json_applies_immediately(cdsp):
    """And so should one uploaded as JSON."""
    config = json.loads(cdsp.send("GetConfigJson"))
    config["title"] = "from json"
    cdsp.send("SetConfigJson", json.dumps(config))
    cdsp.poll_until("GetConfigTitle", "from json")
    assert cdsp.send("GetState") == "Running"


def test_patch_config_changes_only_what_it_names(cdsp):
    """A patch is a partial config, so everything it leaves out should survive."""
    cdsp.send("PatchConfig", {"title": "patched"})
    cdsp.poll_until("GetConfigTitle", "patched")
    assert cdsp.send("GetConfigValue", "/devices/samplerate") == 48000


def test_set_config_value_by_pointer(cdsp):
    """SetConfigValue should place a value at the JSON Pointer it is given."""
    cdsp.send("SetConfigValue", "set by pointer", pointer="/description")
    cdsp.poll_until("GetConfigDescription", "set by pointer")
    assert cdsp.send("GetConfigValue", "/description") == "set by pointer"


def test_get_config_value_reads_nested_fields(cdsp):
    """A pointer should reach into the pipeline, not just the top level."""
    assert cdsp.send("GetConfigValue", "/filters/testgain/parameters/gain") == -6.0


def test_config_title_and_description_default_to_empty(cdsp):
    """The base config sets neither, so both should come back empty rather than fail."""
    assert cdsp.send("GetConfigTitle") == ""
    assert cdsp.send("GetConfigDescription") == ""


def test_get_config_file_path(start_cdsp, config_file):
    """The path the engine was started with should be readable back."""
    path = config_file()
    cdsp = start_cdsp(config=path)
    assert cdsp.send("GetConfigFilePath") == path


def test_read_config_does_not_change_the_active_one(cdsp):
    """ReadConfig parses and fills defaults, and nothing else."""
    parsed = cdsp.send("ReadConfig", titled("only parsed"))
    assert "only parsed" in parsed
    assert "queuelimit:" in parsed
    assert cdsp.send("GetConfigTitle") == ""


def test_read_config_json(cdsp):
    """The JSON variant should behave the same way."""
    config = json.loads(cdsp.send("GetConfigJson"))
    config["title"] = "only parsed"
    parsed = cdsp.send("ReadConfigJson", json.dumps(config))
    assert "only parsed" in parsed
    assert cdsp.send("GetConfigTitle") == ""


def test_read_config_file(cdsp, config_file):
    """ReadConfigFile should read from disk without touching the active config."""
    parsed = cdsp.send("ReadConfigFile", config_file({"gain: -6.0": "gain: -3.0"}))
    assert "-3.0" in parsed
    assert cdsp.send("GetConfigValue", "/filters/testgain/parameters/gain") == -6.0


def test_read_config_file_that_is_not_there(cdsp, tmp_path):
    """A missing file is a read error, with a message saying so."""
    reply = cdsp.send_raw("ReadConfigFile", str(tmp_path / "nope.yml"))
    assert reply["result"] == "ConfigReadError"
    assert "nope.yml" in reply["message"]


def test_validate_config_accepts_a_good_config(cdsp):
    """A config that parses and makes sense comes back filled in."""
    assert "type: Dummy" in cdsp.send("ValidateConfig", base_config())


def test_validate_config_rejects_a_broken_config(cdsp):
    """A config that does not parse is a read error."""
    reply = cdsp.send_raw("ValidateConfig", "devices:\n  samplerate: 48000\n")
    assert reply["result"] == "ConfigReadError"
    assert "chunksize" in reply["message"]


def test_validate_config_rejects_a_nonsensical_config(cdsp):
    """One that parses but cannot work is a validation error, which is the other path."""
    reply = cdsp.send_raw(
        "ValidateConfig", base_config().replace("names: [testgain]", "names: [nosuchfilter]")
    )
    assert reply["result"] == "ConfigValidationError"
    assert "nosuchfilter" in reply["message"]


def test_validate_config_json(cdsp):
    """The JSON variant should reject the same thing."""
    config = json.loads(cdsp.send("GetConfigJson"))
    config["pipeline"][0]["names"] = ["nosuchfilter"]
    reply = cdsp.send_raw("ValidateConfigJson", json.dumps(config))
    assert reply["result"] == "ConfigValidationError"
    assert "nosuchfilter" in reply["message"]


@pytest.mark.parametrize(
    "bad, result",
    [
        ("devices:\n  samplerate: 48000\n", "ConfigReadError"),
        (None, "ConfigValidationError"),
    ],
    ids=["unparseable", "invalid"],
)
def test_a_rejected_config_leaves_the_running_one_alone(cdsp, bad, result):
    """The engine must not be left half way into a config it refused."""
    cdsp.send("SetConfig", titled("still here"))
    cdsp.poll_until("GetConfigTitle", "still here")

    if bad is None:
        bad = titled("rejected").replace("names: [testgain]", "names: [nosuchfilter]")
    assert cdsp.send_raw("SetConfig", bad)["result"] == result

    assert cdsp.send("GetState") == "Running"
    assert cdsp.send("GetConfigTitle") == "still here"


def test_set_config_file_path_then_reload(start_cdsp, config_file):
    """Pointing at another file should change nothing until the reload."""
    cdsp = start_cdsp(config=config_file())
    other = config_file({"gain: -6.0": "gain: -3.0"})

    cdsp.send("SetConfigFilePath", other)
    assert cdsp.send("GetConfigFilePath") == other
    assert cdsp.send("GetConfigValue", "/filters/testgain/parameters/gain") == -6.0

    cdsp.send("Reload")
    # Polled rather than waited out: a hot reload is applied on the next chunk, so the
    # change lands in a millisecond or two and a fixed sleep would only slow this down.
    cdsp.poll_until_true(
        "GetConfigJson", lambda text: '"gain":-3.0' in text.replace(" ", ""), timeout=5.0
    )


def test_reload_picks_up_an_edit_on_disk(start_cdsp, config_file):
    """Reload rereads the active path, which is the same thing SIGHUP does."""
    path = config_file()
    cdsp = start_cdsp(config=path)
    config_file({'description: "nbr 1"': 'description: "nbr 2"'}, path=path)

    cdsp.send("Reload")
    cdsp.poll_until_true("GetConfigJson", lambda text: "nbr 2" in text, timeout=5.0)
    assert cdsp.send("GetState") == "Running"


def test_previous_config_is_kept_when_processing_stops(start_cdsp):
    """The previous config is recorded when a session ends, not on every hot reload."""
    cdsp = start_cdsp(extra_args=["--wait"])
    assert cdsp.send("GetPreviousConfig").strip() == "null"

    cdsp.send("Stop")
    cdsp.poll_until("GetState", "Inactive")
    # Stop stores the previous config after the pipeline is down, `src/engine.rs:166`,
    # so the state reaches Inactive first and reading it straight away still finds null.
    cdsp.poll_until_true("GetPreviousConfig", lambda text: "type: Dummy" in text)
