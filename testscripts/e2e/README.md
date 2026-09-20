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
- `*.yml` — the configs the tests load
