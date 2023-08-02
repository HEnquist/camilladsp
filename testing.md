# Testing

## Built-in tests
Some of the functionality is covered by tests implemented in Rust.
These tests are run via cargo:
```sh
cargo test
```

## Config update tests
A set of tests for testing that changing the running configuration works correctly
is implemeted as a Python test script.
This requires the Python packages `pytest` and `pycamilladsp` to run.

Some tests trigger a config reload by sending SIGHUP to the camilladsp process.
This is not available on Windows.
It uses the `pgrep` command for getting the PID of the running camilladsp process,
and it assumes that only one camilladsp instance is running.

To run the tests, prepare four valid config files, named "conf1.yml" to "conf4.yml".
Example files are available in `testscripts/config_load_test`.
Place the new config files in that folder.

Start camilladsp in wait mode, with the websocket server listening on port 1234:
```sh
camilladsp -w -v -p1234
```

Now start the tests:
```sh
cd testscripts/config_load_test
pytest -v
```

A complete run takes a couple of minutes.

# Benchmarks

There are benchmarks to monitor the performance of some filters.
These use the `criterion` framework.
Run them with cargo:
```sh
cargo bench
```
