"""Fixtures for the CamillaDSP end-to-end tests.

Every test here drives the real binary: it is started as a child process with a config,
controlled over the websocket, and shut down again. Nothing is mocked.
"""

import os
import socket
import subprocess
import sys
import time

import pytest

from wsclient import Client

# The configs live next to the tests, as in testscripts/config_load_test. A `configs/`
# subdirectory would be nicer, but the repo .gitignore excludes that name everywhere.
HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

# How long to wait for the websocket server to accept a connection after the spawn.
STARTUP_TIMEOUT = 20.0
# How long to wait for the process to be gone after asking it to exit.
SHUTDOWN_TIMEOUT = 10.0


def default_binary():
    """The debug binary a plain `cargo build --features dummy-backend` produces."""
    name = "camilladsp.exe" if sys.platform == "win32" else "camilladsp"
    return os.path.join(REPO_ROOT, "target", "debug", name)


def free_port():
    """Grab a port the OS says is free, so concurrent runs do not collide."""
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class CamillaDsp:
    """A running CamillaDSP process and a websocket connection to it."""

    def __init__(self, process, client, port):
        self.process = process
        self.client = client
        self.port = port
        self.pid = process.pid

    def send(self, command, value=None, **fields):
        return self.client.send(command, value, **fields)

    def send_raw(self, command, value=None, **fields):
        return self.client.send_raw(command, value, **fields)

    def send_text(self, text):
        return self.client.send_text(text)

    def poll_until(self, command, expected, timeout=10.0):
        return self.client.poll_until(command, expected, timeout=timeout)

    def poll_until_true(self, command, predicate, timeout=10.0):
        return self.client.poll_until_true(command, predicate, timeout=timeout)

    def is_running(self):
        return self.process.poll() is None

    def exit(self, timeout=SHUTDOWN_TIMEOUT):
        """Ask CamillaDSP to exit, and return the exit code it left behind."""
        self.send("Exit")
        return self.process.wait(timeout=timeout)


@pytest.fixture(scope="session")
def camilladsp_bin():
    """Path to the binary under test, overridable with CAMILLADSP_BIN."""
    path = os.environ.get("CAMILLADSP_BIN") or default_binary()
    if not os.path.isfile(path):
        pytest.fail(
            f"CamillaDSP binary not found at '{path}'. Build it with "
            "`cargo build --features dummy-backend`, or point CAMILLADSP_BIN at it."
        )
    return path


@pytest.fixture
def spawn_cdsp(camilladsp_bin):
    """Factory that starts CamillaDSP without waiting for anything.

    Only needed by tests where the process is not expected to come up at all, such as
    the bad config cases. Everything else wants start_cdsp.
    """
    started = []

    def _spawn(config="dummy_sine.yml", extra_args=()):
        port = free_port()
        args = [camilladsp_bin, "-p", str(port), *extra_args]
        if config is not None:
            # A config given as an absolute path is used as-is, which is what the tests
            # that build a config in tmp_path rely on.
            args.append(os.path.join(HERE, config))
        # stdout and stderr are inherited, so pytest captures the log and prints it
        # when a test fails.
        process = subprocess.Popen(args)
        started.append(process)
        return process, port

    yield _spawn

    for process in started:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=SHUTDOWN_TIMEOUT)


@pytest.fixture
def start_cdsp(spawn_cdsp):
    """Factory that starts CamillaDSP and cleans it up when the test is done."""
    started = []

    def _start(config="dummy_sine.yml", extra_args=(), wait_for_running=True):
        process, port = spawn_cdsp(config, extra_args)
        client = _connect(process, port)
        cdsp = CamillaDsp(process, client, port)
        started.append(cdsp)
        if wait_for_running:
            cdsp.poll_until("GetState", "Running")
        return cdsp

    yield _start

    for cdsp in started:
        _teardown(cdsp)


@pytest.fixture
def config_file(tmp_path):
    """Factory that writes an edited copy of a test config and returns its path.

    Tests that need a config variant build it from one of the checked in ones rather
    than carrying a near duplicate file, so a change to the base config cannot leave a
    variant behind. The replacements are asserted, so a typo fails loudly instead of
    silently testing the unedited config.
    """
    count = [0]

    def _write(replacements=None, base="dummy_sine.yml", path=None):
        """Write an edited copy of `base`, to `path` if given, and return the path.

        Passing a path that was handed out earlier rewrites that file in place, which is
        what the tests that edit the config on disk under a running engine need.
        """
        with open(os.path.join(HERE, base)) as conf:
            text = conf.read()
        for old, new in (replacements or {}).items():
            assert old in text, f"'{old}' is not in {base}"
            text = text.replace(old, new)
        if path is None:
            count[0] += 1
            path = str(tmp_path / f"config{count[0]}.yml")
        with open(path, "w") as conf:
            conf.write(text)
        return str(path)

    return _write


@pytest.fixture
def cdsp(start_cdsp):
    """A CamillaDSP running the dummy to dummy config, already in the Running state."""
    return start_cdsp()


def _connect(process, port):
    """Wait for the websocket server to come up, then connect to it."""
    deadline = time.monotonic() + STARTUP_TIMEOUT
    last_error = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"CamillaDSP exited during startup with code {process.returncode}")
        try:
            return Client(port)
        except (ConnectionRefusedError, OSError) as err:
            last_error = err
            time.sleep(0.05)
    process.kill()
    raise TimeoutError(f"No websocket on port {port} after {STARTUP_TIMEOUT} s: {last_error}")


def _teardown(cdsp):
    """Shut down cleanly if the test has not already, and never leave a process behind."""
    if cdsp.is_running():
        try:
            cdsp.exit()
        except Exception:
            cdsp.process.kill()
            cdsp.process.wait(timeout=SHUTDOWN_TIMEOUT)
    try:
        cdsp.client.close()
    except Exception:
        pass
