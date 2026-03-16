# Controlling via REST API

If the REST API server is enabled by building with the `rest-api` feature and providing the `--rest-port` option, CamillaDSP will listen for incoming HTTP requests on the specified port.

By default the REST API server binds to the address 127.0.0.1, which means it's only accessible locally (on the same machine). If it should be also available to remote machines, give the IP address of the interface where it should be available with the `--address` option (shared with the websocket server). Giving 0.0.0.0 will bind to all interfaces.

The REST API coexists with the WebSocket interface. Both can be used simultaneously.

## Getting started

### Building with REST API support
```
cargo build --release --features rest-api
```

### Starting CamillaDSP with the REST API
```
camilladsp --rest-port 1236 /path/to/config.yml
```
The REST API will be available at `http://127.0.0.1:1236/api/v1/`.

## Response format

All responses use a JSON envelope:
```json
{
  "result": "Ok",
  "value": ...
}
```

Error responses include a message:
```json
{
  "result": "Error",
  "message": "Human-readable error description"
}
```

### HTTP status codes

| Code | Meaning |
|------|---------|
| 200  | Successful operation |
| 400  | Malformed request body or invalid parameters |
| 404  | Unknown endpoint |
| 422  | Valid request but operation failed (e.g., invalid config) |
| 500  | Unexpected server failure |

## OpenAPI specification

The full OpenAPI specification is available at:
- `GET /api/v1/openapi.yaml`

The spec is also available in the repository at `docs/openapi.yaml`.

## All endpoints

All endpoints are under the base path `/api/v1`.

### System / Lifecycle

- `GET /version` : Get the CamillaDSP version.
  * Returns the version as a string.
  * Example: `curl http://localhost:1236/api/v1/version`
  * Response: `{"result":"Ok","value":"3.0.1"}`

- `GET /state` : Get the current processing state.
  * Returns one of: "Starting", "Running", "Paused", "Inactive", "Stalled".

- `GET /stopreason` : Get the reason processing was stopped.
  * Returns the stop reason as a string.

- `POST /reload` : Reload the current configuration file.
  * Example: `curl -X POST http://localhost:1236/api/v1/reload`

- `POST /stop` : Stop audio processing.

- `POST /exit` : Exit the CamillaDSP process.

### Configuration

- `GET /config` : Get the active configuration as YAML.
  * Returns the config in YAML as a string value.

- `GET /config/json` : Get the active configuration as JSON.
  * Returns the config in JSON as a string value.

- `GET /config/title` : Get the configuration title.
  * Returns the title as a string.

- `GET /config/description` : Get the configuration description.
  * Returns the description as a string.

- `GET /config/previous` : Get the previous active configuration as YAML.

- `GET /config/filepath` : Get the active configuration file path.
  * Returns the path as a string, or null if not set.

- `PUT /config/filepath` : Set the configuration file path.
  * Request body: `{"value": "/path/to/config.yml"}`
  * The new config is not applied until `POST /reload` is called.

- `PUT /config` : Set and apply a new configuration (YAML).
  * Request body: `{"value": "<yaml config string>"}`
  * Applied directly.

- `PUT /config/json` : Set and apply a new configuration (JSON).
  * Request body: `{"value": "<json config string>"}`
  * Applied directly.

- `POST /config/read` : Parse a YAML config string and return the normalized version.
  * Request body: `{"value": "<yaml config string>"}`
  * Returns the parsed config with defaults filled in.

- `POST /config/readfile` : Read and parse a configuration file from disk.
  * Request body: `{"path": "/path/to/config.yml"}`

- `POST /config/validate` : Validate a YAML configuration string.
  * Request body: `{"value": "<yaml config string>"}`
  * Performs more extensive checks than `read`.

### State File

- `GET /state/filepath` : Get the state file path.
  * Returns the path as a string, or null if no state file is used.

- `GET /state/fileupdated` : Check if the state file is up to date.
  * Returns a boolean.

### Volume and Mute (Main / Fader 0)

- `GET /volume` : Get the main volume in dB.
  * Returns the value as a float.
  * Example: `curl http://localhost:1236/api/v1/volume`
  * Response: `{"result":"Ok","value":-10.0}`

- `PUT /volume` : Set the main volume in dB.
  * Request body: `{"value": -10.0}`
  * Example: `curl -X PUT -H "Content-Type: application/json" -d '{"value":-10.0}' http://localhost:1236/api/v1/volume`

- `POST /volume/adjust` : Adjust volume by a relative amount with optional limits.
  * Request body: `{"value": 2.0}` or `{"value": 2.0, "min": -50.0, "max": 0.0}`
  * Returns the new volume as a float.

- `GET /mute` : Get the main mute state.
  * Returns a boolean.

- `PUT /mute` : Set the main mute state.
  * Request body: `{"value": true}`

- `POST /mute/toggle` : Toggle the main mute state.
  * Returns the new mute state as a boolean.

### Faders

