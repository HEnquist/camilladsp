# End-to-end tests

These tests drive the real `camilladsp` binary: each one starts it as a child process with a
config, controls it over the websocket, and shuts it down again. Nothing is mocked.

Most of them run on the test-only dummy capture and playback devices, which move audio at a paced
rate without touching any hardware. That needs a build with the `dummy-backend` feature. The rest
carry the `stock` marker and run against a build without it, which is the binary a release ships:
they cover the file, wav, stdin, stdout and generator devices, and they are the only tests here
that say anything about the default build. See "Two builds" below.

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
pip install pytest pytest-timeout websocket-client numpy
pytest -v testscripts/e2e -m "not stock"
```

The `e2e` profile is optimised, and that matters more than it sounds: an unoptimised build spends
over half a chunk period resampling, and a device that cannot keep up produces underruns and missed
deadlines that are facts about the build rather than about the code. It keeps the debug assertions
and overflow checks, which cost a few percent on a binary that is otherwise idle. See Cargo.toml.

The tests find the most recently built binary among the `e2e`, `release-fast`, `release` and `debug`
profiles, so a plain `cargo build --features dummy-backend` while iterating on the Rust side is
tested rather than a stale optimised one. `CAMILLADSP_BIN` overrides the choice.

## Two builds

The `stock` marked tests need the other build, so they are a second command and, in CI, a second
job:

```sh
cargo build --profile e2e
pytest -v testscripts/e2e -m stock
```

Both builds land on the same path, so whichever was built last is the one the suite finds, and
running the whole suite in one go against one binary is not a thing to do: `-m "not stock"` needs
the feature and `-m stock` asserts it is absent. The split is what the two jobs in
`.github/workflows/e2e_software.yml` do. The stock job runs on Linux and Windows rather than all
three, because the file backend has two reader implementations, `NonBlockingReader` on Linux and
`BlockingReader` everywhere else, see `src/file_backend/mod.rs`, and one runner covers one of them.

Anything marked `stock` has to keep away from the dummy devices, since a stock build rejects their
config outright. `device_config` swaps them in when asked, so a stock test builds its own blocks
through `swdevices.py` instead.

## ALSA on real devices

The tests in `alsa/` carry the `alsa` marker and run the ALSA backend against kernel devices:
snd-aloop for a capture the test can feed and a playback it can record, and snd-dummy for a sink
with nothing behind it. They are the only tests that reach the ALSA code at all. `alsa/loopback.py`
explains which cable is which and the three loopback properties the tests are built on.

The GitHub runners' kernel is built without sound, so in CI the `alsa` job in
`.github/workflows/e2e_alsa.yml` boots an Ubuntu minimal cloud image under KVM, copies the binary
and this directory in, and runs the suite there, once for each ALSA backend. On a Linux machine of
your own it is only the modules, with your user in the `audio` group:

```sh
sudo modprobe snd-aloop pcm_substreams=2
sudo modprobe snd-dummy
cargo build --profile e2e              # or with --features threaded-alsa
pytest -v testscripts/e2e -m alsa
```

Anywhere the two cards are missing the tests skip. They do not care which build they get, since
both have the ALSA backend, but the other markers do, so run them as their own selection.

Rate adjust is tested against a loopback cable whose `PCM Rate Shift 100000` control the test
skews, not against snd-dummy, which has no control over its clock. The adjust loop then has to
bring the capture cable's own shift to the same value, and the tests read that back from the
card. In a VM on a shared runner the scheduling is jittery and the loop answers jitter the way
it answers drift, so those tests judge a median of readings and never a single one.

## CoreAudio on BlackHole

The tests in `coreaudio/` carry the `coreaudio` marker and run the CoreAudio backend against two
BlackHole devices: BlackHole 2ch for a capture the test can feed, and BlackHole 16ch for a playback
it can record. They are the only tests that reach the CoreAudio code at all.
`coreaudio/blackhole.py` explains which device is which and the four BlackHole properties the
tests are built on.

BlackHole is a HAL plugin, so the casks install on a stock runner. The installer asks for a reboot,
and what it actually needs is a restart of coreaudiod, which the `coreaudio` job in
`.github/workflows/e2e_coreaudio.yml` does before the tests. On a Mac of your own it is the same two
casks, plus sounddevice for the feeder and the recorder:

```sh
brew install --cask blackhole-2ch blackhole-16ch
sudo killall coreaudiod
cargo build --profile e2e
pip install sounddevice
pytest -v testscripts/e2e -m coreaudio
```

Anywhere the two devices are missing the tests skip. They change the devices' sample rate, clock
source and pitch, which the devices keep after the process that set them is gone, so a fixture
puts all three back as a fresh install has them around every test. That includes a BlackHole 2ch
you use for something else.

Rate adjust is tested against BlackHole 16ch put on its adjustable clock at a pitch the test sets.
The adjust loop then has to bring BlackHole 2ch's pitch to the same value, and the tests read that
back from the device, the same shape as the ALSA rate shift tests.

Setting those properties from Python has two traps, both handled in `blackhole.py`. A pan written
just after switching to the adjustable clock is accepted and then lost, so it is written until it
reads back. And a sample rate change has the HAL restore the clock source and pan it last saw,
which can land just after the new rate reads back and undo a write made in between.

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
- `test_failures.py` — device failures, sample rate changes, and the end of a stream
- `test_file_devices.py` — the file devices, and a paced end of the pipeline meeting a free one
- `test_teardown.py` — stopping a run in the states where stopping is hard
- `test_processing.py` — a mixer and a Biquad, asserted through the audio
- `swdevices.py` — the software devices' config blocks and sample format encoders, for the below
- `test_raw_files.py` — the raw file devices, every sample format, bit exact
- `test_wav_files.py` — the wav header, RF64, and a wav round trip
- `test_stdio.py` — the Stdin and Stdout devices, byte exact
- `test_generator.py` — the SignalGenerator device and its three waveforms
- `test_file_resampling.py` — resampling between rates with both ends free running
- `test_exact_processing.py` — a gain and a mixer, asserted sample by sample
- `test_stock_build.py` — what a release build has, and what it must refuse
- `*.yml` — the configs the tests load
- `alsa/` — the ALSA suite, see "ALSA on real devices" above
- `coreaudio/` — the CoreAudio suite, see "CoreAudio on BlackHole" above
- `pytest.ini` — the global timeout, which makes every test a hang check, and the four markers

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
are readable keys like any other. `error` makes the device report a failure, `rate` makes it
switch sample rate, and `eof` ends the capture's stream. The listener is owned by the device, so
it dies on a config reload and nothing carries over between tests, which is also what makes
`error` a one shot: the session that comes back is built on control state that starts clear.

A test that needs a pipe on stdin or stdout passes `pipe_stdin` or `pipe_stdout` to `spawn_cdsp`
or `start_cdsp`. stderr stays inherited either way, so the CamillaDSP log still reaches pytest on a
failure. A piped stdout has to be read: the free running devices produce hundreds of megabytes a
second, the pipe holds 64 kB, and a playback device blocked inside a write never gets to exit.
`cdsp.exit()` drains it while it waits, and `read_exactly` in `swdevices.py` is how a test takes a
bounded amount of audio out of a device that never ends.

Measuring a rate from a counter needs more care than timing a sleep. A counter here is read over
a socket, and the dummy control socket opens a connection per command, so the value is sampled
somewhere inside a round trip rather than when the test asked. A reading taken promptly and one
taken after the machine was away for 200 ms produce the same numbers with different answers, and
on a busy runner the trip alone can be a few percent of a short window. Prefer asserting on
something the engine measures over its own window, such as `GetCaptureRate`, and poll it rather
than reading it once.

The control socket's own latency used to be the largest term in that. Its accept loop is
non-blocking and sleeps `POLL_INTERVAL` between tries, so a client arriving just after a sleep
starts waits out the rest of it: at 50 ms that was a median of 56 ms on every counter read, which
is 4 % of a 1.5 s measurement window before the machine does anything at all. It is 2 ms now, and
a read is about 3 ms. Worth knowing before adding a test that reads a counter in a loop.

Tests that assert on timing accuracy, rather than merely taking time, carry the `pacing` marker,
and CI runs them on Linux only. The rate control loop and the buffer accounting are portable code
with no `cfg` in them, so a second and third runner would only be measuring their own schedulers.
Everything unmarked runs on all three, and `test_dummy_smoke.py` checks the pacer against the
clock everywhere. Where a test makes two claims and only one of them is about the machine keeping
up, split it rather than marking the pair: the level stream's cadence is two tests for that
reason, a lower bound that holds anywhere and an upper bound that does not. Run them locally with
`pytest testscripts/e2e -m "not stock"`, which selects every dummy test including the paced ones,
since `-m "not pacing"` is only what CI passes on the other two runners.

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
