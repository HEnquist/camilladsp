# PipeWire backend

The PipeWire backend creates filter nodes in the PipeWire graph.
Unlike other backends that connect directly to devices,
PipeWire nodes are meant to be connected via WirePlumber rules or tools like Helvum.

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
  type: Pipewire
  channels: 2
  node_name: camilladsp-capture  # optional, this is the default
  autoconnect_to: null  # optional
```

### Playback device
```yaml
playback:
  type: Pipewire
  channels: 2
  node_name: camilladsp-playback  # optional, this is the default
  autoconnect_to: null  # optional
```

### Parameters

| Parameter | Description |
|-----------|-------------|
| `channels` | Number of audio channels (required) |
| `node_name` | PipeWire node name for WirePlumber matching (optional, defaults to `camilladsp-capture` or `camilladsp-playback`) |
| `autoconnect_to`| PipeWire name or serial (as a string) of a node to autoconnect to (optional) |

#### Autoconnect

The `autoconnect_to` parameter takes either a name or a serial number (as a string) of a PipeWire node.
If given, CamillaDSP will try to connect its capture and/or playback node to the given node.
This enables basic routing to be set up without any additional tools,
and is useful when both the source and sink nodes already exist.
For anything more advanced, it is recommended to leave this parameter at the default `null`,
and instead set up routing with WirePlumber rules.


## WirePlumber routing

CamillaDSP creates nodes that do not auto-connect to any devices.
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
    type: Pipewire
    channels: 2
    # node_name defaults to camilladsp-capture
  playback:
    type: Pipewire
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
