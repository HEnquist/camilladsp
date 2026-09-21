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
cargo build --profile e2e --features dummy-backend
pip install pytest pytest-timeout websocket-client
pytest -v testscripts/e2e
```

The `e2e` profile is optimised, and that matters more than it sounds: an unoptimised build spends
over half a chunk period resampling, and a device that cannot keep up produces underruns and missed
deadlines that are facts about the build rather than about the code. It keeps the debug assertions
and overflow checks, which cost a few percent on a binary that is otherwise idle. See Cargo.toml.

The tests find the most recently built binary among the `e2e`, `release-fast`, `release` and `debug`
profiles, so a plain `cargo build --features dummy-backend` while iterating on the Rust side is
tested rather than a stale optimised one. `CAMILLADSP_BIN` overrides the choice.

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
- `test_dummy_states.py` — stalled and paused processing, driven from that socket
- `test_rate_control.py` — the PI controller, the buffer level, and clocks that disagree
- `test_resampling.py` — the capture side resampler: the types, the load, the rates
- `test_clipping.py` — the playback's sample format conversion, which is where clipping happens
- `*.yml` — the configs the tests load
- `pytest.ini` — the global timeout, which makes every test a hang check, and the `pacing` marker

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

Tests that assert on timing accuracy, rather than merely taking time, carry the `pacing` marker,
and CI runs them on Linux only. The rate control loop and the buffer accounting are portable code
with no `cfg` in them, so a second and third runner would only be measuring their own schedulers.
Everything unmarked runs on all three, and `test_dummy_smoke.py` checks the pacer against the
clock everywhere. Where a test makes two claims and only one of them is about the machine keeping
up, split it rather than marking the pair: the level stream's cadence is two tests for that
reason, a lower bound that holds anywhere and an upper bound that does not. Run them locally with
`pytest testscripts/e2e`, which selects everything, since `-m "not pacing"` is only what CI passes
on the other two.

Two things to know before writing more rate control tests. The buffer level is sampled as a chunk
arrives and drains by a chunk before the next one does, so single readings are points on a sawtooth
and the ones in between differ by a whole chunk: average a handful of them, as `average_level` does.
And a short `adjust_interval_s` converges faster, not slower, since the controller works on the
error relative to the frames in one interval, so at the 10 s default a buffer error of a chunk or
two is a rounding error. The tests run at 0.2 s.

A dummy device can also be given the two things a real one does to the audio itself. The capture
resamples when `capture_samplerate` differs from `samplerate`, and the playback converts each chunk
to a `format` when it is given one, rather than dropping the audio as it arrives. Those are what
reach `GetResamplerLoad`, the resampler selection, the capture side of rate adjust and
`GetClippedSamples`, all of which read as zero on a config without them. `dummy_resample.yml` uses
the cheap AsyncPoly resampler on purpose: it is a tenth of a percent of a chunk period against a few
percent for AsyncSinc, which leaves the timing assertions all the headroom there is to have on a
runner shared with whatever else is on it.

Poll, do not sleep. The status snapshot only refreshes once per update interval, so every status
getter reads back as zero for the first fraction of a second after the devices start, and a fixed
wait long enough to cover that on a loaded CI runner would slow the whole suite down.
