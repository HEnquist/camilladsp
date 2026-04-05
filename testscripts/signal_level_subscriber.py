import json
import websocket

WS_URL = "ws://127.0.0.1:1234"

# CamillaDSP websocket level streaming quick reference:
#
# Subscribe command:
#   {"SubscribeSignalLevels": "capture"}
#
# Supported options for SubscribeSignalLevels:
#   "capture"  -> stream capture-side levels
#   "playback" -> stream playback-side levels
#   "both"     -> stream both capture and playback from one connection
#
# Typical pushed response format:
# {
#   "SignalLevelsEvent": {
#     "result": "Ok",
#     "value": {
#       "side": "capture",
#       "rms": [-58.1, -57.6],
#       "peak": [-39.4, -38.9]
#     }
#   }
# }

ws = websocket.create_connection(WS_URL)
ws.send(json.dumps({"SubscribeSignalLevels": "both"}))

print("Subscribed to capture and playback signal levels. Press Ctrl-C to stop.")

# The server will push level updates as they occur.
# This loop will print them until interrupted.
try:
    while True:
        message = ws.recv()
        payload = json.loads(message)
        if "SignalLevelsEvent" in payload:
            value = payload["SignalLevelsEvent"]["value"]
            side = value["side"]
            rms = ", ".join(str(v) for v in value["rms"])
            peak = ", ".join(str(v) for v in value["peak"])
            print(f"side={side} rms=[{rms}] peak=[{peak}]")
except KeyboardInterrupt:
    print("Stopping subscription...")
    ws.send(json.dumps("StopSignalLevelSubscription"))
    ws.close()
