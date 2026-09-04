import json
from datetime import datetime

import websocket

WS_URL = "ws://127.0.0.1:1234"

# CamillaDSP websocket state streaming quick reference:
#
# Subscribe command:
#   {"command": "SubscribeState"}
#
# Stop command:
#   {"command": "StopSubscription"}
#
# Typical pushed response while running:
# {
#   "reply": "StateEvent",
#   "result": "Ok",
#   "value": {
#     "state": "Running"
#   }
# }
#
# Typical pushed response when stopped/inactive:
# {
#   "reply": "StateEvent",
#   "result": "Ok",
#   "value": {
#     "state": "Inactive",
#     "stop_reason": "Done"
#   }
# }

ws = websocket.create_connection(WS_URL)

# Command 1: start streaming processing state changes.
ws.send(json.dumps({"command": "SubscribeState"}))

start_reply = json.loads(ws.recv())
start_status = start_reply.get("result")
if start_status != "Ok":
    raise RuntimeError(f"Failed to start subscription: {start_reply}")

print("Subscribed to state changes. Press Ctrl-C to stop.")

try:
    while True:
        message = ws.recv()
        payload = json.loads(message)
        if payload.get("reply") == "StateEvent":
            value = payload.get("value", {})
            state = value.get("state")
            stop_reason = value.get("stop_reason")
            timestamp = datetime.now().isoformat(timespec="milliseconds")
            if stop_reason is not None:
                print(f"{timestamp} state={state}, stop_reason={stop_reason}")
            else:
                print(f"{timestamp} state={state}")
except KeyboardInterrupt:
    print("Stopping state subscription...")

    # Command 2: stop the active subscription.
    ws.send(json.dumps({"command": "StopSubscription"}))

    stop_reply = json.loads(ws.recv())
    stop_status = stop_reply.get("result")
    if stop_status != "Ok":
        raise RuntimeError(f"Failed to stop subscription: {stop_reply}")

    ws.close()
