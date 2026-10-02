# PipeWire backend

The PipeWire backend creates filter nodes in the PipeWire graph.
Unlike other backends that connect directly to devices,
PipeWire nodes are meant to be connected via WirePlumber rules or tools like Helvum.

## Sources and sinks

PipeWire describes every node by the direction the audio flows,
not by whether the node is a piece of hardware or an application.

- A *source* produces audio and feeds it into the graph.
  Microphones and line inputs are sources, and so is a music player sending out its audio.
- A *sink* consumes audio from the graph.
  Speakers and headphones are sinks, and so is an application that is recording.

The CamillaDSP capture device takes audio from a source,
and the CamillaDSP playback device sends audio to a sink.

Every sink additionally has a set of *monitor* ports that carry whatever that sink is receiving.
Capturing from those is how to process audio that something else is playing,
see [Loopback capture](#loopback-capture).

`wpctl status` lists the sources and sinks on the system,
with applications shown under "Streams".

## Build requirements

The PipeWire backend requires the PipeWire development libraries:

- Debian/Ubuntu: `sudo apt install libpipewire-0.3-dev`
- Fedora: `sudo dnf install pipewire-devel`
- Arch Linux: `sudo pacman -S pipewire`

Build with the `pipewire-backend` feature:
```bash
cargo build --release --features pipewire-backend
```

## Configuration

Like PulseAudio, PipeWire uses F32 internally, so no sample format configuration is needed.

### Capture device
```yaml
capture:
  type: PipeWire
  channels: 2
  node_name: camilladsp-capture (*)
  node_description: CamillaDSP Capture (*)
  node_group_name: camilladsp (*)
  autoconnect_to: null (*)
  loopback: false (*)
```

### Playback device
```yaml
playback:
  type: PipeWire
  channels: 2
  node_name: camilladsp-playback (*)
  node_description: CamillaDSP Playback (*)
  node_group_name: camilladsp (*)
  autoconnect_to: null (*)
```

The parameters marked (*) are optional. The values shown above are the defaults.
These are used if the parameters are set to `null` or left out from the configuration.

### Parameters

| Parameter | Description |
|-----------|-------------|
| `channels` | Number of audio channels (required) |
| `node_name` | PipeWire node name for WirePlumber matching (optional, defaults to `camilladsp-capture` or `camilladsp-playback`) |
| `node_description` | PipeWire node description, shown in tools such as Helvum (optional, defaults to `CamillaDSP Capture` or `CamillaDSP Playback`) |
| `node_group_name` | PipeWire node group name (optional, defaults to `camilladsp`) |
| `autoconnect_to`| PipeWire name or serial (given as a string with quotes, `"123"`) of a node to autoconnect to (optional) |
| `loopback` | Capture from a sink instead of from a source, capture only (optional, defaults to `false`) |

#### Node groups

PipeWire nodes can be assigned to *groups*.
Nodes in the same group are always scheduled by the same driver.
This ensures that these nodes run in the same graph execution cycle, sharing the same clock and timing.
Use the same group name for capture and playback.
Leave at the default value unless more than one CamillaDSP instance is running.


#### Autoconnect

The `autoconnect_to` parameter takes either a name or a serial number of a PipeWire node.
The property is a string, so serial numbers must be quoted in the yaml file (`autoconnect_to: "123"`).
If given, CamillaDSP will ask PipeWire to try connect the CamillaDSP capture or playback node to the given node.
This enables basic routing to be set up without any additional tools,
and is useful when both the source and sink nodes already exist.

A capture device connects to a source, and a playback device to a sink.
To capture the output of a sink instead, see [Loopback capture](#loopback-capture).

If the target node does not exist, the CamillaDSP node is left unconnected
instead of being connected to the default device,
and gets connected automatically if the target appears later.
This means that a wrong or misspelled name gives silence rather than audio from an unexpected device.

For anything more advanced, it is recommended to leave this parameter at the default `null`,
and instead set up routing with WirePlumber rules.

Node names can be found with the `pw-cli` command:
```sh
pw-cli ls Node
```

Example of node names for audio playback devices:
- Intel HD Audio headphone output: `alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Headphones__sink`
- MOTU M4: `alsa_output.hw_M4_0`


#### Loopback capture

A capture device normally connects to a source.
Setting `loopback` to `true` makes it capture from the output of a sink instead,
via the monitor ports that PipeWire provides for every sink.
This is how to process the audio that some other application is playing,
and corresponds to loopback capture in the WASAPI backend.

Combine it with `autoconnect_to` to pick which sink to capture from:
```yaml
capture:
  type: PipeWire
  channels: 2
  autoconnect_to: alsa_output.pci-0000_00_1f.3.analog-stereo
  loopback: true
```

The flag is needed even when the sink is named explicitly.
WirePlumber only considers sources when it looks up an `autoconnect_to` name for a capture device,
so without `loopback: true` a sink name matches nothing and the node stays unconnected.
This does not apply when the sink is given as a serial number,
but a name is usually the better choice since serial numbers change when a node is recreated.

Sinks are the nodes with a `media.class` of `Audio/Sink`.
`wpctl status` lists them under "Sinks".


## WirePlumber routing

CamillaDSP by default creates nodes that do not auto-connect to any devices.
Use WirePlumber rules to connect them to your audio sources and sinks.

### Example WirePlumber rules (0.5+)

WirePlumber 0.5 and later use `.conf` files with SPA-JSON format instead of Lua.

Create `~/.config/wireplumber/wireplumber.conf.d/51-camilladsp.conf`:

```conf
monitor.alsa.rules = [
  # Connect a USB microphone to CamillaDSP capture
  {
    matches = [
      { node.name = "alsa_input.usb-Some_Microphone-00.analog-stereo" }
    ]
    actions = {
      update-props = {
        target.object = "camilladsp-capture"
      }
    }
  }
  # Connect CamillaDSP playback to speakers
  {
    matches = [
      { node.name = "camilladsp-playback" }
    ]
    actions = {
      update-props = {
        target.object = "alsa_output.pci-0000_00_1f.3.analog-stereo"
      }
    }
  }
]
```

Restart WirePlumber after adding rules:
```bash
systemctl --user restart wireplumber
```

To find the correct node names, use:
```bash
wpctl status
```

### Manual connection with pw-link

You can also connect nodes manually:
```bash
# List available nodes
pw-cli ls Node

# Connect capture
pw-link "alsa_input.usb-...:capture_FL" "camilladsp-capture:input_0"
pw-link "alsa_input.usb-...:capture_FR" "camilladsp-capture:input_1"

# Connect playback
pw-link "camilladsp-playback:output_0" "alsa_output.pci-...:playback_FL"
pw-link "camilladsp-playback:output_1" "alsa_output.pci-...:playback_FR"
```

### Using Helvum

Helvum is a graphical patchbay for PipeWire.
Install it and drag connections between CamillaDSP nodes and your audio devices.

## Monitoring

Use `pw-top` to see active PipeWire nodes and their CPU usage:
```bash
pw-top
```

Use `pw-cli` to inspect CamillaDSP nodes:
```bash
pw-cli info camilladsp-capture
pw-cli info camilladsp-playback
```

## Full example configuration

```yaml
devices:
  samplerate: 48000
  chunksize: 1024
  capture:
    type: PipeWire
    channels: 2
    # node_name defaults to camilladsp-capture
  playback:
    type: PipeWire
    channels: 4
    # node_name defaults to camilladsp-playback

filters:
  lowpass:
    type: Biquad
    parameters:
      type: Lowpass
      freq: 80
      q: 0.707

mixers:
  stereo_to_quad:
    channels:
      in: 2
      out: 4
    mapping:
      - dest: 0
        sources:
          - channel: 0
            gain: 0
      - dest: 1
        sources:
          - channel: 1
            gain: 0
      - dest: 2
        sources:
          - channel: 0
            gain: 0
      - dest: 3
        sources:
          - channel: 1
            gain: 0

pipeline:
  - type: Mixer
    name: stereo_to_quad
  - type: Filter
    channel: 2
    names:
      - lowpass
  - type: Filter
    channel: 3
    names:
      - lowpass
```

This configuration:
1. Captures 2-channel audio from whatever is routed to `camilladsp-capture`
2. Mixes to 4 channels (stereo + subwoofer pair)
3. Applies lowpass filter to channels 2 and 3 (subwoofers)
4. Outputs 4 channels to `camilladsp-playback`
