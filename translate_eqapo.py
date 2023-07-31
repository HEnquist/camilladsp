import sys
import yaml
from copy import copy, deepcopy

class EqAPO():
    filter_types = {
        "PK": "Peaking",
        "PEQ": "Peaking",
        "HP": "Highpass",
        "HPQ": "Highpass",
        "LP": "Lowpass",
        "LPQ": "Lowpass",
        "BP": "Bandpass",
        "NO": "Notch",
        "LS": "Lowshelf",
        "LSC": "Lowshelf",
        "HS": "Highshelf",
        "HSC": "Highshelf",
        "IIR": None, #TODO
    }
    # TODO
    # add support for 
    # HSC x dB: High-shelf filter x dB per oct.
    # LSC x dB: Low-shelf filter x dB per oct.
    # LS 6dB: Low-shelf filter 6 dB per octave, with corner freq.
    # LS 12dB: Low-shelf filter 12 dB per octave, with corner freq.
    # HS 6dB: High-shelf filter 6 dB per octave, with corner freq.
    # HS 12dB: High-shelf filter 12 dB per octave, with corner freq.


    # Label to channel number, assuming 7.1
    channel_map = {
        "L": 0,
        "R": 1,
        "C": 2,
        "LFE": 3,
        "RL": 4,
        "RR": 5,
        "SL": 6,
        "SR": 7
    }

    delay_units = {
        "ms": "ms",
        "samples": "samples"
    }

    def __init__(self, filename, nbr_channels):
        with open(filename) as file:
            self.lines = file.readlines()
        self.filters = {}
        self.mixers = {}
        self.name_index = {
            "Filter": 1,
            "Preamp": 1,
            "Convolution": 1,
            "Delay": 1,
            "Copy": 1,
        }
        self.selected_channels = None
        self.current_filterstep = {
            "type": "Filter",
            "names": [],
            "description": "Default, all channels",
            "channels": copy(self.selected_channels)
        }
        self.pipeline = [self.current_filterstep]
        self.nbr_channels = nbr_channels

    def lookup_channel_index(self, label):
        if label in self.channel_map:
            channel = self.channel_map[label]
        elif label.isnumeric():
            channel = int(label) - 1
        else:
            # TODO handle this
            channel = None
        return channel

    # Parse a single command parameter
    def parse_single_parameter(self, params):
        # Inline expressions (ex: Fc `2*a`) are not supported
        # TODO add a check for this.
        if params[0] == "Fc":
            nbr_tokens = 3
            assert params[2].lower() == "hz"
            value = float(params[1])
            parsed = {"freq": value}
        elif params[0] == "Q":
            nbr_tokens = 2
            value = float(params[1])
            parsed = {"q": value}
        elif params[0] == "Gain":
            nbr_tokens = 3
            assert params[2].lower() == "db"
            value = float(params[1])
            parsed = {"gain": value}
        elif params[0] == "BW":
            nbr_tokens = 3
            assert params[1].lower() == "oct"
            value = float(params[2])
            parsed = {"bandwidth": value}
        else:
            print("Skipping unknown token:", params[0])
            return {}, params[1:]
        return parsed, params[nbr_tokens:]

    # Parse the parameters for a command
    def parse_filter_params(self, param_str):
        params = param_str.split()
        enabled = params[0] == "ON"
        ftype = params[1]
        ftype_c = self.filter_types[ftype]
        param_dict = {"type": ftype_c}
        tokens = params[2:]
        while tokens:
            p, tokens = self.parse_single_parameter(tokens)
            param_dict.update(p)
        return param_dict

    # Parse a Preamp command to a filter
    def parse_gain(self, param_str):
        params = param_str.split()
        gain = float(params[0])
        if params[1].lower() != "db":
            print("invalid preamp line:", param_str)
            return
        return {
            "type": "Gain",
            "parameters": {
                "gain": gain,
                "scale": "dB"
            }
        }

    # Parse a Delay command to a filter
    def parse_delay(self, param_str):
        params = param_str.split()
        delay = float(params[0])
        unit = self.delay_units[params[1]]
        return {
            "type": "Delay",
            "parameters": {
                "delay": delay,
                "unit": unit
            }
        }

    # Parse a Copy command into a Mixer
    def parse_copy(self, param_str):
        handled_channels = set()
        mixer = {
            "channels": {
                "in": self.nbr_channels,
                "out": self.nbr_channels,
            },
            "mapping": []
        }
        params = param_str.strip().split(" ")
        for dest in params:
            dest_ch, expr = dest.split("=")
            dest_ch = self.lookup_channel_index(dest_ch)
            handled_channels.add(dest_ch)
            print("dest", dest_ch)
            mapping = {
                "dest": dest_ch,
                "mute": False,
                "sources": []
            }
            mixer["mapping"].append(mapping)
            sources = expr.split("+")
            for source in sources:
                if "*" in source:
                    gain_str, channel = source.split("*")
                    if gain_str.endswith("dB"):
                        gain = float(gain_str[:-2])
                        scale = "dB"
                    else:
                        gain = float(gain_str)
                        scale = "linear"
                elif source == "0.0":
                    # EqAPO supports setting channels to an arbitrary constant.
                    # Here only 0.0 is supported, as other values have no practical use.
                    channel = None
                else:
                    gain = 0
                    scale = "dB"
                    channel = source
                if channel is not None:
                    channel = self.lookup_channel_index(channel)
                    # TODO make a mixer config
                    print("source", channel, gain, scale)
                    source = {
                        "channel": channel,
                        "gain": gain,
                        "inverted": False,
                        "scale": scale,
                    }
                    mapping["sources"].append(source)
        for dest_ch in set(range(self.nbr_channels)) - handled_channels:
            print("pass through", dest_ch)
            mapping = {
                "dest": dest_ch,
                "mute": False,
                "sources": [
                    {
                        "channel": dest_ch,
                        "gain": 0.0,
                        "inverted": False,
                        "scale": "dB",
                    }
                ]
            }
            mixer["mapping"].append(mapping)
        return mixer


    # Parse a single line
    def parse_line(self, line):
        if not line or line.startswith("#") or not ":" in line:
            return
        filtname = None
        command_name, params = line.split(":", 1)
        command = command_name.split()[0]
        if command in ("Filter", "Convolution", "Preamp", "Delay"):
            if command == "Filter":
                filter = self.parse_filter_params(params)
                filter = {
                    "type": "Biquad",
                    "parameters": filter
                }
            elif command == "Convolution":
                filename = params.strip()
                filter = {
                    "type": "Conv",
                    "parameters": {
                        "filename": filename,
                        "type": "wav"
                    }
                }
            elif command == "Preamp":
                filter = self.parse_gain(params)
            elif command == "Delay":
                filter = self.parse_delay(params)
            filter["description"] = line.strip()
            filtname = f"{command}_{self.name_index[command]}"
            self.name_index[command] += 1
            self.filters[filtname] = filter
            self.pipeline[-1]["names"].append(filtname)
        elif command == "Channel":
            if params.strip() == "all":
                self.selected_channels = None
            else:
                self.selected_channels = [self.lookup_channel_index(c) for c in params.strip().split(" ")]
            new_filterstep = {
                "type": "Filter",
                "names": [],
                "description": line.strip(),
                "channels": copy(self.selected_channels),
            }
            self.pipeline.append(new_filterstep)
        elif command == "Copy":
            mixer = self.parse_copy(params)
            mixer["description"] = line.strip()
            mixername = f"{command}_{self.name_index[command]}"
            self.name_index[command] += 1
            self.mixers[mixername] = mixer
            step = {
                "type": "Mixer",
                "name": mixername,
            }
            self.pipeline.append(step)
            step = {
                "type": "Filter",
                "names": [],
                "description": "Continued after mixer",
                "channels": copy(self.selected_channels)
            }
            self.pipeline.append(step)
        elif command in ("Device", "Include", "Eval", "If", "ElseIf", "Else", "EndIf", "Stage", "GraphicEQ"):
            print("Skipping unsupported command:", command)

    def postprocess(self):
        for idx, step in enumerate(list(self.pipeline)):
            if step["type"] == "Filter" and len(step["names"]) == 0:
                self.pipeline.pop(idx)
        for _, mixer in self.mixers.items():
            for idx, dest in enumerate(list(mixer["mapping"])):
                if len(dest["sources"]) == 0:
                    mixer["mapping"].pop(idx)
        # Expand filter steps to all channels
        pipeline = []
        for step in self.pipeline:
            if step["type"] != "Filter":
                pipeline.append(step)
            else:
                channels = step["channels"]
                if channels is None:
                    channels = range(self.nbr_channels)
                for channel in channels:
                    new_step = deepcopy(step)
                    new_step["channel"] = channel
                    del new_step["channels"]
                    pipeline.append(new_step)
        self.pipeline = pipeline

    def build_config(self):
        config = {
            "filters": self.filters,
            "mixers": self.mixers,
            "pipeline": self.pipeline
        }
        return config

    def translate_file(self):
        for idx, line in enumerate(self.lines):
            self.parse_line(line)
        self.postprocess()
        config = self.build_config()
        return config


