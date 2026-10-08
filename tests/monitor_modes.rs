//! Software-only configuration/pipeline regressions. No device is opened.
use camilladsp::audiochunk::AudioChunk;
use camilladsp::config::{self, ConfigChange, Configuration, MonitorMode};
use camilladsp::filters::Filter;
use camilladsp::filters::basicfilters::Delay;
use camilladsp::filters::fftconv::ConvCoeffCache;
use camilladsp::filters::lookahead_limiter::{LookaheadGain, limiter_parameters};
use camilladsp::pipeline::Pipeline;
use camilladsp::processors::Processor;
use camilladsp::processors::lookahead_limiter::LookaheadLimiter;
use camilladsp::utils::conversions::buffer_to_chunk_rawbytes;
use camilladsp::{CamillaFloat, ProcessingParameters};
use std::sync::Arc;

const MODES: [MonitorMode; 3] = [MonitorMode::Sum, MonitorMode::Max, MonitorMode::Rms];

#[derive(Clone, Copy, Debug)]
enum Kind {
    Compressor,
    NoiseGate,
    Limiter,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Compressor => "Compressor",
            Self::NoiseGate => "NoiseGate",
            Self::Limiter => "LookaheadLimiter",
        }
    }

    fn default_mode(self) -> MonitorMode {
        match self {
            Self::Limiter => MonitorMode::Max,
            _ => MonitorMode::Sum,
        }
    }
}

struct Fixture {
    kind: Kind,
    mode: Option<&'static str>,
    chunk: usize,
    attack: usize,
    release: usize,
    monitor: &'static str,
    process: &'static str,
    delayed_only: bool,
}

impl Fixture {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            mode: None,
            chunk: 8,
            attack: if matches!(kind, Kind::Limiter) { 0 } else { 1 },
            release: if matches!(kind, Kind::Limiter) { 0 } else { 1 },
            monitor: "[2, 3]",
            process: "[0, 1]",
            delayed_only: false,
        }
    }

    fn yaml(&self) -> String {
        let mode = self
            .mode
            .map_or(String::new(), |m| format!("      monitor_mode: {m}\n"));
        let additional = match self.kind {
            Kind::Compressor => "      threshold: -20\n      factor: 2\n".to_string(),
            Kind::NoiseGate => "      threshold: -20\n      attenuation: 40\n".to_string(),
            Kind::Limiter => format!(
                "      limit: 0\n      delay_processed_only: {}\n",
                self.delayed_only
            ),
        };
        format!(
            "devices:
  samplerate: 48000
  chunksize: {chunk}
  volume_ramp_time_ms: 0
  capture:
    type: SignalGenerator
    channels: 4
    signal: {{type: Sine, freq: 1000, level: -12}}
  playback: {{type: Stdout, channels: 4, format: F64_LE}}
processors:
  p:
    type: {kind}
    parameters:
      channels: 4
      monitor_channels: {monitor}
      process_channels: {process}
{mode}      attack: {attack}
      attack_unit: samples
      release: {release}
      release_unit: samples
{additional}pipeline:
  - {{type: Processor, name: p}}
",
            chunk = self.chunk,
            kind = self.kind.name(),
            monitor = self.monitor,
            process = self.process,
            attack = self.attack,
            release = self.release,
        )
    }

    fn config(&self) -> Configuration {
        yaml_serde::from_str(&self.yaml()).unwrap()
    }
}

fn set_mode(conf: &mut Configuration, mode: Option<MonitorMode>) {
    match conf.processors.as_mut().unwrap().get_mut("p").unwrap() {
        config::Processor::Compressor { parameters, .. } => parameters.monitor_mode = mode,
        config::Processor::NoiseGate { parameters, .. } => parameters.monitor_mode = mode,
        config::Processor::LookaheadLimiter { parameters, .. } => parameters.monitor_mode = mode,
        _ => unreachable!(),
    }
}

