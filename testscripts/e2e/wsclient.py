"""A minimal CamillaDSP websocket client for the end-to-end tests.

Deliberately not pyCamillaDSP. That library is released separately and will lag: a new
websocket command would be untestable here until the client ships support for it, and a
pinned but lagging client silently narrows what these tests cover. This one just forwards
whatever command name it is handed, so it cannot lag.
"""

import json
import time

from websocket import create_connection


class CommandError(Exception):
    """Raised when a command comes back with a result other than Ok."""

    def __init__(self, command, reply):
        self.command = command
        self.reply = reply
        self.result = reply.get("result")
        message = reply.get("message")
        text = f"{command} returned {self.result}"
        if message:
            text += f": {message}"
        super().__init__(text)


class Client:
    """One websocket connection to a running CamillaDSP."""

    def __init__(self, port, host="127.0.0.1", timeout=10.0):
        self._ws = create_connection(f"ws://{host}:{port}", timeout=timeout)

    def send(self, command, value=None):
        """Send a command and return its value, or None for commands without one.

        Raises CommandError if the result is not Ok.
        """
        message = {"command": command}
        if value is not None:
            message["value"] = value
        self._ws.send(json.dumps(message))
        reply = json.loads(self._ws.recv())
        if reply.get("result") != "Ok":
            raise CommandError(command, reply)
        return reply.get("value")

    def send_raw(self, command, value=None):
        """Send a command and return the whole reply, without raising on an error result."""
        message = {"command": command}
        if value is not None:
            message["value"] = value
        self._ws.send(json.dumps(message))
        return json.loads(self._ws.recv())

    def poll_until_true(self, command, predicate, timeout=10.0, interval=0.02):
        """Poll a getter until predicate(value) holds, and return that value.

        Polling rather than sleeping a fixed time is what keeps these tests both fast and
        free of the flakiness a too-short fixed wait brings.
        """
        deadline = time.monotonic() + timeout
        value = None
        while True:
            value = self.send(command)
            if predicate(value):
                return value
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{command} was still {value!r} after {timeout} s")
            time.sleep(interval)

    def poll_until(self, command, expected, timeout=10.0, interval=0.02):
        """Poll a getter until it returns `expected`, and return the value."""
        try:
            return self.poll_until_true(
                command, lambda value: value == expected, timeout, interval
            )
        except TimeoutError as err:
            raise TimeoutError(f"{err}, expected {expected!r}") from None

    def close(self):
        self._ws.close()
