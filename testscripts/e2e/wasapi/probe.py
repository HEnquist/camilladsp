"""Probe of VB-Cable on a Windows runner, run by .github/workflows/wasapi_probe.yml.

Not a test. Each subcommand prints what it finds, and exits non zero only where a later
suite would fail too. Meant to be taken apart into fixtures once the answers are in.

    probe.py devices              the WASAPI devices as PortAudio sees them
    probe.py roundtrip            a tone through the cable with PortAudio alone
    probe.py caps [--asio]        the devices and capabilities as CamillaDSP sees them
    probe.py cdsp [--exclusive] [--asio]
                                  CamillaDSP playing into the cable, then capturing from it,
                                  through WASAPI or through FlexASIO on top of the cable
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import sounddevice as sd

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from wsclient import Client  # noqa: E402

REPO = Path(__file__).resolve().parents[3]
CAMILLADSP = REPO / "target" / "e2e" / "camilladsp.exe"
PORT = 12399
FLEXASIO_TOML = Path.home() / "FlexASIO.toml"
# FlexASIO logs only while this file exists.
FLEXASIO_LOG = Path.home() / "FlexASIO.log"
FREQ = 1000.0
AMPLITUDE = 0.5
CHANNELS = 2
# A tone at AMPLITUDE has an RMS of 0.35. Anything well under that is either silence or a
# volume stage in the way, and the printed level says which.
MIN_RMS = 0.1


def wasapi_devices(fragment, output):
    """Index and info of each WASAPI device whose name contains `fragment`."""
    hostapi = next(
        i for i, api in enumerate(sd.query_hostapis()) if "WASAPI" in api["name"]
    )
    key = "max_output_channels" if output else "max_input_channels"
    found = [
        (index, dev)
        for index, dev in enumerate(sd.query_devices())
        if dev["hostapi"] == hostapi and fragment in dev["name"] and dev[key] > 0
    ]
    if not found:
        raise SystemExit(f"no WASAPI {'output' if output else 'input'} matching {fragment!r}")
    return found


def wasapi_device(fragment, output):
    return wasapi_devices(fragment, output)[0]


# The playback side of the cable has two endpoints, a 2 channel one and "CABLE In 16 Ch".
# Installed without the vendor's setup the 2 channel one is called "Speakers" rather than
# "CABLE Input", so it is found by the driver name and by not being the 16 channel one.
def cable_inputs():
    return wasapi_devices("(VB-Audio Virtual Cable)", output=True)


def cable_input():
    return next(d for d in cable_inputs() if "16 Ch" not in d[1]["name"])


def cable_output():
    return wasapi_device("CABLE Output", output=False)


def tone_stream(device, rate):
    """An output stream playing the tone on every channel until closed."""
    position = [0]

    def callback(out, frames, _time, status):
        if status:
            print("tone:", status)
        t = (np.arange(frames) + position[0]) / rate
        out[:] = (AMPLITUDE * np.sin(2 * np.pi * FREQ * t)).astype(np.float32)[:, None]
        position[0] += frames

    return sd.OutputStream(
        device=device,
        samplerate=rate,
        channels=CHANNELS,
        dtype="float32",
        callback=callback,
        extra_settings=sd.WasapiSettings(auto_convert=True),
    )


def record(device, rate, seconds):
    with sd.InputStream(
        device=device,
        samplerate=rate,
        channels=CHANNELS,
        dtype="float32",
        extra_settings=sd.WasapiSettings(auto_convert=True),
    ) as stream:
        data, overflowed = stream.read(int(seconds * rate))
    if overflowed:
        print("record: overflow")
    return data


def report(what, data):
    """Print the level per channel, and whether it looks like the tone."""
    rms = np.sqrt(np.mean(data.astype(np.float64) ** 2, axis=0))
    peak = np.max(np.abs(data), axis=0)
    ok = bool(np.all(rms > MIN_RMS))
    print(f"{what}: {len(data)} frames, rms {np.round(rms, 4)}, peak {np.round(peak, 4)}, "
          f"{'ok' if ok else 'SILENT'}")
    return ok


def devices():
    print(sd.get_portaudio_version())
    for api in sd.query_hostapis():
        print(api["name"])
    print(sd.query_devices())
    for index, dev in cable_inputs() + [cable_output()]:
        print(f"{index}: {dev}")


def roundtrip():
    in_index, in_dev = cable_output()
    rate = int(in_dev["default_samplerate"])
    ok = True
    for out_index, out_dev in cable_inputs():
        print(f"{out_dev['name']} at {out_dev['default_samplerate']}, CABLE Output at {rate}")
        with tone_stream(out_index, rate):
            time.sleep(0.5)
            data = record(in_index, rate, 2.0)
        ok &= report(f"PortAudio roundtrip from {out_dev['name']}", data[int(0.2 * rate):])
    if not ok:
        raise SystemExit(1)


def caps(backend):
    if backend == "Asio":
        write_flexasio_toml(cable_output()[1]["name"], cable_input()[1]["name"], False)
    proc = subprocess.Popen([str(CAMILLADSP), "-w", "-p", str(PORT)])
    time.sleep(2)
    client = Client(PORT)
    for kind in ("Capture", "Playback"):
        found = client.send(f"GetAvailable{kind}Devices", backend=backend)
        print(f"{kind}: {json.dumps(found, indent=1)}")
        for name, _ in found:
            if "VB-Audio" in name or "FlexASIO" in name:
                reply = client.send_raw(f"Get{kind}DeviceCapabilities", backend=backend, device=name)
                print(name, json.dumps(reply, indent=1))
    client.send_raw("Exit")
    proc.wait(10)


def start_camilladsp(config, name):
    path = Path(f"{name}.yml")
    path.write_text(config)
    log = open(f"{name}.log", "w")
    proc = subprocess.Popen(
        [str(CAMILLADSP), "-v", "-p", str(PORT), str(path)], stderr=log, stdout=log
    )
    deadline = time.monotonic() + 10
    while True:
        try:
            client = Client(PORT)
            break
        except OSError:
            if time.monotonic() > deadline or proc.poll() is not None:
                raise SystemExit(f"{name}: websocket never came up") from None
            time.sleep(0.1)
    while client.send("GetState") != "Running":
        if time.monotonic() > deadline or proc.poll() is not None:
            print(f"{name}: state {client.send_raw('GetState')}")
            break
        time.sleep(0.1)
    return proc, client


def stop_camilladsp(proc, client, name):
    try:
        print(f"{name}: state {client.send_raw('GetState').get('value')}, "
              f"stop reason {client.send_raw('GetStopReason').get('value')}")
        client.send_raw("Exit")
    except Exception as err:  # noqa: BLE001, a probe reports and carries on
        print(f"{name}: websocket gone: {err}")
    try:
        proc.wait(10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    print(f"{name}: exit code {proc.returncode}")
    print(Path(f"{name}.log").read_text()[-4000:])
    if FLEXASIO_LOG.exists():
        print(f"FlexASIO log, {FLEXASIO_LOG.stat().st_size} bytes, the tail:")
        print(FLEXASIO_LOG.read_text(errors="replace")[-3000:])
        FLEXASIO_LOG.write_text("")


def wasapi_block(side, device, exclusive, fmt):
    lines = [f"  {side}:", "    type: Wasapi", f"    channels: {CHANNELS}",
             f'    device: "{device}"', f"    exclusive: {'true' if exclusive else 'false'}"]
    if fmt:
        lines.append(f"    format: {fmt}")
    return "\n".join(lines)


def asio_block(side, fmt):
    return "\n".join([f"  {side}:", "    type: Asio", f"    channels: {CHANNELS}",
                      '    device: "FlexASIO"', f"    format: {fmt}"])


def write_flexasio_toml(input_device, output_device, exclusive):
    """FlexASIO reads its settings from the user profile. An empty device disables that
    direction, which the one way runs need: a driver with both open on the one cable would
    feed its own output back in."""
    lines = ['backend = "Windows WASAPI"']
    for section, device in (("input", input_device), ("output", output_device)):
        lines += ["", f"[{section}]", f'device = "{device}"']
        if exclusive:
            lines += ['sampleType = "Int16"', "wasapiExclusiveMode = true",
                      "wasapiExplicitSampleFormat = true"]
    FLEXASIO_TOML.write_text("\n".join(lines) + "\n")
    print(FLEXASIO_TOML.read_text())


def cdsp(exclusive, backend):
    _, out_dev = cable_input()
    in_index, in_dev = cable_output()
    rate = int(in_dev["default_samplerate"])
    mode = ("exclusive" if exclusive else "shared") + ("_asio" if backend == "Asio" else "")
    ok = True

    # Shared mode is always F32, exclusive needs a format the driver takes. FlexASIO does
    # no conversion, so its ASIO side has the sample type the TOML asks for.
    def block(side, device):
        if backend == "Asio":
            return asio_block(side, "S16_LE" if exclusive else "F32_LE")
        return wasapi_block(side, device, exclusive, "S16" if exclusive else None)

    # Playback: a generated tone into CABLE Input, recorded off CABLE Output by PortAudio.
    name = f"playback_{mode}"
    if backend == "Asio":
        write_flexasio_toml("", out_dev["name"], exclusive)
    config = "\n".join([
        "devices:",
        f"  samplerate: {rate}",
        "  chunksize: 1024",
        "  capture:",
        "    type: SignalGenerator",
        f"    channels: {CHANNELS}",
        f"    signal: {{type: Sine, freq: {FREQ}, level: -6.0206}}",
        block("playback", out_dev["name"]),
        "pipeline: []",
        "",
    ])
    proc, client = start_camilladsp(config, name)
    time.sleep(0.5)
    data = record(in_index, rate, 2.0)
    stop_camilladsp(proc, client, name)
    ok &= report(f"CamillaDSP {name}", data[int(0.2 * rate):])

    # Capture: PortAudio plays the tone into CABLE Input, CamillaDSP captures CABLE Output
    # into a raw file.
    name = f"capture_{mode}"
    out_index, _ = cable_input()
    raw = Path(f"{name}.raw")
    if backend == "Asio":
        write_flexasio_toml(in_dev["name"], "", exclusive)
    config = "\n".join([
        "devices:",
        f"  samplerate: {rate}",
        "  chunksize: 1024",
        block("capture", in_dev["name"]),
        "  playback:",
        "    type: File",
        f"    channels: {CHANNELS}",
        f"    filename: {raw.name}",
        "    format: F32_LE",
        "pipeline: []",
        "",
    ])
    with tone_stream(out_index, rate):
        time.sleep(0.3)
        proc, client = start_camilladsp(config, name)
        time.sleep(2.5)
        stop_camilladsp(proc, client, name)
    data = np.fromfile(raw, dtype="<f4").reshape(-1, CHANNELS) if raw.exists() else np.zeros((0, 2))
    ok &= report(f"CamillaDSP {name}", data[int(0.5 * rate):])
    return ok


def main():
    command = sys.argv[1] if len(sys.argv) > 1 else ""
    backend = "Asio" if "--asio" in sys.argv else "Wasapi"
    if command == "devices":
        devices()
    elif command == "roundtrip":
        roundtrip()
    elif command == "caps":
        caps(backend)
    elif command == "cdsp":
        if not cdsp("--exclusive" in sys.argv, backend):
            raise SystemExit(1)
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    os.chdir(os.environ.get("RUNNER_TEMP", "."))
    main()