fn resolved_mode(conf: &Configuration) -> MonitorMode {
    match &conf.processors.as_ref().unwrap()["p"] {
        config::Processor::Compressor { parameters, .. } => parameters.monitor_mode(),
        config::Processor::NoiseGate { parameters, .. } => parameters.monitor_mode(),
        config::Processor::LookaheadLimiter { parameters, .. } => parameters.monitor_mode(),
        _ => unreachable!(),
    }
}

fn build(conf: &Configuration) -> Pipeline {
    let mut validated = conf.clone();
    let _impulses = config::validate_config(&mut validated, None).unwrap();
    Pipeline::from_config(
        validated,
        Arc::new(ProcessingParameters::default()),
        None,
        &mut ConvCoeffCache::new(),
    )
}

fn reload(pipeline: &mut Pipeline, old: &mut Configuration, mut new: Configuration) {
    let _impulses = config::validate_config(&mut new, None).unwrap();
    match config::config_diff(old, &new) {
        ConfigChange::FilterParameters {
            filters,
            processors,
        } => {
            assert!(processors.iter().any(|p| p == "p"));
            pipeline.update_parameters(
                new.clone(),
                &filters,
                &processors,
                &mut ConvCoeffCache::new(),
            );
        }
        ConfigChange::None => {}
        other => panic!("Mode update must not rebuild the pipeline: {other:?}"),
    }
    *old = new;
}

fn chunk(waveforms: Vec<Vec<CamillaFloat>>) -> AudioChunk {
    let frames = waveforms[0].len();
    AudioChunk::new(waveforms, 1.0, -1.0, frames, frames)
}

fn signal(n: usize, left: CamillaFloat, right: CamillaFloat) -> Vec<Vec<CamillaFloat>> {
    vec![vec![0.5; n], vec![-0.25; n], vec![left; n], vec![right; n]]
}

fn close(a: CamillaFloat, b: CamillaFloat) {
    let tolerance = 64.0 * CamillaFloat::EPSILON * a.abs().max(b.abs()).max(1.0);
    assert!(
        (a - b).abs() <= tolerance,
        "{a} != {b}, tolerance {tolerance}"
    );
}

fn equal_waveforms(actual: &[Vec<CamillaFloat>], expected: &[Vec<CamillaFloat>]) {
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(a.len(), b.len());
        for (&a, &b) in a.iter().zip(b) {
            close(a, b);
        }
    }
}

