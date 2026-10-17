import json
import websocket

WS_URL = "ws://127.0.0.1:1234"

# CamillaDSP websocket level streaming quick reference:
#
# Subscribe command:
#   {"command": "SubscribeSignalLevels", "value": "capture"}
#
# Supported options for the "value" field:
#   "capture"  -> stream capture-side levels
#   "playback" -> stream playback-side levels
#   "both"     -> stream both capture and playback from one connection
#
# Typical pushed response format:
# {
#   "reply": "SignalLevelsEvent",
#   "result": "Ok",
#   "value": {
#     "side": "capture",
#     "rms": [-58.1, -57.6],
#     "peak": [-39.4, -38.9]
#   }
# }

ws = websocket.create_connection(WS_URL)

# Command 1: start streaming signal levels for both sides.
ws.send(json.dumps({"command": "SubscribeSignalLevels", "value": "both"}))

start_reply = json.loads(ws.recv())
start_status = start_reply.get("result")
if start_status != "Ok":
    raise RuntimeError(f"Failed to start subscription: {start_reply}")

print("Subscribed to capture and playback signal levels. Press Ctrl-C to stop.")

# The server will push level updates as they occur.
# This loop will print them until interrupted.
try:
    while True:
        message = ws.recv()
        payload = json.loads(message)
        if payload.get("reply") == "SignalLevelsEvent":
            value = payload["value"]
            side = value["side"]
            rms = ", ".join(str(v) for v in value["rms"])
            peak = ", ".join(str(v) for v in value["peak"])
            print(f"side={side} rms=[{rms}] peak=[{peak}]")
except KeyboardInterrupt:
    print("Stopping subscription...")

    # Command 2: stop the active subscription.
    ws.send(json.dumps({"command": "StopSubscription"}))

    stop_reply = json.loads(ws.recv())
    stop_status = stop_reply.get("result")
    if stop_status != "Ok":
        raise RuntimeError(f"Failed to stop subscription: {stop_reply}")

    ws.close()
