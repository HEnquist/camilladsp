#!/usr/bin/env python3
"""
ASCII-art spectrum analyzer for CamillaDSP.

Polls GetSpectrum every POLL_INTERVAL seconds and draws a 30-band bar chart.

GetSpectrum command format:
  {
    "GetSpectrum": {
      "side":     "capture" | "playback",
      "channel":  null (average all) | 0, 1, 2, ...  (single channel),
      "min_freq": <Hz>,
      "max_freq": <Hz>,
      "n_bins":   <integer>
    }
  }

Response:
  {
    "GetSpectrum": {
      "result": "Ok",
      "value": {
        "frequencies": [<Hz>, ...],
        "magnitudes":  [<dBFS>, ...]
      }
    }
  }
"""

import json
import sys
import time

import websocket

WS_URL = "ws://127.0.0.1:1234"
SIDE = "capture"
CHANNEL = None       # None = all channels averaged; 0, 1, ... for a single channel
N_BINS = 60
MIN_FREQ = 20.0
MAX_FREQ = 20000.0

ROWS_PER_20DB = 5    # rows per 20 dB interval; total height scales with DB range
DB_MIN = -80.0
DB_MAX = 0.0
POLL_INTERVAL = 0.1  # seconds between requests (~10 fps)
BAR_WIDTH = 1        # terminal columns per frequency band

_DB_STEP = 20.0
_N_INTERVALS = round((DB_MAX - DB_MIN) / _DB_STEP)
HEIGHT = _N_INTERVALS * ROWS_PER_20DB

# Colours per 20 dB interval, from just below the top downward; last entry repeats.
_INTERVAL_COLOURS = [
    "\033[48;5;208m",  # orange
    "\033[43m",        # yellow
    "\033[42m",        # green
    "\033[46m",        # cyan
    "\033[44m",        # blue
]


def _build_tick_labels():
    labels = {}
    for i in range(_N_INTERVALS):
        db = DB_MIN + i * _DB_STEP
        labels[i * ROWS_PER_20DB + 1] = f"  {db:+.0f} dBFS"
    labels[HEIGHT] = f"  {DB_MAX:+.0f} dBFS"
    return labels

_DB_TICK_LABELS = _build_tick_labels()


def _row_colour(row):
    if row == HEIGHT:
        return "\033[101m"  # bright red, top row only
    interval = (HEIGHT - 1 - row) // ROWS_PER_20DB
    return _INTERVAL_COLOURS[min(interval, len(_INTERVAL_COLOURS) - 1)]


def fmt_freq(hz):
    return f"{hz / 1000:.0f}k" if hz >= 1000 else f"{hz:.0f}"


def render(frequencies, magnitudes):
    heights = [
        min(HEIGHT, round(max(0.0, (m - DB_MIN) / (DB_MAX - DB_MIN) * HEIGHT)))
        for m in magnitudes
    ]
    # Frequency axis: one label every 5 bands.
    freq_row = "".join(
        fmt_freq(frequencies[i]).ljust(5 * BAR_WIDTH)[: 5 * BAR_WIDTH]
        for i in range(0, len(frequencies), 5)
    )
    lines = []
    lines.append(freq_row)

    for row in range(HEIGHT, 0, -1):
        cell = _row_colour(row) + " \033[0m"
        bar = "".join(
            cell * BAR_WIDTH if h >= row else " " * BAR_WIDTH
            for h in heights
        )
        lines.append(bar + _DB_TICK_LABELS.get(row, ""))

    lines.append(freq_row)

    return lines


def main():
    ws = websocket.create_connection(WS_URL)

    request = json.dumps({
        "GetSpectrum": {
            "side": SIDE,
            "channel": CHANNEL,
            "min_freq": MIN_FREQ,
            "max_freq": MAX_FREQ,
            "n_bins": N_BINS,
        }
    })

    print(f"Spectrum analyzer: {N_BINS} bands, {MIN_FREQ:.0f}–{MAX_FREQ/1000:.0f}k Hz, "
          f"poll interval {POLL_INTERVAL*1000:.0f} ms  |  Ctrl-C to exit")

    lines_drawn = 0
    try:
        while True:
            ws.send(request)
            reply = json.loads(ws.recv())
            payload = reply.get("GetSpectrum", {})

            if payload.get("result") != "Ok":
                sys.stderr.write(f"Error: {payload.get('result')}\n")
                time.sleep(1.0)
                continue

            value = payload["value"]
            lines = render(value["frequencies"], value["magnitudes"])

            # Move cursor up to overwrite the previous frame.
            if lines_drawn:
                sys.stdout.write(f"\033[{lines_drawn}A")
            sys.stdout.write("\n".join(lines) + "\n")
            sys.stdout.flush()
            lines_drawn = len(lines)

            time.sleep(POLL_INTERVAL)

    except KeyboardInterrupt:
        print()
    finally:
        ws.close()


if __name__ == "__main__":
    main()
