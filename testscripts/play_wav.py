#play wav
import yaml
from websocket import create_connection
import sys
import os
import json
from analyze_wav import read_wav_header

try:
    port = int(sys.argv[1])
    template_file = os.path.abspath(sys.argv[2])
    wav_file = os.path.abspath(sys.argv[3])
except:
    print("Usage: start CamillaDSP with the websocket server enabled, and wait mode:")
    print("> camilladsp -p4321 -w")
    print("Then play a wav file:")
    print("> python play_wav.py 4321 path/to/some/template/config.yml path/to/file.wav")
    sys.exit()
# read the config to a Python dict
with open(template_file) as f:
    cfg=yaml.safe_load(f)

wav_info = read_wav_header(wav_file)
if wav_info["SampleFormat"] == "unknown":
    print("Unknown wav sample format!")

# template
capt_device = {
    "type": "File",
    "filename": wav_file,
    "format": wav_info["SampleFormat"],
    "channels": wav_info["NumChannels"],
    "skip_bytes": wav_info["DataStart"],
    "read_bytes":  wav_info["DataLength"],
}
# Modify config
cfg["devices"]["capture_samplerate"] = wav_info["SampleRate"]
cfg["devices"]["enable_rate_adjust"] = False
if cfg["devices"]["samplerate"] != cfg["devices"]["capture_samplerate"]:
    cfg["devices"]["enable_resampling"] = True
    cfg["devices"]["resampler_type"] = "Synchronous"
else:
    cfg["devices"]["enable_resampling"] = False
cfg["devices"]["capture"] = capt_device

# Serialize to yaml string
modded = yaml.dump(cfg)

# Send the modded config
ws = create_connection("ws://127.0.0.1:{}".format(port))
ws.send(json.dumps({"SetConfig": modded))
ws.recv()