"""Temporary: PortAudio's own capture of CABLE Output at each width, exclusive and shared."""

import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from wasapi.cable import RATE, Feeder, capture_endpoint, sine_level_db  # noqa: E402

import sounddevice as sd  # noqa: E402


def to_float(raw, dtype):
    if dtype == "int24":
        b = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        v = (b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)) << 8
        return v.astype(np.float64).reshape(-1, 2) / 2**31
    arr = np.frombuffer(raw, dtype=dtype).reshape(-1, 2).astype(np.float64)
    return arr / {"int16": 2**15, "int32": 2**31, "float32": 1}[dtype]


feed = Feeder()
time.sleep(0.5)
for dtype in ("int16", "int24", "int32", "float32"):
    for exclusive in (True, False):
        settings = sd.WasapiSettings(exclusive=True) if exclusive else sd.WasapiSettings(auto_convert=True)
        try:
            with sd.RawInputStream(
                device=capture_endpoint()[0], samplerate=RATE, channels=2, dtype=dtype,
                extra_settings=settings,
            ) as stream:
                raw, _ = stream.read(RATE)
        except Exception as err:  # noqa: BLE001
            print(dtype, exclusive, "error", err)
            continue
        col = to_float(bytes(raw), dtype)[RATE // 4 :, 0]
        print(dtype, "exclusive" if exclusive else "shared", f"{sine_level_db(col):.2f} dB",
              np.round(col[:6], 4))
feed.stop()

# PortAudio's loopback of the render endpoint, while the feeder plays into it.
from wasapi.cable import render_endpoint  # noqa: E402

print(sd.get_portaudio_version())
render_name = render_endpoint()[1]
for index, dev in enumerate(sd.query_devices()):
    if "oopback" in dev["name"]:
        print("loopback device", index, dev["name"], dev["max_input_channels"])
feed = Feeder()
time.sleep(0.5)
for index, dev in enumerate(sd.query_devices()):
    if "oopback" in dev["name"] and render_name.split(" (")[0] in dev["name"] and dev["max_input_channels"] >= 2:
        try:
            with sd.InputStream(device=index, samplerate=RATE, channels=2, dtype="float32") as stream:
                data, _ = stream.read(RATE)
            col = data[RATE // 4 :, 0].astype(np.float64)
            print("PortAudio loopback", dev["name"], f"{sine_level_db(col):.2f} dB", np.round(col[:6], 4))
        except Exception as err:  # noqa: BLE001
            print("PortAudio loopback", dev["name"], "error", err)
feed.stop()
