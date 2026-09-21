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
from websocket import WebSocketException

from dummyctl import Control
from wsclient import Client

# The configs live next to the tests. A `configs/` subdirectory would be nicer, but the
# repo .gitignore excludes that name everywhere.
HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))

# The control_port values in dummy_control.yml, which every test replaces with a free one.
CAPTURE_PORT_PLACEHOLDER = "control_port: 11111"
PLAYBACK_PORT_PLACEHOLDER = "control_port: 22222"

# How long to wait for the websocket server to accept a connection after the spawn.
STARTUP_TIMEOUT = 20.0
# How long to wait for the process to be gone after asking it to exit.
SHUTDOWN_TIMEOUT = 10.0


# Where a dummy-backend build can end up, most appropriate first. The suite wants the e2e
# profile, see Cargo.toml for why it is optimised, but a debug build is what someone
# iterating on the Rust side will have to hand.
BUILD_PROFILES = ("e2e", "release-fast", "release", "debug")


def default_binary():
    """The most recently built binary among the profiles, or the e2e one if there is none.

    Whichever was built last is the one the caller just built, so both
    `cargo build --profile e2e --features dummy-backend` and a plain debug build lead to
    that build being tested rather than to a stale one from the other profile. Set
    CAMILLADSP_BIN to override the choice entirely.
    """
    name = "camilladsp.exe" if sys.platform == "win32" else "camilladsp"
    paths = [os.path.join(REPO_ROOT, "target", profile, name) for profile in BUILD_PROFILES]
    built = [path for path in paths if os.path.isfile(path)]
    return max(built, key=os.path.getmtime) if built else paths[0]


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
        self.extra_clients = []

    def new_client(self):
        """Open a second connection to the same process.

        A subscribed connection accepts nothing but StopSubscription, so a test that
        wants to both watch a stream and drive the engine needs two. Subscribing on this
        one rather than on `self.client` also keeps the teardown's Exit working.
        """
        client = Client(self.port)
        self.extra_clients.append(client)
        return client

    def send(self, command, value=None, **fields):
        return self.client.send(command, value, **fields)

    def send_raw(self, command, value=None, **fields):
        return self.client.send_raw(command, value, **fields)

    def send_text(self, text):
        return self.client.send_text(text)

    def poll_until(self, command, expected, timeout=10.0, interval=0.02):
        return self.client.poll_until(command, expected, timeout=timeout, interval=interval)

    def poll_until_true(self, command, predicate, timeout=10.0, interval=0.02):
        return self.client.poll_until_true(
            command, predicate, timeout=timeout, interval=interval
        )

    def is_running(self):
        return self.process.poll() is None

    def exit(self, timeout=SHUTDOWN_TIMEOUT):
        """Ask CamillaDSP to exit, and return the exit code it left behind.

        The reply to Exit can be lost rather than read. The process closes the socket on
        its way out, and on Windows that arrives as a reset which discards anything
        still in the receive buffer. The exit code is what this asserts on, so a dead
        connection here is an expected outcome and not a failure. A command error still
        propagates, since that means the process was in no state to be asked.
        """
        try:
            self.send("Exit")
        except (OSError, WebSocketException):
            pass
        return self.process.wait(timeout=timeout)


@pytest.fixture(scope="session")
def camilladsp_bin():
    """Path to the binary under test, overridable with CAMILLADSP_BIN."""
    path = os.environ.get("CAMILLADSP_BIN") or default_binary()
    if not os.path.isfile(path):
        pytest.fail(
            f"CamillaDSP binary not found at '{path}'. Build it with "
            "`cargo build --profile e2e --features dummy-backend`, or point "
            "CAMILLADSP_BIN at one."
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
def control_cdsp(start_cdsp, config_file):
    """Factory for a CamillaDSP whose dummy devices have a control socket each.

    The ports in the config are placeholders, replaced here with ports the OS says are
    free so concurrent runs do not collide. The handle gets a `capture_control` and a
    `playback_control`, see dummyctl.py.
    """

    def _start(replacements=None, base="dummy_control.yml", **kwargs):
        ports = {"capture": free_port(), "playback": free_port()}
        edits = {
            CAPTURE_PORT_PLACEHOLDER: f"control_port: {ports['capture']}",
            PLAYBACK_PORT_PLACEHOLDER: f"control_port: {ports['playback']}",
        }
        edits.update(replacements or {})
        cdsp = start_cdsp(config=config_file(edits, base=base), **kwargs)
        cdsp.capture_control = Control(ports["capture"])
        cdsp.playback_control = Control(ports["playback"])
        # A test that builds a second config has to keep the same ports, or the change
        # restarts the devices and the sockets move out from under it.
        cdsp.control_edits = edits
        return cdsp

    return _start


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
    for client in [cdsp.client, *cdsp.extra_clients]:
        try:
            client.close()
        except Exception:
            pass
