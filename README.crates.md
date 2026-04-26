# CamillaDSP

CamillaDSP is a cross-platform audio processing engine designed for
flexible, low-latency DSP pipelines.
It runs as a stand-alone application, reading its configuration from a
YAML file and exposing a WebSocket interface for runtime control.

Currently, users of the binary are the primary audience, and their stability
and experience take priority. Library use is a bonus — the DSP components are
made available as a crate for use in other applications, but it is not yet a
first-class use case. This may change in the future depending on adoption and
user feedback.

When used as a library, the degree of integration is up to the application.
It is possible to run the full engine including audio I/O, or to use only
individual components — for example, creating a single filter and applying
it to a buffer of audio samples — or anything in between.

## Versioning and documentation

Because the primary audience is binary users, both versioning and
documentation reflect that priority.

Version numbers follow the releases of the CamillaDSP binary. Semantic
versioning is not guaranteed from a library consumer's perspective — a patch
release that introduces no breaking changes for binary users may still
contain breaking changes to the library API. If you depend on this crate,
pin to an exact version and review the changes before upgrading.

Likewise, API documentation may be sparse or absent in places. No
guarantees are made about its completeness.

## Backends

Platform-specific audio backends are selected at compile time via feature flags:

| Feature | Backend | Platform |
|---------|---------|----------|
| *(default)* | ALSA | Linux |
| `pulse-backend` | PulseAudio | Linux |
| `pipewire-backend` | PipeWire | Linux |
| `cpal-backend` / `jack-backend` | CPAL / JACK | Linux, macOS, Windows |
| `asio-backend` | ASIO | Windows |
| *(default)* | CoreAudio | macOS |
| *(default)* | WASAPI | Windows |

## Pipeline components

- `filters` — biquad EQ, FIR/IIR convolution, loudness, gain, delay, dither
- `mixer` — channel routing and mixing matrices
- `processors` — compressor, noise gate, and other dynamics processors
- `pipeline` — assembles components into an ordered processing graph

## Configuration

See the `config` module for the full configuration schema. Configs are
serialised as YAML and can be validated, patched, and reloaded without
stopping the engine.

## Embedding CamillaDSP

The `engine` module exposes the top-level supervisor that owns the audio
threads. `processing` contains the inner pipeline loop. Status and control
messages flow through the types defined in the crate root
(`StatusMessage`, `CommandMessage`, `CaptureStatus`, `PlaybackStatus`, …).

The `32bit` feature switches the internal processing format from `f64` to `f32`
(`PrcFmt`).
