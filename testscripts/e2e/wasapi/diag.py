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

# An independent loopback of the render endpoint, through the soundcard package.
from wasapi.cable import render_endpoint  # noqa: E402

import soundcard as sc  # noqa: E402

render_name = render_endpoint()[1]
for mic in sc.all_microphones(include_loopback=True):
    print("soundcard input", repr(mic.name), mic.isloopback)
feed = Feeder()
time.sleep(0.5)
for mic in sc.all_microphones(include_loopback=True):
    if mic.isloopback and "VB-Audio" in mic.name:
        try:
            data = mic.record(samplerate=RATE, numframes=RATE)
            col = data[RATE // 4 :, 0].astype(np.float64)
            print("soundcard loopback", mic.name, f"{sine_level_db(col):.2f} dB", np.round(col[:6], 4))
        except Exception as err:  # noqa: BLE001
            print("soundcard loopback", mic.name, "error", err)
feed.stop()
