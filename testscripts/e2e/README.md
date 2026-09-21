# End-to-end tests

These tests drive the real `camilladsp` binary: each one starts it as a child process with a
config, controls it over the websocket, and shuts it down again. Nothing is mocked.

They run on the test-only dummy capture and playback devices, which move audio at a paced rate
without touching any hardware. That needs a build with the `dummy-backend` feature.

## Why Python and not Rust

The obvious alternative is a Rust suite in `tests/`, and it was considered. It would need no new
dependencies, since `tungstenite`, `serde_json`, `yaml_serde`, `waveadapter` and `realfft` are all
in `[dependencies]` and therefore visible to `tests/` targets, and it would ride the existing
feature matrix for free. Neither advantage outweighed the extra code to maintain.

What these tests actually do is start a process, send it JSON, and compare numbers. The websocket
protocol is request/reply JSON over text frames, see `websocket.md`, so a client covering every
one of the ~80 commands is a handful of lines, and the whole of `wsclient.py`, subscriptions
included, is 150. `parametrize` turns a sweep over sample rates or fader indices into one
decorator. Nothing has to be recompiled to change a test.

None of that makes Python better than Rust in general, it makes it cheaper here, and cheap is what
decides whether the next test gets written. The split is the point: `cargo test` covers the DSP,
this suite covers the binary.

The client is deliberately not pyCamillaDSP either. That library is released separately and would
lag, so a new websocket command would be untestable here until the client shipped support for it,
and a pinned but lagging client would silently narrow what these tests cover. `wsclient.py`
forwards whatever command name it is handed, so it cannot lag.

## Running them

```sh
cargo build --features dummy-backend
pip install pytest pytest-timeout websocket-client
pytest -v testscripts/e2e
```

The tests look for `target/debug/camilladsp` by default. Set `CAMILLADSP_BIN` to test a different
build, for example a release one.

## Layout

- `conftest.py` — the fixtures that spawn the binary, wait for its websocket, and tear it down
- `wsclient.py` — a small raw JSON websocket client, deliberately not pyCamillaDSP
- `dummyctl.py` — a client for the dummy devices' control socket, which is the fake hardware
- `test_dummy_smoke.py` — the dummy devices themselves: pacing, rates, chunk sizes, channels
- `test_lifecycle.py` — startup, shutdown, exit codes and signals
- `test_config_commands.py` — reading, writing, patching and validating configs
- `test_config_churn.py` — the ported reload suite: rapid config changes over every route
- `test_volume.py` — volume, mute and the five faders, including their effect on the audio
- `test_signal_levels.py` — the level getters, against computed values
- `test_statefile.py` — what survives a restart
- `test_errors.py` — malformed input and the other unhappy paths
- `test_spectrum.py` — `GetSpectrum` and `SubscribeSpectrum` against a known tone
- `test_subscriptions.py` — the pushed level, VU and state event streams
- `test_dummy_control.py` — the dummy devices' control socket: protocol, counters, lifetime
- `*.yml` — the configs the tests load
- `pytest.ini` — the global timeout, which makes every test a hang check as well

## Writing more

Two fixtures do most of the work. `start_cdsp` spawns the binary on a free port and hands back a
handle that can `send` commands and `poll_until` a getter changes. `config_file` writes an edited
copy of one of the configs above, which is how a test gets a variant without another near
duplicate `.yml` landing here.

A subscribed connection accepts nothing but `StopSubscription`, so a test that watches a stream
and drives the engine at the same time needs two connections. `cdsp.new_client()` opens the second
one, and the subscription goes on that so `cdsp.send` keeps working. On the client side,
`recv_events` reads pushed events with the time each arrived, and `stop_subscription` ends the
stream, skipping past any events still in flight ahead of the reply.

A dummy device can be made to misbehave while it is running, through a loopback TCP socket of
its own, see `src/dummy_backend/control.rs`. A device gets one when its config block carries a
`control_port`, and `control_cdsp` is the fixture that starts a pair of them on free ports and
hands back a `capture_control` and a `playback_control`. The protocol is one line in and one line
out, `stall:1` to write and `stall` to read, and the counters `frames`, `pauses` and `resyncs`
are readable keys like any other. The listener is owned by the device, so it dies on a config
reload and nothing carries over between tests.

Poll, do not sleep. The status snapshot only refreshes once per update interval, so every status
getter reads back as zero for the first fraction of a second after the devices start, and a fixed
wait long enough to cover that on a loaded CI runner would slow the whole suite down.