if __name__ == "__main__":
    try:
        fname = sys.argv[1]
    except Exception:
        print("Translate an EqualizerAPO filter file to CamillaDSP filters.", file=sys.stderr)
        print("This script creates the 'filters', 'mixers' and 'pipeline' sections of a CamillaDSP config.", file=sys.stderr)
        print("An EqualizerAPO config does not specify the number of channels.", file=sys.stderr)
        print("This is required to generate corresponding CamillaDSP mixers.", file=sys.stderr)
        print("The number of channels is given as the second parameter.", file=sys.stderr)
        print("This is optional and defaults to 2 if omitted.", file=sys.stderr)
        print("Usage:", file=sys.stderr)
        print("> python translate_eqapo.py config.txt 2", file=sys.stderr)
        print("Output can also be redirected to a file:", file=sys.stderr)
        print("> python translate_eqapo.py config.txt > from_eqapo.yml", file=sys.stderr)
        sys.exit()

    try:
        nbr_channels = int(sys.argv[2])
    except Exception:
        print("Assuming 2 channels")
        nbr_channels = 2
    
    translator = EqAPO(fname, nbr_channels)
    config = translator.translate_file()
    print("\nTranslated config, copy-paste into CamillaDSP config file:\n", file=sys.stderr)
    print(yaml.dump(config))

