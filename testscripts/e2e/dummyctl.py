"""A client for the dummy devices' test control socket.

The websocket controls CamillaDSP. This controls the fake hardware under it: it makes a
dummy device stall, go silent, or run its clock off nominal while the engine is running,
and reads back the frame, pause and resync counters. The protocol is one line in and one
line out, see `src/dummy_backend/control.rs`.

A connection is opened per command and closed again, so a test that leaves one dangling
cannot block the device's listener, which serves one connection at a time.

`SimulatedSource` is the other direction: a server the dummy capture asks what its source
is doing, for the controller tests that follow a source.
"""

import socket
import threading
import time

# How long to wait for a reply before giving up. The device answers from a thread of its
# own, so nothing it does can make this slow, but a stalled test should not hang here.
TIMEOUT = 5.0

COUNTERS = ("frames", "pauses", "resyncs")


class Control:
    """The control socket of one dummy device."""

    def __init__(self, port, host="127.0.0.1"):
        self.port = port
        self.host = host

    def raw(self, line):
        """Send one line and return the reply, whatever it says."""
        with socket.create_connection((self.host, self.port), timeout=TIMEOUT) as conn:
            conn.sendall(f"{line}\n".encode())
            reply = b""
            while not reply.endswith(b"\n"):
                chunk = conn.recv(256)
                if not chunk:
                    raise ConnectionError(f"Control socket closed while reading a reply to '{line}'")
                reply += chunk
        return reply.decode().strip()

    def get(self, key):
        """Read a key, and return its value as a string."""
        reply = self.raw(key)
        prefix = f"{key}="
        assert reply.startswith(prefix), f"Reading '{key}' gave '{reply}'"
        return reply[len(prefix) :]

    def get_int(self, key):
        return int(self.get(key))

    def set(self, key, value):
        """Write a key, and fail unless it was accepted."""
        reply = self.raw(f"{key}:{value}")
        assert reply == "ok", f"Setting '{key}' to '{value}' gave '{reply}'"

    def counters(self):
        """All three read-only counters, as a dict."""
        return {key: self.get_int(key) for key in COUNTERS}

    def wait_until_ready(self, timeout=10.0, interval=0.05):
        """Wait for the listener to answer, and return how long that took.

        The listener is owned by the device, so it goes away on a config reload and comes
        back with the new one, and it retries the bind while the old one releases the port.
        """
        deadline = time.monotonic() + timeout
        started = time.monotonic()
        last_error = None
        while time.monotonic() < deadline:
            try:
                self.raw("frames")
                return time.monotonic() - started
            except OSError as err:
                last_error = err
                time.sleep(interval)
        raise TimeoutError(
            f"No dummy control socket on port {self.port} after {timeout} s: {last_error}"
        )


class SimulatedSource:
    """What feeds a dummy capture, the stand-in for a player on a loopback.

    The source has to outlive the sessions, since the controller asks it at startup and
    while nothing runs, so the test runs it rather than the device. The dummy connects,
    sends `state`, and gets one line back, see `src/dummy_backend/source.rs`. While
    following, the capture stops for a source at another rate or channel count, and
    captures silence from an inactive one.
    """

    def __init__(self, state="inactive"):
        self.state = state
        self._server = socket.create_server(("127.0.0.1", 0))
        self._server.settimeout(0.05)
        self.port = self._server.getsockname()[1]
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        while not self._stop.is_set():
            try:
                conn, _ = self._server.accept()
            except (TimeoutError, OSError):
                continue
            with conn:
                try:
                    conn.settimeout(TIMEOUT)
                    conn.recv(64)
                    conn.sendall(f"{self.state}\n".encode())
                except OSError:
                    pass

    def play(self, rate, channels=2, fmt="S32_LE"):
        self.state = f"format {rate} {channels} {fmt}"

    def stop_playing(self):
        self.state = "inactive"

    def unknown(self):
        self.state = "unknown"

    def close(self):
        self._stop.set()
        self._thread.join(timeout=5)
        self._server.close()
