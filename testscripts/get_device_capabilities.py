import argparse
import json

import websocket

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 1234


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--list", action="store_true")
    direction_group = parser.add_mutually_exclusive_group()
    direction_group.add_argument(
        "-p",
        "--playback",
        action="store_const",
        const="playback",
        dest="direction",
        default="playback",
    )
    direction_group.add_argument(
        "-c",
        "--capture",
        action="store_const",
        const="capture",
        dest="direction",
    )
    parser.add_argument("-b", "--backend", required=True)
    parser.add_argument("-d", "--device")
    args = parser.parse_args()
    if not args.list and not args.device:
        parser.error("--device is required unless --list is used")
    return args


def main():
    args = parse_args()
    ws_url = f"ws://{args.host}:{args.port}"
    if args.list:
        command_name = (
            "GetAvailableCaptureDevices"
            if args.direction == "capture"
            else "GetAvailablePlaybackDevices"
        )
        command = {command_name: args.backend}
    else:
        command_name = (
            "GetCaptureDeviceCapabilities"
            if args.direction == "capture"
            else "GetPlaybackDeviceCapabilities"
        )
        command = {command_name: [args.backend, args.device]}

    ws = websocket.create_connection(ws_url)
    try:
        ws.send(json.dumps(command))
        reply = json.loads(ws.recv())
        print(json.dumps(reply, indent=2))
    finally:
        ws.close()


if __name__ == "__main__":
    main()