fn check_rms_masked_capture(kind: Kind) {
    const FRAMES: usize = 256;
    let parameters = match kind {
        Kind::Compressor => "threshold: -10\n      factor: 2",
        Kind::NoiseGate => "threshold: -10\n      attenuation: 40",
        Kind::Limiter => "limit: -10",
    };
    // Cover either entry order, repeated masked entries, and an entirely masked
    // monitor selection. Masked entries must still dilute the RMS reading.
    for monitor in ["[0, 1]", "[1, 0]", "[0, 1, 1]", "[1]"] {
        let yaml = format!(
            "devices:
  samplerate: 48000
  chunksize: {FRAMES}
  capture: {{type: Stdin, channels: 2, format: F64_LE}}
  playback: {{type: Stdout, channels: 1, format: F64_LE}}
processors:
  p:
    type: {kind}
    parameters:
      channels: 2
      monitor_channels: {monitor}
      process_channels: [0]
      monitor_mode: Rms
      attack: 1
      attack_unit: samples
      release: 1
      release_unit: samples
      {parameters}
mixers:
  mono:
    channels: {{in: 2, out: 1}}
    mapping:
      - dest: 0
        sources: [{{channel: 0, gain: 0}}]
pipeline:
  - {{type: Processor, name: p}}
  - {{type: Mixer, name: mono}}
",
            kind = kind.name(),
        );
        let mut conf: Configuration = yaml_serde::from_str(&yaml).unwrap();
        config::validate_config(&mut conf, None).unwrap();
        let used_channels = config::used_capture_channels(&conf);
        assert_eq!(used_channels, [true, false]);
        let mut masked_pipeline = build(&conf);
        let mut silent_pipeline = build(&conf);
        for sample in [0.5_f64, 0.0, -0.5] {
            let bytes: Vec<u8> = [sample, 0.0]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .cycle()
                .take(FRAMES * 2 * size_of::<f64>())
                .collect();
            let convert = |mask: &[bool]| {
                buffer_to_chunk_rawbytes(
                    &bytes,
                    2,
                    &config::BinarySampleFormat::F64_LE,
                    bytes.len(),
                    mask,
                    false,
                )
            };
            // Use the same masking and conversion as file/stdin capture.
            let masked = convert(&used_channels);
            assert_eq!(masked.waveforms[0].len(), FRAMES);
            assert!(masked.waveforms[1].is_empty());
            let silent = convert(&[true, true]);
            assert_eq!(silent.waveforms[1], vec![0.0; FRAMES]);

            let expected = silent_pipeline.process_chunk(silent);
            let actual = masked_pipeline.process_chunk(masked);
            // A processing-thread panic can leave the executable's exit status
            // successful, so check that complete, correct audio is produced.
            assert_eq!(actual.frames, FRAMES);
            assert_eq!(actual.valid_frames, FRAMES);
            assert_eq!(actual.channels, 1);
            assert_eq!(actual.waveforms[0].len(), FRAMES);
            equal_waveforms(&actual.waveforms, &expected.waveforms);
        }
    }
}

#[test]
fn compressor_rms_masked_capture_matches_silence() {
    check_rms_masked_capture(Kind::Compressor);
}

#[test]
fn noise_gate_rms_masked_capture_matches_silence() {
    check_rms_masked_capture(Kind::NoiseGate);
}

#[test]
fn limiter_rms_masked_capture_matches_silence() {
    check_rms_masked_capture(Kind::Limiter);
}

#[test]
fn modes_defaults_and_roundtrips() {
    for kind in [Kind::Compressor, Kind::NoiseGate, Kind::Limiter] {
        for mode in [None, Some("null"), Some("Sum"), Some("Max"), Some("Rms")] {
            let mut fixture = Fixture::new(kind);
            fixture.mode = mode;
            let conf = fixture.config();
            let expected = match mode {
                Some("Sum") => MonitorMode::Sum,
                Some("Max") => MonitorMode::Max,
                Some("Rms") => MonitorMode::Rms,
                _ => kind.default_mode(),
            };
            assert_eq!(resolved_mode(&conf), expected);
            let json = serde_json::to_value(&conf).unwrap();
            let property = json.pointer("/processors/p/parameters/monitor_mode");
            if mode.is_none() || mode == Some("null") {
                assert!(property.is_none());
            } else {
                assert_eq!(property.unwrap().as_str(), mode);
            }
            let from_json: Configuration = serde_json::from_value(json).unwrap();
            assert_eq!(from_json, conf);
            let yaml = yaml_serde::to_string(&conf).unwrap();
            let from_yaml: Configuration = yaml_serde::from_str(&yaml).unwrap();
            assert_eq!(from_yaml, conf);
        }
        for bad in ["RMS", "max", "Other", "true", "1", "[]", "{}"] {
            let mut fixture = Fixture::new(kind);
            fixture.mode = Some(bad);
            assert!(yaml_serde::from_str::<Configuration>(&fixture.yaml()).is_err());
        }
    }
    let filter = "type: LookaheadLimiter
parameters:
  limit: 0
  attack: 1
  attack_unit: samples
  release: 1
  release_unit: samples
  monitor_mode: Max
";
    assert!(yaml_serde::from_str::<config::Filter>(filter).is_err());
}