The faders are selected using a 0-based index: 0 for `Main` and 1 to 4 for `Aux1` to `Aux4`.

- `GET /faders` : Get all faders (volume and mute state).
  * Returns a list of objects with `volume` and `mute` properties.

- `GET /faders/{index}/volume` : Get a fader's volume.
  * Returns an object with `index` and `volume` fields.

- `PUT /faders/{index}/volume` : Set a fader's volume in dB.
  * Request body: `{"value": -5.0}`

- `PUT /faders/{index}/volume/external` : Set a fader's volume (external, immediate effect).
  * Request body: `{"value": -5.0}`
  * Special command for use with Loudness filter and external volume control.

- `POST /faders/{index}/volume/adjust` : Adjust a fader's volume by a relative amount.
  * Request body: `{"value": 2.0}` or `{"value": 2.0, "min": -50.0, "max": 0.0}`
  * Returns an object with `index` and `volume` fields.

- `GET /faders/{index}/mute` : Get a fader's mute state.
  * Returns an object with `index` and `mute` fields.

- `PUT /faders/{index}/mute` : Set a fader's mute state.
  * Request body: `{"value": true}`

- `POST /faders/{index}/mute/toggle` : Toggle a fader's mute state.
  * Returns an object with `index` and `mute` fields.

### Signal Levels

All signal level values are in dB, where 0 dB means full level.

- `GET /signal/range` : Get the signal range of the capture device.
  * Returns the value as a float.

- `GET /signal/levels` : Get all signal levels (RMS and peak for playback and capture).
  * Returns an object with `playback_rms`, `playback_peak`, `capture_rms`, `capture_peak` arrays.
  * Optional query parameter `since`: a float (seconds) or `last` for since-last-request.
  * Examples:
    - `GET /signal/levels` — current levels
    - `GET /signal/levels?since=5.0` — levels from last 5 seconds
    - `GET /signal/levels?since=last` — levels since last request

- `GET /signal/peaks/sincestart` : Get peak signal levels since processing started.
  * Returns an object with `playback` and `capture` arrays.

- `POST /signal/peaks/sincestart/reset` : Reset peak signal levels since start.

- `GET /signal/capture/rms` : Get capture signal RMS levels.
  * Optional `since` query parameter.
  * Returns an array of floats.

- `GET /signal/capture/peak` : Get capture signal peak levels.
  * Optional `since` query parameter.
  * Returns an array of floats.

- `GET /signal/playback/rms` : Get playback signal RMS levels.
  * Optional `since` query parameter.
  * Returns an array of floats.

- `GET /signal/playback/peak` : Get playback signal peak levels.
  * Optional `since` query parameter.
  * Returns an array of floats.

### Processing

- `GET /processing/capturerate` : Get the measured capture sample rate.
  * Returns the value as an integer (Hz).

- `GET /processing/updateinterval` : Get the signal level update interval.
  * Returns the value as an integer (ms).

- `PUT /processing/updateinterval` : Set the signal level update interval.
  * Request body: `{"value": 200}`

- `GET /processing/rateadjust` : Get the current rate adjustment factor.
  * Returns the value as a float.

- `GET /processing/bufferlevel` : Get the current playback buffer level.
  * Returns the value as an integer (samples).

- `GET /processing/clippedsamples` : Get the number of clipped samples.
  * Returns the value as an integer.

- `POST /processing/clippedsamples/reset` : Reset the clipped samples counter.

- `GET /processing/load` : Get the current DSP processing load.
  * Returns the value as a float (0.0 to 1.0+).

### Devices

- `GET /devices/supportedtypes` : Get supported audio device backend types.
  * Returns an object with `playback` and `capture` arrays of strings.

- `GET /devices/capture/{backend}` : Get available capture devices for a backend.
  * Returns a list of `[name, description]` pairs.
  * Backend examples: "Alsa", "Pulse", "CoreAudio", "Wasapi".

- `GET /devices/playback/{backend}` : Get available playback devices for a backend.
  * Returns a list of `[name, description]` pairs.

## Example: curl usage

### Get the version
```sh
curl http://localhost:1236/api/v1/version
```
Response:
```json
{"result":"Ok","value":"3.0.1"}
```

### Set the volume
```sh
curl -X PUT -H "Content-Type: application/json" \
  -d '{"value": -15.0}' \
  http://localhost:1236/api/v1/volume
```
Response:
```json
{"result":"Ok"}
```

### Load a new configuration file
```sh
curl -X PUT -H "Content-Type: application/json" \
  -d '{"value": "/path/to/newconfig.yml"}' \
  http://localhost:1236/api/v1/config/filepath

curl -X POST http://localhost:1236/api/v1/reload
```

### Get signal levels from last 5 seconds
```sh
curl "http://localhost:1236/api/v1/signal/levels?since=5.0"
```

### Get available capture devices
```sh
curl http://localhost:1236/api/v1/devices/capture/Alsa
```
