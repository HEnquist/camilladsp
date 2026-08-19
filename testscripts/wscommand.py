"""Send arbitrary websocket commands to a running CamillaDSP instance.

A thin manual client for poking at the websocket API, useful while pycamilladsp
lags behind the current command format.

Commands are given either as a bare command name, or as full JSON when the
command takes arguments:

    wscommand.py GetState
    wscommand.py GetVersion GetConfigFilePath
    wscommand.py '{"command": "SetConfigFilePath", "value": "some.yml"}' Reload
    wscommand.py '{"command": "GetPlaybackDeviceCapabilities",
                   "backend": "asio", "device": "MOTU M Series"}'

Several commands in one invocation are sent over the same connection, in order,
which is what you want for pairs such as SetConfigFilePath followed by Reload.
"""

import argparse
import json
import sys

import websocket

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 1234


def parse_args():
    parser = argparse.ArgumentParser(
        description="Send websocket commands to CamillaDSP.",
    )
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument(
        "-t",
        "--timeout",
        type=float,
        default=10.0,
        help="socket timeout in seconds",
    )
    parser.add_argument(
        "-m",
        "--max-length",
        type=int,
        default=0,
        help="truncate replies longer than this many characters (0 = no limit). "
        "Handy for GetConfig, which returns the whole config.",
    )
    parser.add_argument(
        "command",
        nargs="+",
        help="command name, or a JSON object for commands that take arguments",
    )
    return parser.parse_args()


def build_command(arg):
    """Accept either a bare command name or a full JSON object."""
    stripped = arg.strip()
    if stripped.startswith("{"):
        try:
            return json.loads(stripped)
        except json.JSONDecodeError as err:
            raise SystemExit(f"Not valid JSON: {arg}\n{err}") from err
    return {"command": stripped}


def format_reply(raw, max_length):
    try:
        text = json.dumps(json.loads(raw), indent=2)
    except json.JSONDecodeError:
        # Print malformed replies verbatim rather than hiding them.
        text = raw
    if max_length and len(text) > max_length:
        return text[:max_length] + f"\n...[truncated, {len(text)} chars total]"
    return text


def main():
    args = parse_args()
    commands = [build_command(arg) for arg in args.command]

    ws = websocket.create_connection(
        f"ws://{args.host}:{args.port}", timeout=args.timeout
    )
    failed = False
    try:
        for command in commands:
            ws.send(json.dumps(command))
            raw = ws.recv()
            print(f"-> {json.dumps(command)}")
            print(format_reply(raw, args.max_length))
            print()
            # A non-Ok result is worth a non-zero exit so this can be scripted.
            try:
                if json.loads(raw).get("result") not in ("Ok", None):
                    failed = True
            except json.JSONDecodeError:
                failed = True
    finally:
        ws.close()

    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
