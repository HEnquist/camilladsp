"""A client for the dummy devices' test control socket.

The websocket controls CamillaDSP. This controls the fake hardware under it: it makes a
dummy device stall, go silent, or run its clock off nominal while the engine is running,
and reads back the frame, pause and resync counters. The protocol is one line in and one
line out, see `src/dummy_backend/control.rs`.

A connection is opened per command and closed again, so a test that leaves one dangling
cannot block the device's listener, which serves one connection at a time.
"""

import socket
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
