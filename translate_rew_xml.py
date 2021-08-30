import xml.etree.ElementTree as ET                                                                   
import sys
import yaml

try:
    fname = sys.argv[1]
except Exception:
    print("Translate a REW XML equalization file to CamillaDSP filters.", file=sys.stderr)
    print("This script creates the 'filters' and 'pipeline' sections of a CamillaDSP config.", file=sys.stderr)
    print("Usage:", file=sys.stderr)
    print("> python translate_rew_xml.py file_from_rew.xml", file=sys.stderr)
    print("Output can also be redirected to a file:", file=sys.stderr)
    print("> python translate_rew_xml.py file_from_rew.xml > my_rew_filter.yml", file=sys.stderr)
    sys.exit()

tree = ET.parse(fname)
root = tree.getroot()

filters = {}
pipeline = []

for channel, speaker in enumerate(root):
    speakername = speaker.get('location')
    print(f"Found speaker: {speakername}", file=sys.stderr)
    pipelinestep = {"type": "Filter", "channel": channel, "names": [] }
    for filt in speaker:
        filt_num = filt.get('number')
        filt_enabled = filt.get('enabled')
        freq = float(filt.find('frequency').text)
        gain = float(filt.find('level').text)
        q = float(filt.find('Q').text)
        print(f"Found filter: {filt_num}, enabled: {filt_enabled}, f: {freq}, Q: {q}, gain: {gain}", file=sys.stderr)
        filter_name = f"{speakername}_{filt_num}"
        filtparams = {"type": "Peaking", "freq": freq, "gain": gain, "q": q}
        filtdata = {"type": "Biquad", "parameters": filtparams }
        filters[filter_name] = filtdata
        pipelinestep["names"].append(filter_name)
    pipeline.append(pipelinestep)

print("\nTranslated config, copy-paste into CamillaDSP config file:\n", file=sys.stderr)
config = {"filters": filters, "pipeline": pipeline}
print(yaml.dump(config))