#[test]
fn channel_list_expansion_and_validation() {
    for kind in [Kind::Compressor, Kind::NoiseGate, Kind::Limiter] {
        for list in [None, Some("null"), Some("[]")] {
            let mut fixture = Fixture::new(kind);
            fixture.monitor = list.unwrap_or("null");
            fixture.process = list.unwrap_or("null");
            let mut yaml = fixture.yaml();
            if list.is_none() {
                yaml = yaml
                    .replace("      monitor_channels: null\n", "")
                    .replace("      process_channels: null\n", "");
            }
            let conf: Configuration = yaml_serde::from_str(&yaml).unwrap();
            match conf.processors.as_ref().unwrap()["p"].clone() {
                config::Processor::Compressor { parameters, .. } => {
                    let p = camilladsp::processors::compressor::Compressor::from_config(
                        "p", parameters, 48000, 8,
                    );
                    assert_eq!(p.monitor_channels, [0, 1, 2, 3]);
                    assert_eq!(p.process_channels, [0, 1, 2, 3]);
                }
                config::Processor::NoiseGate { parameters, .. } => {
                    let p = camilladsp::processors::noisegate::NoiseGate::from_config(
                        "p", parameters, 48000, 8,
                    );
                    assert_eq!(p.monitor_channels, [0, 1, 2, 3]);
                    assert_eq!(p.process_channels, [0, 1, 2, 3]);
                }
                config::Processor::LookaheadLimiter { parameters, .. } => {
                    let p = LookaheadLimiter::from_config("p", parameters, 48000, 8);
                    assert_eq!(p.monitor_channels, [0, 1, 2, 3]);
                    assert_eq!(p.process_channels, [0, 1, 2, 3]);
                }
                _ => unreachable!(),
            }
        }
        for invalid_monitor in ["[4]", "[0, 99]"] {
            let mut fixture = Fixture::new(kind);
            fixture.monitor = invalid_monitor;
            assert!(config::validate_config(&mut fixture.config(), None).is_err());
        }
        let mut fixture = Fixture::new(kind);
        fixture.process = "[4]";
        assert!(config::validate_config(&mut fixture.config(), None).is_err());
        fixture.process = "[0, 1]";
        let wrong_count = fixture
            .yaml()
            .replace("      channels: 4", "      channels: 2");
        let mut wrong: Configuration = yaml_serde::from_str(&wrong_count).unwrap();
        assert!(config::validate_config(&mut wrong, None).is_err());
    }
}

#[test]
fn compressor_and_gate_opposite_polarity_and_silence() {
    for kind in [Kind::Compressor, Kind::NoiseGate] {
        for mode in MODES {
            let fixture = Fixture::new(kind);
            let mut conf = fixture.config();
            set_mode(&mut conf, Some(mode));
            let mut pipeline = build(&conf);
            // 128 samples with 1-sample time constants, far from the threshold.
            for _ in 0..16 {
                let out = pipeline.process_chunk(chunk(signal(8, 0.5, -0.5)));
                equal_waveforms(&out.waveforms[2..], &[vec![0.5; 8], vec![-0.5; 8]]);
            }
            let out = pipeline.process_chunk(chunk(signal(8, 0.5, -0.5)));
            let gain = out.waveforms[0][7] / 0.5;
            close(out.waveforms[1][7], -0.25 * gain);
            match (kind, mode) {
                (Kind::Compressor, MonitorMode::Sum) => close(gain, 1.0),
                (Kind::Compressor, _) => assert!(gain < 0.6 && gain > 0.3),
                (Kind::NoiseGate, MonitorMode::Sum) => close(gain, 0.01),
                (Kind::NoiseGate, _) => close(gain, 1.0),
                _ => unreachable!(),
            }
            for _ in 0..16 {
                pipeline.process_chunk(chunk(signal(8, 0.0, 0.0)));
            }
            let quiet = pipeline.process_chunk(chunk(signal(8, 0.0, 0.0)));
            let expected = if matches!(kind, Kind::NoiseGate) {
                0.005
            } else {
                0.5
            };
            close(quiet.waveforms[0][7], expected);
        }
    }
}

