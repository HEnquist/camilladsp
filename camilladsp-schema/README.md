# camilladsp-schema

The configuration side of [CamillaDSP](https://crates.io/crates/camilladsp), with no audio or DSP
dependencies. It contains:

- the config types, and every validation rule CamillaDSP applies to a config
- reading of filter coefficient files
- the websocket protocol types, the commands and replies exchanged with a running CamillaDSP

This makes it possible to read, validate and edit CamillaDSP configs, or to talk to a running
CamillaDSP over the websocket, without pulling in the engine itself.
The `camilladsp` crate re-exports everything here from the same paths.

## Features

| Feature | Description |
|---------|-------------|
| `pipewire-backend` | Accept PipeWire devices in configs |
| `dummy-backend` | Accept the test-only Dummy devices in configs |
| `utoipa` | OpenAPI schemas for the config and protocol types |

## Versioning

This crate is released together with `camilladsp` and shares its version numbers.
Semantic versioning is not guaranteed for the Rust API, so pin to an exact version.

## License

Licensed under either of the GNU General Public License version 3 or the Mozilla Public License
Version 2.0, at your option.
