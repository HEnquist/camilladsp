"""A minimal CamillaDSP websocket client for the end-to-end tests.

Deliberately not pyCamillaDSP. That library is released separately and will lag: a new
websocket command would be untestable here until the client ships support for it, and a
pinned but lagging client silently narrows what these tests cover. This one just forwards
whatever command name it is handed, so it cannot lag.
"""

import json
import time

from websocket import WebSocketTimeoutException, create_connection

# Pushed events carry a `reply` name of their own, so a reply to a command can be told
# apart from an event without tracking what the connection is subscribed to.
EVENT_REPLIES = frozenset(
    {"SignalLevelsEvent", "VuLevelsEvent", "StateEvent", "SpectrumEvent"}
)


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
        self._timeout = timeout
        self._ws = create_connection(f"ws://{host}:{port}", timeout=timeout)

    def send(self, command, value=None, **fields):
        """Send a command and return its value, or None for commands without one.

        Extra named fields are passed straight through, which is what the fader commands
        need: send("SetFaderVolume", -3.0, fader=1).

        Raises CommandError if the result is not Ok.
        """
        reply = self.send_raw(command, value, **fields)
        if reply.get("result") != "Ok":
            raise CommandError(command, reply)
        return reply.get("value")

    def send_raw(self, command, value=None, **fields):
        """Send a command and return the whole reply, without raising on an error result."""
        message = {"command": command, **fields}
        if value is not None:
            message["value"] = value
        self._ws.send(json.dumps(message))
        return json.loads(self._ws.recv())

    def send_text(self, text):
        """Send a raw string and return the raw reply, for malformed input tests."""
        self._ws.send(text)
        return json.loads(self._ws.recv())

    def recv(self, timeout=5.0):
        """Read one message off the socket, pushed event or reply, and return it parsed.

        Raises TimeoutError if nothing arrives, which is what the tests asserting that a
        stream has stopped rely on.
        """
        self._ws.settimeout(timeout)
        try:
            return json.loads(self._ws.recv())
        except WebSocketTimeoutException:
            raise TimeoutError(f"No message arrived within {timeout} s") from None
        finally:
            self._ws.settimeout(self._timeout)

    def recv_events(self, count, timeout=5.0):
        """Read `count` pushed events, and return (arrival time, value) pairs.

        The timestamps are what the cadence assertions need. A reply that is not an
        event fails here rather than being skipped, since nothing else should turn up on
        a subscribed connection.
        """
        events = []
        for _ in range(count):
            message = self.recv(timeout)
            name = message.get("reply")
            if name not in EVENT_REPLIES:
                raise AssertionError(f"Expected a pushed event, got {message}")
            if message.get("result") != "Ok":
                raise CommandError(name, message)
            events.append((time.monotonic(), message.get("value")))
        return events

    def send_while_subscribed(self, command, value=None, **fields):
        """Send a command on a subscribed connection and return the reply to it.

        Events queued before the command was handled still arrive first, so the reply is
        not necessarily the next message on the socket. Only `StopSubscription` is
        accepted while a stream is active, `src/websocket_server/mod.rs:625`, so
        everything else comes back as an `Invalid` reply, and this does not raise on it.
        """
        message = {"command": command, **fields}
        if value is not None:
            message["value"] = value
        self._ws.send(json.dumps(message))
        while True:
            reply = self.recv()
            if reply.get("reply") not in EVENT_REPLIES:
                return reply

    def stop_subscription(self):
        """End the active subscription, discarding any events still in flight."""
        reply = self.send_while_subscribed("StopSubscription")
        if reply.get("result") != "Ok":
            raise CommandError("StopSubscription", reply)
        return reply

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
