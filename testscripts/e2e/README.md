# End-to-end tests

These tests drive the real `camilladsp` binary: each one starts it as a child process with a
config, controls it over the websocket, and shuts it down again. Nothing is mocked.

They run on the test-only dummy capture and playback devices, which move audio at a paced rate
without touching any hardware. That needs a build with the `dummy-backend` feature.

## Running them

```sh
cargo build --features dummy-backend
pip install pytest websocket-client
pytest -v testscripts/e2e
```

The tests look for `target/debug/camilladsp` by default. Set `CAMILLADSP_BIN` to test a different
build, for example a release one.

## Layout

- `conftest.py` — the fixtures that spawn the binary, wait for its websocket, and tear it down
- `wsclient.py` — a small raw JSON websocket client, deliberately not pyCamillaDSP
- `test_dummy_smoke.py` — the dummy devices themselves: pacing, rates, chunk sizes, channels
- `test_lifecycle.py` — startup, shutdown, exit codes and signals
- `test_config_commands.py` — reading, writing, patching and validating configs
- `test_volume.py` — volume, mute and the five faders, including their effect on the audio
- `test_signal_levels.py` — the level getters, against computed values
- `test_statefile.py` — what survives a restart
- `test_errors.py` — malformed input and the other unhappy paths
- `*.yml` — the configs the tests load

## Writing more

Two fixtures do most of the work. `start_cdsp` spawns the binary on a free port and hands back a
handle that can `send` commands and `poll_until` a getter changes. `config_file` writes an edited
copy of one of the configs above, which is how a test gets a variant without another near
duplicate `.yml` landing here.

Poll, do not sleep. The status snapshot only refreshes once per update interval, so every status
getter reads back as zero for the first fraction of a second after the devices start, and a fixed
wait long enough to cover that on a loaded CI runner would slow the whole suite down.