#[test]
fn limiter_aggregate_modes_have_documented_limits() {
    for mode in MODES {
        let mut fixture = Fixture::new(Kind::Limiter);
        fixture.monitor = "[0, 1]";
        let mut conf = fixture.config();
        set_mode(&mut conf, Some(mode));
        let mut pipeline = build(&conf);
        let out = pipeline.process_chunk(chunk(vec![
            vec![2.0; 8],
            vec![-2.0; 8],
            vec![0.0; 8],
            vec![0.0; 8],
        ]));
        let expected = if mode == MonitorMode::Sum { 2.0 } else { 1.0 };
        close(out.waveforms[0][7], expected);
        let mut pipeline = build(&conf);
        let out = pipeline.process_chunk(chunk(vec![
            vec![2.0; 8],
            vec![0.0; 8],
            vec![0.0; 8],
            vec![0.0; 8],
        ]));
        let expected = if mode == MonitorMode::Rms {
            CamillaFloat::sqrt(2.0)
        } else {
            1.0
        };
        close(out.waveforms[0][7], expected);
    }
}

/// History-aware reference with independent channel arithmetic, using the
/// unchanged gain and delay primitives. It never resets history on a mode change.
struct ReferenceLimiter {
    gain: LookaheadGain,
    delays: Vec<Delay>,
    delayed_only: bool,
}

impl ReferenceLimiter {
    fn new(fixture: &Fixture) -> Self {
        let (limit, attack, release) = limiter_parameters(
            0.0,
            fixture.attack as f64,
            config::TimeUnit::Samples,
            fixture.release as f64,
            config::TimeUnit::Samples,
            48000,
        );
        Self {
            gain: LookaheadGain::new(limit, attack, release, 48000, fixture.chunk),
            delays: (0..4)
                .map(|_| Delay::new("ref", 48000, attack as f64, false))
                .collect(),
            delayed_only: fixture.delayed_only,
        }
    }

    fn process(
        &mut self,
        mut waveforms: Vec<Vec<CamillaFloat>>,
        mode: MonitorMode,
    ) -> Vec<Vec<CamillaFloat>> {
        let detection: Vec<_> = waveforms[2]
            .iter()
            .zip(&waveforms[3])
            .map(|(&a, &b)| match mode {
                MonitorMode::Sum => a + b,
                MonitorMode::Max => a.abs().max(b.abs()),
                MonitorMode::Rms => ((a * a + b * b) / 2.0).sqrt(),
            })
            .collect();
        self.gain.process_detection(&detection);
        for (index, waveform) in waveforms.iter_mut().enumerate() {
            if !self.delayed_only || index < 2 {
                self.delays[index].process_waveform(waveform);
            }
            if index < 2 {
                for (value, gain) in waveform.iter_mut().zip(self.gain.envelope()) {
                    *value *= gain;
                }
            }
        }
        waveforms
    }
}

