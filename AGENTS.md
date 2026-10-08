# Instructions for AI coding agents

CamillaDSP is a cross-platform audio processing engine written in Rust. It captures audio from a
device, runs it through a pipeline of filters, mixers and processors defined in a YAML config, and
plays the result on another device in realtime. A websocket server allows monitoring and control,
and the GUI and other tools rely on it.

These rules describe the house style of CamillaDSP. They apply to human contributors too.
Read the [Contributing](README.md#contributing) section of the README first. Where these rules say
to ask, an agent should stop and ask its user, who can then check with the maintainer in an issue.

## Project layout

- `src/filters/`, `src/processors/`, `src/mixer.rs`: the DSP.
- `src/*_backend/`: audio backends. `src/utils/`: shared helpers for resampling, sample format
  conversion, timing and rate adjust. Check these before backend-specific code.
- `camilladsp-schema/`: config types, validation and the websocket protocol. It is a separate crate,
  also used by the GUI.
- Unit tests live in `#[cfg(test)] mod tests` at the bottom of each source file. Do not add a
  `tests/` directory, a separate test file or a test harness.
- User docs: `README.md`, `backend_*.md`, `websocket.md` and the other Markdown files in the root.
  The changelog is `CHANGELOG.md`.

## Scope

- Keep the diff as small as the change allows, and do only what the issue or PR is about.
- Fix problems where they originate, not with a workaround where they show up. If the root cause is
  out of scope, point it out instead of working around it.
- Do not add new infrastructure without asking first. That includes test directories and harnesses,
  CI jobs, dependencies and example configs.
- Add a benchmark only when it brings real value, for example when the change is about performance
  and the numbers decide between options. Most changes do not need one.
- Do not guard against audio sample values that cannot occur in practice. Samples are nowhere near
  floating point overflow. Config values are different, they still need validation in
  `camilladsp-schema`.
- Before finishing, go through the diff and ask for each part: is this needed, can it be simpler?
  Repeat until cutting more would remove something of real value.

## Breaking changes

CamillaDSP has many existing users, and every breaking change has a real cost for them. Configs need
updating, and scripts and tools that use the websocket API or the crates stop working.

- Do not rename, restructure or change existing behaviour just because the result would be nicer or
  more consistent.
- If possible, make new config properties optional, with a default that keeps the existing
  behaviour.
- If a breaking change really seems needed, raise it in an issue first.

## Code

- Read the surrounding code first and match it: structure, naming, comment density, idiom.
- Do not allocate or lock in the per-chunk processing path of filters, processors and mixers.
  Allocate buffers in the constructor and on config updates.
- Setup and coefficient math run in `f64`, processing runs in `CamillaFloat`, and values that are
  only reported (levels, spectrum) are `f32`. Convert to `CamillaFloat` once, where a finished value
  is stored for per-sample use. Use the `ToCamillaFloat` and `ToF32` traits, not `as` casts.
  `src/filters/biquad.rs` is the reference.
- Tests go in the existing `#[cfg(test)] mod tests` at the bottom of the file being changed. Test the
  new behaviour with realistic values, not unchanged behaviour or extreme edge cases.
- Optional config fields are `Option<T>` with `#[serde(default)]`, without `skip_serializing_if`,
  and the default is resolved in a getter on the parameters struct. Config types and validation
  live in `camilladsp-schema`.

## Docs

- Follow the style and length of the surrounding docs, both in the Markdown files and in the doc
  comments in the code.
- A new parameter gets a line in the parameter list of its README section. Add a short paragraph
  only when the behaviour needs explaining.
- Do not reword existing docs unless they are wrong.
- No disclaimers about what is not guaranteed or has not been validated.
- Wrap Markdown prose at around 100 columns.
- Changelog entries are one short line per change, under the version the change targets, or under
  an `Unreleased` heading if that is not decided yet.

## Commits and pull requests

- Commit subjects are plain imperative sentences, like "Add monitor mode to the compressor", with no
  `feat:` style prefix.
- Keep PR descriptions short: one summary line and a few bullets. No test logs.
- Open an issue and agree on the approach before starting any larger change.
- Ask which branch to target before starting. It depends on the change and on where the project
  is in its release cycle.

## Validation

Run in this order and stop at the first failure:

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

The `--workspace` flag is needed, otherwise `camilladsp-schema` is skipped. The sample precision is
a rustc cfg, not a feature. When a change touches sample arithmetic, also run the tests with
`RUSTFLAGS="--cfg camillafloat_f32"`.
