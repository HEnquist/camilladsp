# Testing

## Built-in tests
Some of the functionality is covered by tests implemented in Rust.
These tests are run via cargo:
```sh
cargo test
```

## End-to-end tests
A pytest suite drives the real binary.
Each test starts `camilladsp` as a child process with a config, controls it over the websocket,
and shuts it down again.
It covers the lifecycle and exit codes, the config command surface including rapid config churn,
volume, mute and the faders, the signal level getters, the spectrum analysis, the pushed event
subscriptions, the state file, the error paths, the stalled and paused states, the rate
control loop, the capture side resampler, the clipping that comes with converting to a
sample format, the stop reasons left behind by a device failure, a sample rate change or
a stream that ends, the processing itself through a mixer and a filter, and the file devices
in every combination of a paced and a free-running end.

Most of the tests run on the test-only dummy capture and playback devices,
so they need a build with the `dummy-backend` feature.
They also need the Python packages `pytest`, `pytest-timeout`, `websocket-client`
and `numpy`.

```sh
cargo build --profile e2e --features dummy-backend
pip install pytest pytest-timeout websocket-client numpy
pytest -v testscripts/e2e -m "not stock"
```

A complete run takes a couple of minutes.

The rest carry the `stock` marker and run against a build without the feature, which is the
binary a release ships.
They cover the raw file, wav, stdin, stdout and generator devices in every sample format,
the wav header and its RF64 variant, resampling with both ends of the pipeline free running,
and the processing asserted sample by sample rather than through a meter.
They also check that a stock build has no dummy devices and rejects a config asking for one.

```sh
cargo build --profile e2e
pytest -v testscripts/e2e -m stock
```

That run takes a couple of seconds, since nothing in it is paced.
Both builds land on the same path, so the two selections are two commands and not one.
The `e2e` profile is optimised because the devices are paced in real time and an unoptimised
build cannot keep up with itself, see Cargo.toml.
The tests use the most recently built binary among the `e2e`, `release-fast`, `release` and
`debug` profiles, and `CAMILLADSP_BIN` overrides that.
See `testscripts/e2e/README.md` for the layout, for why the suite is in Python rather than
Rust, and for what to know before adding tests.

## Running the tests on GitHub Actions
A complete test run covers Linux, Windows and macOS, on several CPU architectures.
Each backend suite in `testscripts/e2e` can only run on its own platform:
ALSA and PipeWire on Linux, CoreAudio on macOS, and WASAPI and ASIO on Windows.
Running everything locally would therefore take several computers.

The GitHub Actions workflows of the repository run all of it on GitHub's runners,
including the setup of the sound devices each backend suite needs.
Each end-to-end workflow only runs when the change touches its backend or the shared code.

The workflows run in a fork too, so they can be used before a pull request is opened.
GitHub disables the workflows of a new fork, so you first need to enable them in the Actions tab
of the fork.
Pushing a branch to the fork then starts them.
A convenient way to see all the results together is to open a pull request inside your own fork,
from your branch to the fork's copy of the branch you are targeting.
This runs the same checks as a pull request to the main repository.
Once everything passes, close it and open the real pull request.
The end-to-end workflows can also be started by hand with "Run workflow" in the Actions tab.

## Benchmarks

The benchmarks in `benches/` use the `criterion` framework.

| Benchmark | What it measures |
| --- | --- |
| `filters` | Single filters: convolution at several lengths, biquad and DiffEq. |
| `conversions` | Converting between raw sample bytes and audio chunks. |
| `conv_parallel` | How a convolution filter scales over channels, serial and on the thread pool, with shared or separate coefficients. |
| `biquad_canon` | The interleaved biquad kernel, for choosing how many channels and stages to run side by side. |
| `pipeline` | One chunk through complete pipelines of different shapes. |
| `pipeline_build` | Building a pipeline, and updating the filter parameters of an existing one, as happens on a config reload. |
| `fftconv_kernels` | The complex multiply kernels of the FFT convolution, scalar against NEON on `aarch64` and AVX+FMA on `x86_64`. |

Run all of them, or a single one, with cargo:
```sh
cargo bench
cargo bench --bench pipeline
```

The `fftconv_kernels` benchmark needs the `bench` feature, which exposes the kernel functions,
and a plain `cargo bench` skips it:
```sh
cargo bench --features bench --bench fftconv_kernels
```

### Getting stable results

This is general benchmarking advice, nothing in it is specific to CamillaDSP.

Results are only comparable between runs made under the same conditions.
On a laptop, running on battery or letting the machine sleep in the middle of a run changes the
numbers by more than most optimisations do.
These steps make the results more stable:

- Prefer a stationary computer over a laptop when there is a choice.
  A laptop is limited by its cooling, so it lowers the clock frequency as it heats up
  during a long run, while a stationary computer can usually keep its clock steady.
- Keep a laptop on mains power for every run, and keep power saving modes off.
- Keep the machine from sleeping during a long run.
  On macOS, run the benchmark under `caffeinate -i`.
- Disable turbo boost, since the clock frequency it gives depends on temperature and on what
  else is running.
  On Linux with an Intel CPU, write `1` to `/sys/devices/system/cpu/intel_pstate/no_turbo`.
  With other CPUs, write `0` to `/sys/devices/system/cpu/cpufreq/boost`.
  On Windows, setting the maximum processor state of the power plan to 99 % disables it.
  Apple silicon has no setting for it.
- On Linux, set the CPU frequency governor to `performance`, for example with
  `cpupower frequency-set -g performance`.
- Close other programs that use the CPU, such as browsers and music players.
- Let criterion do the comparison.
  Save a baseline before the change with `cargo bench -- --save-baseline before`,
  and compare against it after the change with `cargo bench -- --baseline before`.
- Repeat the comparison in the order before, after, before.
  If the second "before" run differs from the first as much as "after" does, the difference is
  noise.