#[test]
fn limiter_all_mode_updates_preserve_buffered_audio_history_and_release() {
    for delayed_only in [false, true] {
        for attack in [0, 3, 17] {
            for release in [0, 19] {
                for old_mode in MODES {
                    for new_mode in MODES {
                        let mut fixture = Fixture::new(Kind::Limiter);
                        fixture.attack = attack;
                        fixture.release = release;
                        fixture.delayed_only = delayed_only;
                        let mut conf = fixture.config();
                        set_mode(&mut conf, Some(old_mode));
                        let mut pipeline = build(&conf);
                        let mut reference = ReferenceLimiter::new(&fixture);
                        for block in 0..12 {
                            if block == 3 {
                                let mut next = conf.clone();
                                set_mode(&mut next, Some(new_mode));
                                reload(&mut pipeline, &mut conf, next);
                            }
                            let mode = if block < 3 { old_mode } else { new_mode };
                            let mut samples = signal(8, 0.0, 0.0);
                            // Peak in the final sample just before the update;
                            // another peak afterwards distinguishes all modes.
                            if block == 2 || block == 5 {
                                samples[2][7] = 4.0;
                                samples[3][7] = -2.0;
                            }
                            let expected = reference.process(samples.clone(), mode);
                            let actual = pipeline.process_chunk(chunk(samples));
                            equal_waveforms(&actual.waveforms, &expected);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn effective_default_update_preserves_state_on_all_processors() {
    for kind in [Kind::Compressor, Kind::NoiseGate, Kind::Limiter] {
        let mut fixture = Fixture::new(kind);
        fixture.attack = 17;
        fixture.release = 19;
        let mut conf = fixture.config();
        let mut changing = build(&conf);
        let mut control = build(&conf);
        for block in 0..12 {
            if block == 3 {
                let mut next = conf.clone();
                set_mode(&mut next, Some(kind.default_mode()));
                reload(&mut changing, &mut conf, next);
            }
            let mut samples = signal(8, 0.0, 0.0);
            if block == 2 {
                samples[2][7] = 4.0;
            }
            let expected = control.process_chunk(chunk(samples.clone()));
            let actual = changing.process_chunk(chunk(samples));
            for (a, b) in actual
                .waveforms
                .iter()
                .flatten()
                .zip(expected.waveforms.iter().flatten())
            {
                assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }
}

#[test]
fn single_channel_limiter_matches_filter_for_every_mode() {
    for mode in MODES {
        for attack in [0, 3, 17] {
            let yaml = format!(
                "channels: 1\nlimit: 0\nattack: {attack}\nattack_unit: samples\nrelease: 19\nrelease_unit: samples\n"
            );
            let mut params: config::LookaheadLimiterProcessorParameters =
                yaml_serde::from_str(&yaml).unwrap();
            params.monitor_mode = Some(mode);
            let filter_yaml = yaml.replace("channels: 1\n", "");
            let filter_params: config::LookaheadLimiterParameters =
                yaml_serde::from_str(&filter_yaml).unwrap();
            let mut processor = LookaheadLimiter::from_config("p", params, 48000, 8);
            let mut filter = camilladsp::filters::lookahead_limiter::LookaheadLimiter::from_config(
                "f",
                filter_params,
                48000,
                8,
            );
            for block in 0..12 {
                let mut expected: Vec<CamillaFloat> = (0..8)
                    .map(|i| ((block * 8 + i) as CamillaFloat * 0.31).sin() * 2.0)
                    .collect();
                let mut actual = chunk(vec![expected.clone()]);
                filter.process_waveform(&mut expected);
                processor.process_chunk(&mut actual);
                equal_waveforms(&actual.waveforms, &[expected]);
            }
        }
    }
}

#[test]
fn different_legal_chunk_sizes_produce_the_same_stream() {
    for kind in [Kind::Compressor, Kind::NoiseGate, Kind::Limiter] {
        for mode in MODES {
            let mut outputs = Vec::new();
            for size in [8, 16, 32] {
                let mut fixture = Fixture::new(kind);
                fixture.chunk = size;
                fixture.attack = 17;
                fixture.release = 19;
                let mut conf = fixture.config();
                set_mode(&mut conf, Some(mode));
                let mut pipeline = build(&conf);
                let mut output = vec![Vec::new(); 4];
                // Same 256-sample stream including a silent delayed tail.
                for start in (0..256).step_by(size) {
                    let mut samples = signal(size, 0.0, 0.0);
                    let (first, second) = samples[2..].split_at_mut(1);
                    for (i, (left, right)) in first[0].iter_mut().zip(&mut second[0]).enumerate() {
                        let time = start + i;
                        if time < 128 {
                            *left = (time as CamillaFloat * 0.31).sin() * 4.0;
                            *right = (time as CamillaFloat * 0.17).cos() * 2.0;
                        }
                    }
                    let actual = pipeline.process_chunk(chunk(samples));
                    for (all, current) in output.iter_mut().zip(actual.waveforms) {
                        all.extend(current);
                    }
                }
                outputs.push(output);
            }
            equal_waveforms(&outputs[0], &outputs[1]);
            equal_waveforms(&outputs[0], &outputs[2]);
        }
    }
}

#[test]
fn compressor_gate_mode_changes_take_effect_through_config_reload() {
    for kind in [Kind::Compressor, Kind::NoiseGate] {
        for before in MODES {
            for after in MODES {
                let fixture = Fixture::new(kind);
                let mut conf = fixture.config();
                set_mode(&mut conf, Some(before));
                let mut p = build(&conf);
                for _ in 0..16 {
                    p.process_chunk(chunk(signal(8, 0.5, -0.5)));
                }
                let mut next = conf.clone();
                set_mode(&mut next, Some(after));
                reload(&mut p, &mut conf, next);
                for _ in 0..16 {
                    p.process_chunk(chunk(signal(8, 0.5, -0.5)));
                }
                let out = p.process_chunk(chunk(signal(8, 0.5, -0.5)));
                let gain = out.waveforms[0][7] / 0.5;
                match (kind, after) {
                    (Kind::Compressor, MonitorMode::Sum) => close(gain, 1.0),
                    (Kind::Compressor, _) => assert!(gain > 0.3 && gain < 0.6),
                    (Kind::NoiseGate, MonitorMode::Sum) => close(gain, 0.01),
                    (Kind::NoiseGate, _) => close(gain, 1.0),
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[test]
fn max_limits_all_monitored_processed_channels_at_chunk_boundaries() {
    for attack in [0, 3, 17] {
        let mut fixture = Fixture::new(Kind::Limiter);
        fixture.monitor = "[0, 1]";
        fixture.mode = Some("Max");
        fixture.attack = attack;
        fixture.release = 19;
        let mut p = build(&fixture.config());
        for block in 0..24 {
            let mut waveforms = vec![vec![0.0; 8]; 4];
            if block < 12 {
                let (first, second) = waveforms.split_at_mut(1);
                for (i, (left, right)) in first[0].iter_mut().zip(&mut second[0]).enumerate() {
                    *left = if (block + i) % 2 == 0 { 4.0 } else { -2.0 };
                    *right = if (block + i) % 2 == 0 { -2.0 } else { 4.0 };
                }
            }
            let out = p.process_chunk(chunk(waveforms));
            assert!(
                out.waveforms[..2]
                    .iter()
                    .flatten()
                    .all(|x| x.abs() <= 1.0 + 64.0 * CamillaFloat::EPSILON)
            );
        }
    }
}

#[test]
fn limiter_zero_channels_and_unrelated_processor_property_are_rejected() {
    let p: config::LookaheadLimiterProcessorParameters = yaml_serde::from_str(
        "channels: 0\nlimit: 0\nattack: 3\nattack_unit: samples\nrelease: 19\nrelease_unit: samples\n"
    ).unwrap();
    assert!(
        camilladsp::processors::lookahead_limiter::validate_lookahead_limiter(&p, 48000).is_err()
    );
    let race = "type: RACE\nparameters:\n  channels: 2\n  channel_a: 0\n  channel_b: 1\n  delay: 1\n  delay_unit: samples\n  attenuation: 6\n  monitor_mode: Max\n";
    assert!(yaml_serde::from_str::<config::Processor>(race).is_err());
}

#[test]
fn documented_sidechain_example_validates() {
    let mut conf: Configuration =
        yaml_serde::from_str(include_str!("../exampleconfigs/monitor_sidechain.yml")).unwrap();
    assert!(config::validate_config(&mut conf, None).is_ok());
}
