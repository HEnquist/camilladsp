// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

//! Built-in controller: source format following and error recovery.
//!
//! This module holds the settings and the config selection, which are plain functions
//! of the entry config, the reported source format and the settings. The state machine
//! that runs sessions and decides what happens when one ends is in `engine.rs`.
//!
//! The entry config is whatever the user loaded explicitly, by file, by `SetConfig` and so
//! on. When the capture source changes format, following picks the config to run for the
//! new format from two providers. Specific expands a file name template with the reported
//! format and loads that file. Adapt changes the rate of the entry config, the way the
//! `-r` override does.

use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::config::{self, Configuration};
use crate::filters::fftconv::ImpulseCache;
pub use camilladsp_schema::protocol::{
    AdaptCheck, ControllerFileChecks, ControllerSettings, FileCheck, FollowCapture, SourceFormat,
    TOKEN_CHANNELS, TOKEN_FORMAT, TOKEN_SAMPLERATE, TOKENS,
};

/// How close a reported rate has to be to a standard rate to be snapped to it, relative.
///
/// A rate measured over a short window can be more than 1% off, 44581 for 44100 at a
/// 0.2 s window. The closest standard rates, 44100 and 48000, are 8.8% apart, so 3% still
/// can't snap to the wrong one.
const SNAP_TOLERANCE: f64 = 0.03;

/// What a capture source is doing right now, see `audiodevice::query_capture_source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// The source is active at this format.
    Format(SourceFormat),
    /// The backend can tell that nothing feeds the capture.
    Inactive,
    /// The backend can't tell.
    Unknown,
}

/// A config as it was loaded, before overrides, token expansion and path resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSource {
    pub raw: Configuration,
    /// The file it was read from, used to resolve relative paths.
    pub filename: Option<String>,
}

/// What a loaded config is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    /// A new entry config.
    Entry,
    /// A patched version of the running Specific variant. It runs until the next switch,
    /// and the entry stays as it is.
    Variant,
}

/// A config sent to the controller, along with the result of validating it as is.
///
/// The sender validates before sending, so that a client gets its error directly. The
/// result is reused when the config runs as it is, so that the coefficient files are
/// not read twice.
pub struct LoadedConfig {
    pub source: ConfigSource,
    pub kind: LoadKind,
    pub validated: Configuration,
    pub impulses: ImpulseCache,
}

impl LoadedConfig {
    /// Validate `source` as is, and bundle it for sending to the controller.
    pub fn validate(source: ConfigSource, kind: LoadKind) -> crate::Res<Self> {
        let mut validated = source.raw.clone();
        let impulses = config::validate_config(&mut validated, source.filename.as_deref())?;
        Ok(LoadedConfig {
            source,
            kind,
            validated,
            impulses,
        })
    }

    /// Load a config file and validate it as is.
    pub fn from_file(filename: &str) -> crate::Res<Self> {
        let raw = config::load_config(filename)?;
        Self::validate(
            ConfigSource {
                raw,
                filename: Some(filename.to_string()),
            },
            LoadKind::Entry,
        )
    }
}

/// The raw config behind the running one, which `PatchConfig` and `SetConfigValue` patch.
#[derive(Clone, Debug)]
pub struct RunningSource {
    pub source: ConfigSource,
    pub kind: LoadKind,
}

/// Which provider supplied a config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    /// The entry config as is, no following involved.
    Entry,
    /// The Specific provider, with the file it loaded.
    Specific(String),
    /// The Adapt provider.
    Adapt,
}

/// A config ready to run.
pub struct Selected {
    pub config: Configuration,
    pub impulses: ImpulseCache,
    pub provider: Provider,
    /// The source format following selected it for, if any.
    pub format: Option<SourceFormat>,
    /// The raw config behind it, for patching.
    pub running: RunningSource,
}

/// The outcome of [`select_config`].
pub enum Selection {
    /// A config to run.
    Run(Box<Selected>),
    /// The entry config doesn't validate.
    Invalid(String),
    /// Following found no config for this format.
    NoConfig(SourceFormat),
}

/// Live state of the controller, for `GetControllerStatus`.
#[derive(Clone, Debug, Default)]
pub struct ControllerStatus {
    /// The running Specific variant, `None` when the entry config runs as is or adapted.
    pub active_config_file: Option<String>,
    /// Waiting to retry after an error.
    pub recovering: bool,
    /// Retry attempts since the last session that ran long enough.
    pub attempts: usize,
    /// When the next retry is due, while waiting for it.
    pub next_retry: Option<Instant>,
    /// The format no provider had a config for, while waiting for the source to change.
    pub waiting_for_source: Option<SourceFormat>,
}

/// Controller state shared with the websocket server.
#[derive(Clone, Debug, Default)]
pub struct ControllerShared {
    /// The settings, `None` when the statefile has no controller section.
    pub settings: Arc<Mutex<Option<ControllerSettings>>>,
    pub status: Arc<Mutex<ControllerStatus>>,
    /// The raw config behind the running one, `None` while nothing runs.
    pub running: Arc<Mutex<Option<RunningSource>>>,
    /// The entry config, for checking the Specific files against.
    pub entry: Arc<Mutex<Option<ConfigSource>>>,
    /// Whether the process runs in wait mode, without which the controller does nothing.
    pub wait: bool,
}

impl ControllerShared {
    /// The current settings, with defaults if there are none.
    pub fn settings(&self) -> ControllerSettings {
        self.settings.lock().clone().unwrap_or_default()
    }
}

/// Snap a measured rate to a standard rate within [`SNAP_TOLERANCE`] of it.
///
/// Measured rates come out like 44097, and a config for that doesn't exist.
pub fn snap_rate(rate: usize) -> usize {
    crate::STANDARD_RATES
        .iter()
        .map(|r| *r as usize)
        .find(|r| ((rate as f64 - *r as f64) / *r as f64).abs() <= SNAP_TOLERANCE)
        .unwrap_or(rate)
}

/// The same format, with the rate snapped to a standard rate. See [`snap_rate`].
pub fn snap_format(format: &SourceFormat) -> SourceFormat {
    SourceFormat {
        samplerate: format.samplerate.map(snap_rate),
        channels: format.channels,
        format: format.format.clone(),
    }
}

/// The rate the capture device of a config opens at.
fn capture_rate_of(conf: &Configuration) -> usize {
    if conf.devices.resampler.is_some() {
        conf.devices.capture_samplerate()
    } else {
        conf.devices.samplerate()
    }
}

/// Resolve a template against the directory of the entry config file, if it is relative.
fn resolve_template(template: &str, entry_filename: Option<&str>) -> PathBuf {
    let path = PathBuf::from(template);
    if path.is_absolute() {
        return path;
    }
    let base = entry_filename
        .map(PathBuf::from)
        .and_then(|f| f.canonicalize().ok())
        .and_then(|f| f.parent().map(Path::to_path_buf));
    match base {
        Some(dir) => dir.join(path),
        None => path,
    }
}

/// Expand the tokens of a template with a reported format.
///
/// Returns `None` if the template has a token the format gives no value for. A previous
/// value is never filled in, since that would pick a file for a format the source isn't at.
pub fn expand_template(template: &str, format: &SourceFormat) -> Option<String> {
    let mut expanded = template.to_string();
    if expanded.contains(TOKEN_SAMPLERATE) {
        expanded = expanded.replace(TOKEN_SAMPLERATE, &format.samplerate?.to_string());
    }
    if expanded.contains(TOKEN_CHANNELS) {
        expanded = expanded.replace(TOKEN_CHANNELS, &format.channels?.to_string());
    }
    if expanded.contains(TOKEN_FORMAT) {
        expanded = expanded.replace(TOKEN_FORMAT, format.format.as_deref()?);
    }
    Some(expanded)
}

/// Check a Specific variant against the entry config and the format it was selected for.
///
/// Fields of `format` that are not known are not checked.
pub fn check_variant(
    entry: Option<&Configuration>,
    variant: &Configuration,
    format: &SourceFormat,
) -> Result<(), String> {
    if let Some(entry) = entry {
        let entry_key = entry.devices.capture.device_key();
        let variant_key = variant.devices.capture.device_key();
        if entry_key != variant_key {
            return Err(format!(
                "it captures from {} {:?}, but the entry config captures from {} {:?}",
                variant_key.0,
                variant_key.1.unwrap_or_default(),
                entry_key.0,
                entry_key.1.unwrap_or_default()
            ));
        }
    }
    if let Some(expected) = format.samplerate {
        let rate = capture_rate_of(variant);
        if rate != expected {
            return Err(format!("its capture rate is {rate}, expected {expected}"));
        }
    }
    if let Some(channels) = format.channels {
        let capture_channels = variant.devices.capture.channels();
        if capture_channels != channels {
            return Err(format!(
                "it has {capture_channels} capture channels, expected {channels}"
            ));
        }
    }
    if let Some(fmt) = &format.format
        && variant.devices.capture.format_matches(fmt) == Some(false)
    {
        return Err(format!("its capture format is not {fmt}"));
    }
    Ok(())
}

/// The Specific provider: load the file the template gives for this format.
fn specific_config(
    entry: &ConfigSource,
    template: &str,
    format: &SourceFormat,
) -> Option<Box<Selected>> {
    let Some(expanded) = expand_template(template, format) else {
        info!("Specific: the template '{template}' has a token with no reported value");
        return None;
    };
    let path = resolve_template(&expanded, entry.filename.as_deref());
    let path_str = path.to_string_lossy().to_string();
    if !path.exists() {
        info!("Specific: no config file '{path_str}' for {format}");
        return None;
    }
    let raw = match config::load_config(&path_str) {
        Ok(raw) => raw,
        Err(err) => {
            error!("Specific: could not load '{path_str}': {err}");
            return None;
        }
    };
    if let Err(err) = check_variant(Some(&entry.raw), &raw, format) {
        error!("Specific: not using '{path_str}', {err}");
        return None;
    }
    let mut conf = raw.clone();
    let impulses =
        match config::validate_config_at_rate(&mut conf, Some(&path_str), format.known_rate()) {
            Ok(impulses) => impulses,
            Err(err) => {
                error!("Specific: '{path_str}' is not valid: {err}");
                return None;
            }
        };
    Some(Box::new(Selected {
        config: conf,
        impulses,
        provider: Provider::Specific(path_str.clone()),
        format: Some(format.clone()),
        running: RunningSource {
            source: ConfigSource {
                raw,
                filename: Some(path_str),
            },
            kind: LoadKind::Variant,
        },
    }))
}

/// The Adapt provider: change the rate of the entry config to the reported one.
fn adapt_config(entry: &ConfigSource, format: &SourceFormat) -> Option<Box<Selected>> {
    let Some(rate) = format.known_rate() else {
        info!("Adapt: the source rate is unknown");
        return None;
    };
    let entry_channels = entry.raw.devices.capture.channels();
    if let Some(channels) = format.channels
        && channels != entry_channels
    {
        info!(
            "Adapt: the source has {channels} channels, the entry config captures {entry_channels}"
        );
        return None;
    }
    let mut conf = entry.raw.clone();
    if let Some(fmt) = &format.format
        && conf.devices.capture.format_matches(fmt) == Some(false)
        && let Err(err) = config::set_capture_format_by_name(&mut conf.devices.capture, fmt)
    {
        info!("Adapt: can't change the capture format to {fmt}: {err}");
        return None;
    }
    let impulses =
        match config::validate_config_at_rate(&mut conf, entry.filename.as_deref(), Some(rate)) {
            Ok(impulses) => impulses,
            Err(err) => {
                error!("Adapt: the entry config adapted to {format} is not valid: {err}");
                return None;
            }
        };
    if conf.devices.samplerate() != entry.raw.devices.samplerate()
        && let Some(problem) = rate_change_problem(&entry.raw)
    {
        warn!(
            "Adapt: can't change the samplerate to {}, {problem}",
            conf.devices.samplerate()
        );
        return None;
    }
    Some(Box::new(Selected {
        config: conf,
        impulses,
        provider: Provider::Adapt,
        format: Some(format.clone()),
        running: RunningSource {
            source: entry.clone(),
            kind: LoadKind::Entry,
        },
    }))
}

/// Why Adapt can't run a config at another samplerate than its own, `None` if it can.
/// Only matters where Adapt changes `samplerate`, a resampler keeps the config's rate.
fn rate_change_problem(raw: &Configuration) -> Option<String> {
    let settings = config::rate_dependent_settings(raw);
    if settings.is_empty() {
        return None;
    }
    let rate = raw.devices.samplerate();
    Some(format!(
        "these settings are only right at {rate} Hz: {}. Give delays, attack and release times in a time unit rather than in samples, add $samplerate$ to the coefficient file names, or set samplerate to [{rate}] to resample every other rate to {rate} Hz",
        settings.join(", ")
    ))
}

/// Select the config to run, for an entry config and the last known source format.
///
/// `format` is `None` when following is off or no format is known, and then the entry
/// runs as is. `validated` is the entry already validated as is, reused if given.
pub fn select_config(
    entry: &ConfigSource,
    format: Option<&SourceFormat>,
    settings: &ControllerSettings,
    validated: Option<(Configuration, ImpulseCache)>,
) -> Selection {
    let running = RunningSource {
        source: entry.clone(),
        kind: LoadKind::Entry,
    };
    let Some(format) = format else {
        let (config, impulses) = match validated {
            Some(v) => v,
            None => {
                let mut conf = entry.raw.clone();
                match config::validate_config(&mut conf, entry.filename.as_deref()) {
                    Ok(impulses) => (conf, impulses),
                    Err(err) => return Selection::Invalid(err.to_string()),
                }
            }
        };
        return Selection::Run(Box::new(Selected {
            config,
            impulses,
            provider: Provider::Entry,
            format: None,
            running,
        }));
    };
    let format = snap_format(format);
    if let Some(template) = settings.specific_template()
        && let Some(selected) = specific_config(entry, template, &format)
    {
        return Selection::Run(selected);
    }
    if settings.adapt_enabled()
        && let Some(selected) = adapt_config(entry, &format)
    {
        return Selection::Run(selected);
    }
    Selection::NoConfig(format)
}

/// Validate a patched Specific variant, at the rate it was selected for.
pub fn select_variant(
    loaded: LoadedConfig,
    format: Option<&SourceFormat>,
) -> Result<Box<Selected>, String> {
    let rate = format.and_then(SourceFormat::known_rate);
    let mut conf = loaded.source.raw.clone();
    let impulses =
        config::validate_config_at_rate(&mut conf, loaded.source.filename.as_deref(), rate)
            .map_err(|e| e.to_string())?;
    let file = loaded.source.filename.clone().unwrap_or_default();
    Ok(Box::new(Selected {
        config: conf,
        impulses,
        provider: Provider::Specific(file),
        format: format.cloned(),
        running: RunningSource {
            source: loaded.source,
            kind: LoadKind::Variant,
        },
    }))
}

/// Token values captured while matching a file name against a template.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TokenValues {
    samplerate: Option<String>,
    channels: Option<String>,
    format: Option<String>,
}

impl TokenValues {
    fn get_mut(&mut self, token: &str) -> &mut Option<String> {
        match token {
            TOKEN_SAMPLERATE => &mut self.samplerate,
            TOKEN_CHANNELS => &mut self.channels,
            _ => &mut self.format,
        }
    }
}

/// The leading token in `pattern`, if it starts with one.
fn leading_token(pattern: &str) -> Option<&'static str> {
    TOKENS.into_iter().find(|t| pattern.starts_with(t))
}

/// Whether `c` can be part of the value of `token`.
fn token_char(token: &str, c: char) -> bool {
    match token {
        TOKEN_FORMAT => c.is_ascii_alphanumeric() || c == '_',
        _ => c.is_ascii_digit(),
    }
}

/// Match a file name against one component of a template, capturing the token values.
/// A token that appears twice has to have the same value both times.
fn match_component(pattern: &str, name: &str, values: &TokenValues) -> Option<TokenValues> {
    if pattern.is_empty() {
        return name.is_empty().then(|| values.clone());
    }
    if let Some(token) = leading_token(pattern) {
        let rest = &pattern[token.len()..];
        let max_len = name.chars().take_while(|c| token_char(token, *c)).count();
        // Try the longest value first, backing off until the rest matches.
        for len in (1..=max_len).rev() {
            let value = &name[..len];
            let mut candidate = values.clone();
            let slot = candidate.get_mut(token);
            match slot {
                Some(existing) if existing != value => continue,
                _ => *slot = Some(value.to_string()),
            }
            if let Some(found) = match_component(rest, &name[len..], &candidate) {
                return Some(found);
            }
        }
        return None;
    }
    let mut chars = pattern.chars();
    let first = chars.next()?;
    if name.starts_with(first) {
        match_component(chars.as_str(), &name[first.len_utf8()..], values)
    } else {
        None
    }
}

/// Find every file a template can match, with the token values each one gives.
fn find_template_files(template: &Path) -> Vec<(PathBuf, TokenValues)> {
    let mut found = vec![(PathBuf::new(), TokenValues::default())];
    for component in template.components() {
        let part = component.as_os_str().to_string_lossy();
        let has_token = TOKENS.iter().any(|t| part.contains(t));
        let mut next = Vec::new();
        for (base, values) in found {
            if !has_token {
                next.push((base.join(component.as_os_str()), values));
                continue;
            }
            let dir = if base.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                base.clone()
            };
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(matched) = match_component(&part, &name, &values) {
                    next.push((base.join(&name), matched));
                }
            }
        }
        found = next;
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .collect()
}

/// Check every file a Specific template can match against the entry config.
///
/// Each token is a wildcard, and the values read back from a file name give the format
/// the file is checked against, as in [`check_variant`].
///
/// The entry config file should be one of the files, the one for the format it is written
/// for, so that Specific selects it like any other. If it isn't, that is reported too.
pub fn preflight(template: &str, entry: Option<&ConfigSource>) -> Vec<FileCheck> {
    let mut checks = check_template_files(template, entry);
    checks.extend(check_entry_name(template, entry, &checks));
    checks
}

/// Check the configs each enabled follow provider can use, see [`ControllerFileChecks`].
pub fn check_files(
    settings: &ControllerSettings,
    entry: Option<&ConfigSource>,
) -> ControllerFileChecks {
    ControllerFileChecks {
        specific: settings
            .specific_template()
            .map(|template| preflight(template, entry)),
        adapt: settings.adapt_enabled().then(|| check_adapt(entry)),
    }
}

/// Check the entry config that Adapt starts from, and read the format it is written for.
///
/// The format is read after validation, so that it includes the command line overrides.
fn check_adapt(entry: Option<&ConfigSource>) -> AdaptCheck {
    let Some(entry) = entry else {
        return AdaptCheck {
            file: None,
            samplerate: None,
            samplerates: None,
            channels: None,
            format: None,
            problem: Some("no config is loaded".to_string()),
        };
    };
    let mut conf = entry.raw.clone();
    // Adapt changes `samplerate` for a rate in a list other than the first, and for any
    // other rate when the config has no resampler.
    let changes_rate = match entry.raw.devices.samplerate_list() {
        Some(rates) => rates.len() > 1,
        None => entry.raw.devices.resampler.is_none(),
    };
    let problem = match config::validate_config(&mut conf, entry.filename.as_deref()) {
        Err(err) => Some(err.to_string()),
        Ok(_) if changes_rate => rate_change_problem(&entry.raw)
            .map(|problem| format!("Adapt can't change the samplerate, {problem}")),
        Ok(_) => None,
    };
    AdaptCheck {
        file: entry.filename.clone(),
        samplerate: Some(capture_rate_of(&conf)),
        // From the config as written, since a `-r` override turns the list into one rate.
        samplerates: entry
            .raw
            .devices
            .samplerate_list()
            .map(|rates| rates.iter().map(|r| r.get()).collect()),
        channels: Some(conf.devices.capture.channels()),
        format: conf.devices.capture.format_name(),
        problem,
    }
}

/// Report an entry config file that isn't one of the files the template matches.
fn check_entry_name(
    template: &str,
    entry: Option<&ConfigSource>,
    checks: &[FileCheck],
) -> Option<FileCheck> {
    let entry_file = entry?.filename.as_ref()?;
    let canonical = |f: &str| PathBuf::from(f).canonicalize().ok();
    let entry_path = canonical(entry_file)?;
    if checks
        .iter()
        .any(|c| canonical(&c.file).as_ref() == Some(&entry_path))
    {
        return None;
    }
    Some(FileCheck {
        file: entry_file.clone(),
        samplerate: None,
        channels: None,
        format: None,
        problem: Some(format!(
            "the entry config is not named by the template '{template}', so Specific can't select it"
        )),
    })
}

fn check_template_files(template: &str, entry: Option<&ConfigSource>) -> Vec<FileCheck> {
    let path = resolve_template(template, entry.and_then(|e| e.filename.as_deref()));
    find_template_files(&path)
        .into_iter()
        .map(|(file, values)| {
            let file_str = file.to_string_lossy().to_string();
            let samplerate = values.samplerate.as_ref().and_then(|v| v.parse().ok());
            let channels = values.channels.as_ref().and_then(|v| v.parse().ok());
            let problem = check_file(
                &file_str,
                entry,
                samplerate,
                channels,
                values.format.as_deref(),
            );
            FileCheck {
                file: file_str,
                samplerate,
                channels,
                format: values.format,
                problem,
            }
        })
        .collect()
}

fn check_file(
    file: &str,
    entry: Option<&ConfigSource>,
    samplerate: Option<usize>,
    channels: Option<usize>,
    format: Option<&str>,
) -> Option<String> {
    let raw = match config::load_config(file) {
        Ok(raw) => raw,
        Err(err) => return Some(err.to_string()),
    };
    if let Some(name) = format
        && !raw.devices.capture.is_format_name(name)
    {
        return Some(format!("'{name}' is not a known sample format"));
    }
    let source_format = SourceFormat {
        samplerate,
        channels,
        format: format.map(str::to_string),
    };
    if let Err(err) = check_variant(entry.map(|e| &e.raw), &raw, &source_format) {
        return Some(err);
    }
    let mut conf = raw;
    if let Err(err) = config::validate_config_at_rate(
        &mut conf,
        Some(file),
        samplerate.and_then(NonZeroUsize::new),
    ) {
        return Some(err.to_string());
    }
    None
}

/// Log the problems [`preflight`] finds, as warnings.
pub fn log_preflight(template: &str, entry: Option<&ConfigSource>) {
    let checks = check_template_files(template, entry);
    if checks.is_empty() {
        warn!("Specific: no files match the template '{template}'");
    }
    if let Some(check) = check_entry_name(template, entry, &checks) {
        warn!(
            "Specific: '{}': {}",
            check.file,
            check.problem.unwrap_or_default()
        );
    }
    for check in checks {
        match &check.problem {
            Some(problem) => warn!("Specific: '{}' can't be used, {problem}", check.file),
            None => debug!("Specific: '{}' is ok", check.file),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CaptureDevice;

    fn settings(specific: Option<&str>, adapt: Option<bool>) -> ControllerSettings {
        ControllerSettings {
            follow_capture: Some(FollowCapture {
                specific: specific.map(str::to_string),
                adapt,
            }),
            error_recovery: None,
        }
    }

    fn base_config(samplerate: usize, channels: usize, extra: &str) -> Configuration {
        // Validation opens the capture file, so it has to exist on every platform.
        let capture_file = format!("{}/testdata/int32.raw", env!("CARGO_MANIFEST_DIR"));
        let yaml = format!(
            "devices:
  samplerate: {samplerate}
  chunksize: 1024
  capture:
    type: RawFile
    channels: {channels}
    filename: '{capture_file}'
    format: S32_LE
  playback:
    type: File
    channels: {channels}
    filename: /dev/null
    format: S32_LE
{extra}"
        );
        yaml_serde::from_str(&yaml).unwrap()
    }

    fn entry(conf: Configuration) -> ConfigSource {
        ConfigSource {
            raw: conf,
            filename: None,
        }
    }

    fn write_config(dir: &Path, name: &str, conf: &Configuration) -> String {
        let path = dir.join(name);
        std::fs::write(&path, yaml_serde::to_string(conf).unwrap()).unwrap();
        path.to_string_lossy().to_string()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cdsp_controller_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn format(samplerate: usize, channels: Option<usize>) -> SourceFormat {
        SourceFormat {
            samplerate: Some(samplerate),
            channels,
            format: None,
        }
    }

    fn unknown_rate(channels: Option<usize>) -> SourceFormat {
        SourceFormat {
            samplerate: None,
            ..format(0, channels)
        }
    }

    fn run(selection: Selection) -> Box<Selected> {
        match selection {
            Selection::Run(selected) => selected,
            Selection::Invalid(err) => panic!("invalid: {err}"),
            Selection::NoConfig(f) => panic!("no config for {f}"),
        }
    }

    #[test]
    fn settings_validation() {
        assert!(ControllerSettings::default().validate().is_ok());
        assert!(settings(None, Some(true)).validate().is_ok());
        assert!(
            settings(Some("conf_$samplerate$.yml"), None)
                .validate()
                .is_ok()
        );
        // No provider.
        assert!(settings(None, None).validate().is_err());
        assert!(settings(None, Some(false)).validate().is_err());
        // A template without a token.
        assert!(settings(Some("conf.yml"), Some(true)).validate().is_err());
        let sanitized = settings(Some("conf.yml"), None).sanitized();
        assert_eq!(sanitized.follow_capture, None);
        assert!(!sanitized.following_enabled());
    }

    #[test]
    fn rates_snap_to_standard_rates() {
        assert_eq!(snap_rate(44097), 44100);
        assert_eq!(snap_rate(47900), 48000);
        assert_eq!(snap_rate(44581), 44100);
        assert_eq!(snap_rate(96000), 96000);
        // Far from any standard rate, kept as is.
        assert_eq!(snap_rate(60000), 60000);
    }

    #[test]
    fn template_expansion() {
        let full = SourceFormat {
            samplerate: Some(96000),
            channels: Some(4),
            format: Some("S24_3_LE".to_string()),
        };
        assert_eq!(
            expand_template("c_$samplerate$_$channels$_$format$.yml", &full),
            Some("c_96000_4_S24_3_LE.yml".to_string())
        );
        // A token without a value gives nothing.
        assert_eq!(
            expand_template("c_$channels$.yml", &format(96000, None)),
            None
        );
        assert_eq!(
            expand_template("c_$samplerate$.yml", &unknown_rate(Some(2))),
            None
        );
        assert_eq!(
            expand_template("c_$samplerate$.yml", &format(44100, None)),
            Some("c_44100.yml".to_string())
        );
    }

    #[test]
    fn relative_templates_resolve_against_the_entry_file() {
        let dir = temp_dir("relative");
        let entry_file = write_config(&dir, "entry.yml", &base_config(48000, 2, ""));
        let resolved = resolve_template("conf_$samplerate$.yml", Some(&entry_file));
        assert_eq!(
            resolved,
            dir.canonicalize().unwrap().join("conf_$samplerate$.yml")
        );
        // An absolute path needs a drive letter on Windows, so build one from the temp dir.
        let absolute = std::env::temp_dir().join("abs").join("conf.yml");
        let resolved = resolve_template(&absolute.to_string_lossy(), Some(&entry_file));
        assert_eq!(resolved, absolute);
    }

    #[test]
    fn without_a_format_the_entry_runs_as_is() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            None,
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.provider, Provider::Entry);
        assert_eq!(selected.config.devices.samplerate(), 48000);
    }

    #[test]
    fn an_invalid_entry_is_reported() {
        let conf = base_config(48000, 2, "  target_level: 100000\n");
        assert!(matches!(
            select_config(&entry(conf), None, &settings(None, Some(true)), None),
            Selection::Invalid(_)
        ));
    }

    #[test]
    fn adapt_without_resampler_changes_rate_and_chunksize() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(96000, Some(2))),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.provider, Provider::Adapt);
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.chunksize(), 2048);
    }

    #[test]
    fn adapt_with_resampler_changes_capture_rate() {
        let extra = "  resampler:\n    type: Synchronous\n  capture_samplerate: 44100\n";
        let selected = run(select_config(
            &entry(base_config(48000, 2, extra)),
            Some(&format(96000, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 48000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert!(selected.config.devices.resampler.is_some());
        // At 1:1 without rate adjust, the resampler is dropped.
        let selected = run(select_config(
            &entry(base_config(48000, 2, extra)),
            Some(&format(48000, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert!(selected.config.devices.resampler.is_none());
    }

    #[test]
    fn adapt_snaps_a_measured_rate() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(44097, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 44100);
        assert_eq!(selected.format.unwrap().samplerate, Some(44100));
    }

    #[test]
    fn adapt_expands_tokens_at_the_new_rate() {
        let dir = temp_dir("adapt_tokens");
        for rate in [48000, 96000] {
            std::fs::write(dir.join(format!("filter_{rate}.txt")), "1.0\n0.0\n").unwrap();
        }
        let extra = format!(
            "filters:
  fir:
    type: Conv
    parameters:
      type: Raw
      format: TEXT
      filename: {}/filter_$samplerate$.txt
pipeline:
  - type: Filter
    channels: [0]
    names: [fir]
",
            dir.to_string_lossy()
        );
        let selected = run(select_config(
            &entry(base_config(48000, 2, &extra)),
            Some(&format(96000, None)),
            &settings(None, Some(true)),
            None,
        ));
        let filters = selected.config.filters.unwrap();
        let config::Filter::Conv {
            parameters: config::ConvParameters::Raw(params),
            ..
        } = &filters["fir"]
        else {
            panic!("not a raw conv");
        };
        assert!(params.filename.ends_with("filter_96000.txt"));
    }

    #[test]
    fn adapt_gives_nothing_for_unknown_rate_or_changed_channels() {
        let e = entry(base_config(48000, 2, ""));
        let s = settings(None, Some(true));
        assert!(matches!(
            select_config(&e, Some(&unknown_rate(None)), &s, None),
            Selection::NoConfig(_)
        ));
        assert!(matches!(
            select_config(&e, Some(&format(96000, Some(4))), &s, None),
            Selection::NoConfig(_)
        ));
    }

    #[test]
    fn adapt_changes_an_explicit_format() {
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&SourceFormat {
                samplerate: Some(48000),
                channels: None,
                format: Some("S16_LE".to_string()),
            }),
            &settings(None, Some(true)),
            None,
        ));
        let CaptureDevice::RawFile(dev) = &selected.config.devices.capture else {
            panic!("not a raw file");
        };
        assert_eq!(dev.format, config::BinarySampleFormat::S16_LE);
    }

    #[test]
    fn adapt_refuses_a_format_the_backend_lacks() {
        let selected = select_config(
            &entry(base_config(48000, 2, "")),
            Some(&SourceFormat {
                samplerate: Some(48000),
                channels: None,
                format: Some("S24".to_string()),
            }),
            &settings(None, Some(true)),
            None,
        );
        assert!(matches!(selected, Selection::NoConfig(_)));
    }

    #[test]
    fn specific_only_and_both_providers() {
        let dir = temp_dir("specific");
        write_config(&dir, "conf_96000_2.yml", &base_config(96000, 2, ""));
        let template = format!("{}/conf_$samplerate$_$channels$.yml", dir.to_string_lossy());
        let e = entry(base_config(48000, 2, ""));

        // Specific finds the file.
        let selected = run(select_config(
            &e,
            Some(&format(96000, Some(2))),
            &settings(Some(&template), None),
            None,
        ));
        assert!(
            matches!(selected.provider, Provider::Specific(ref f) if f.ends_with("conf_96000_2.yml"))
        );
        assert_eq!(selected.running.kind, LoadKind::Variant);

        // A missing file with Specific only gives no config.
        assert!(matches!(
            select_config(
                &e,
                Some(&format(44100, Some(2))),
                &settings(Some(&template), None),
                None
            ),
            Selection::NoConfig(_)
        ));
        // An unknown token value gives no config.
        assert!(matches!(
            select_config(
                &e,
                Some(&format(96000, None)),
                &settings(Some(&template), None),
                None
            ),
            Selection::NoConfig(_)
        ));
        // With both, a missing file falls back to Adapt.
        let selected = run(select_config(
            &e,
            Some(&format(44100, Some(2))),
            &settings(Some(&template), Some(true)),
            None,
        ));
        assert_eq!(selected.provider, Provider::Adapt);
        assert_eq!(selected.config.devices.samplerate(), 44100);
    }

    #[test]
    fn the_entry_is_one_of_the_specific_files() {
        let dir = temp_dir("entry_format");
        let template = format!("{}/conf_$samplerate$.yml", dir.to_string_lossy());
        let s = settings(Some(&template), None);
        let fmt = format(48000, Some(2));

        // An entry outside the template gets no special treatment: Specific alone has
        // no file for its rate, and the preflight says why.
        let outside = write_config(&dir, "entry.yml", &base_config(48000, 2, ""));
        let e = ConfigSource {
            raw: base_config(48000, 2, ""),
            filename: Some(outside.clone()),
        };
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::NoConfig(_)
        ));
        let checks = preflight(&template, Some(&e));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].file, outside);
        assert!(checks[0].problem.as_ref().unwrap().contains("template"));

        // Named by the template, it is selected for its own rate like any other file.
        let inside = write_config(&dir, "conf_48000.yml", &base_config(48000, 2, ""));
        let e = ConfigSource {
            raw: base_config(48000, 2, ""),
            filename: Some(inside),
        };
        let selected = run(select_config(&e, Some(&fmt), &s, None));
        assert!(
            matches!(selected.provider, Provider::Specific(ref f) if f.ends_with("conf_48000.yml"))
        );
        let checks = preflight(&template, Some(&e));
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].problem, None);
    }

    #[test]
    fn specific_rejects_files_that_fail_the_checks() {
        let dir = temp_dir("specific_checks");
        let template = format!("{}/conf_$samplerate$_$channels$.yml", dir.to_string_lossy());
        let e = entry(base_config(48000, 2, ""));
        let s = settings(Some(&template), None);
        let fmt = format(96000, Some(2));
        let path = dir.join("conf_96000_2.yml");

        // Invalid.
        std::fs::write(&path, "not: a config").unwrap();
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::NoConfig(_)
        ));
        // Wrong rate.
        write_config(&dir, "conf_96000_2.yml", &base_config(44100, 2, ""));
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::NoConfig(_)
        ));
        // Wrong channels.
        write_config(&dir, "conf_96000_2.yml", &base_config(96000, 4, ""));
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::NoConfig(_)
        ));
        // Another capture device.
        let mut other = base_config(96000, 2, "");
        if let CaptureDevice::RawFile(dev) = &mut other.devices.capture {
            dev.filename = "/dev/urandom".to_string();
        }
        write_config(&dir, "conf_96000_2.yml", &other);
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::NoConfig(_)
        ));
        // Different labels are fine.
        let mut labelled = base_config(96000, 2, "");
        if let CaptureDevice::RawFile(dev) = &mut labelled.devices.capture {
            dev.labels = Some(vec![Some("L".to_string()), Some("R".to_string())]);
        }
        write_config(&dir, "conf_96000_2.yml", &labelled);
        assert!(matches!(
            select_config(&e, Some(&fmt), &s, None),
            Selection::Run(_)
        ));
    }

    #[test]
    fn variant_check_on_format() {
        let variant = base_config(48000, 2, "");
        let mut fmt = format(48000, Some(2));
        fmt.format = Some("S16_LE".to_string());
        // The raw file capture has an explicit S32_LE format.
        assert!(check_variant(None, &variant, &fmt).is_err());
        fmt.format = Some("S32_LE".to_string());
        assert!(check_variant(None, &variant, &fmt).is_ok());
    }

    #[test]
    fn components_match_with_token_values() {
        let empty = TokenValues::default();
        let m = match_component(
            "conf_$samplerate$_$channels$.yml",
            "conf_96000_4.yml",
            &empty,
        )
        .unwrap();
        assert_eq!(m.samplerate.as_deref(), Some("96000"));
        assert_eq!(m.channels.as_deref(), Some("4"));
        assert!(match_component("conf_$samplerate$.yml", "conf_.yml", &empty).is_none());
        assert!(match_component("conf_$samplerate$.yml", "conf_abc.yml", &empty).is_none());
        assert!(match_component("conf_$samplerate$.yml", "other.yml", &empty).is_none());
        // Adjacent tokens backtrack until the rest matches. The split is ambiguous, so
        // only check that one was found.
        let m = match_component("c$channels$$samplerate$.yml", "c244100.yml", &empty).unwrap();
        assert!(m.channels.is_some() && m.samplerate.is_some());
        let m = match_component("c$channels$x$samplerate$", "c2x44100", &empty).unwrap();
        assert_eq!(m.channels.as_deref(), Some("2"));
        // A repeated token must have the same value.
        assert!(match_component("$channels$_$channels$", "2_2", &empty).is_some());
        assert!(match_component("$channels$_$channels$", "2_4", &empty).is_none());
        let m = match_component("c_$format$.yml", "c_S24_3_LE.yml", &empty).unwrap();
        assert_eq!(m.format.as_deref(), Some("S24_3_LE"));
    }

    #[test]
    fn preflight_reports_each_file() {
        let dir = temp_dir("preflight");
        write_config(&dir, "conf_96000_2.yml", &base_config(96000, 2, ""));
        write_config(&dir, "conf_44100_2.yml", &base_config(48000, 2, ""));
        write_config(&dir, "conf_48000_4.yml", &base_config(48000, 4, ""));
        std::fs::write(dir.join("conf_88200_2.yml"), "broken").unwrap();
        std::fs::write(dir.join("unrelated.yml"), "broken").unwrap();
        let template = format!("{}/conf_$samplerate$_$channels$.yml", dir.to_string_lossy());
        let e = entry(base_config(48000, 2, ""));
        let checks = preflight(&template, Some(&e));
        assert_eq!(checks.len(), 4);
        let by_name = |name: &str| {
            checks
                .iter()
                .find(|c| c.file.ends_with(name))
                .unwrap()
                .clone()
        };
        assert_eq!(by_name("conf_96000_2.yml").problem, None);
        assert_eq!(by_name("conf_96000_2.yml").samplerate, Some(96000));
        assert_eq!(by_name("conf_48000_4.yml").problem, None);
        assert_eq!(by_name("conf_48000_4.yml").channels, Some(4));
        assert!(
            by_name("conf_44100_2.yml")
                .problem
                .unwrap()
                .contains("rate")
        );
        assert!(by_name("conf_88200_2.yml").problem.is_some());
    }

    #[test]
    fn preflight_handles_tokens_in_directories() {
        let dir = temp_dir("preflight_dirs");
        for rate in [44100, 96000] {
            let sub = dir.join(rate.to_string());
            std::fs::create_dir_all(&sub).unwrap();
            write_config(&sub, "conf.yml", &base_config(rate, 2, ""));
        }
        let template = format!("{}/$samplerate$/conf.yml", dir.to_string_lossy());
        let checks = preflight(&template, None);
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|c| c.problem.is_none()));
    }

    #[test]
    fn check_files_has_a_section_per_enabled_provider() {
        let dir = temp_dir("check_files");
        write_config(&dir, "conf_44100.yml", &base_config(44100, 2, ""));
        let template = format!("{}/conf_$samplerate$.yml", dir.to_string_lossy());
        let e = entry(base_config(44100, 2, ""));

        let adapt_only = check_files(&settings(None, Some(true)), Some(&e));
        assert_eq!(adapt_only.specific, None);
        assert_eq!(
            adapt_only.adapt,
            Some(AdaptCheck {
                file: None,
                samplerate: Some(44100),
                samplerates: None,
                channels: Some(2),
                format: Some("S32_LE".to_string()),
                problem: None,
            })
        );

        let specific_only = check_files(&settings(Some(&template), None), Some(&e));
        assert_eq!(specific_only.adapt, None);
        assert_eq!(specific_only.specific.unwrap().len(), 1);

        let both = check_files(&settings(Some(&template), Some(true)), Some(&e));
        assert!(both.specific.is_some() && both.adapt.is_some());

        let no_entry = check_files(&settings(None, Some(true)), None);
        let adapt = no_entry.adapt.unwrap();
        assert_eq!(adapt.samplerate, None);
        assert!(adapt.problem.is_some());

        let off = check_files(&ControllerSettings::default(), Some(&e));
        assert_eq!((off.specific, off.adapt), (None, None));
    }

    fn with_rate_list(mut conf: Configuration, rates: &[usize]) -> Configuration {
        conf.devices.samplerate = config::SampleRates::List(
            rates
                .iter()
                .map(|r| NonZeroUsize::new(*r).unwrap())
                .collect(),
        );
        conf
    }

    #[test]
    fn a_listed_rate_runs_natively() {
        let extra = "  resampler:\n    type: Synchronous\n";
        let conf = with_rate_list(base_config(48000, 2, extra), &[48000, 44100, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(96000, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert_eq!(selected.config.devices.chunksize(), 2048);
        assert_eq!(selected.config.devices.resampler, None);
    }

    #[test]
    fn a_listed_rate_keeps_the_resampler_for_rate_adjust() {
        let extra = "  resampler:\n    type: Slip\n  enable_rate_adjust: true\n";
        let conf = with_rate_list(base_config(48000, 2, extra), &[48000, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(96000, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.capture_samplerate(), 96000);
        assert_eq!(
            selected.config.devices.resampler,
            Some(config::Resampler::Slip)
        );
    }

    #[test]
    fn an_unlisted_rate_is_resampled_to_the_first() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[48000, 96000]);
        let selected = run(select_config(
            &entry(conf),
            Some(&format(44100, None)),
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 48000);
        assert_eq!(selected.config.devices.capture_samplerate(), 44100);
        assert_eq!(selected.config.devices.chunksize(), 1024);
        assert_eq!(
            selected.config.devices.resampler,
            Some(config::Resampler::Synchronous)
        );
    }

    #[test]
    fn a_rate_list_starts_at_the_first_rate() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[96000, 48000]);
        let selected = run(select_config(
            &entry(conf),
            None,
            &settings(None, Some(true)),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.resampler, None);
    }

    #[test]
    fn without_adapt_a_rate_list_runs_at_the_first_rate() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[96000, 48000]);
        let selected = run(select_config(
            &entry(conf.clone()),
            None,
            &ControllerSettings::default(),
            None,
        ));
        assert_eq!(selected.config.devices.samplerate(), 96000);

        // A Specific file with a list is selected for its first rate.
        let dir = temp_dir("rate_list");
        write_config(&dir, "conf_96000.yml", &conf);
        let template = format!("{}/conf_$samplerate$.yml", dir.to_string_lossy());
        let selected = run(select_config(
            &entry(base_config(48000, 2, "")),
            Some(&format(96000, None)),
            &settings(Some(&template), None),
            None,
        ));
        assert!(matches!(selected.provider, Provider::Specific(_)));
        assert_eq!(selected.config.devices.samplerate(), 96000);
        assert_eq!(selected.config.devices.resampler, None);
    }

    #[test]
    fn a_rate_list_parses_and_rejects_an_empty_one() {
        let parsed = |rates: &str| {
            let yaml = format!(
                "samplerate: {rates}\nchunksize: 1024\ncapture: {{type: Stdin, channels: 2, format: S16_LE}}\nplayback: {{type: Stdout, channels: 2, format: S16_LE}}\n"
            );
            yaml_serde::from_str::<config::Devices>(&yaml)
        };
        assert_eq!(parsed("44100").unwrap().samplerate(), 44100);
        let list = parsed("[96000, 44100]").unwrap();
        assert_eq!(list.samplerate(), 96000);
        assert_eq!(list.samplerate_list().unwrap().len(), 2);
        assert!(parsed("[]").is_err());
    }

    const DELAY_IN_SAMPLES: &str = "filters:
  delay:
    type: Delay
    parameters: {delay: 10, delay_unit: samples}
pipeline:
  - type: Filter
    channels: [0]
    names: [delay]
";

    fn adapt_runs(conf: Configuration, rate: usize) -> bool {
        match select_config(
            &entry(conf),
            Some(&format(rate, None)),
            &settings(None, Some(true)),
            None,
        ) {
            Selection::Run(_) => true,
            Selection::NoConfig(_) => false,
            Selection::Invalid(err) => panic!("invalid: {err}"),
        }
    }

    #[test]
    fn adapt_refuses_to_change_the_rate_of_rate_dependent_settings() {
        let conf = base_config(48000, 2, DELAY_IN_SAMPLES);
        assert!(!adapt_runs(conf.clone(), 96000));
        // At its own rate nothing changes.
        assert!(adapt_runs(conf, 48000));
        // A listed rate other than the first runs natively, at another rate.
        let listed = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000, 96000]);
        assert!(!adapt_runs(listed, 96000));
    }

    #[test]
    fn rate_dependent_settings_run_when_the_source_is_resampled() {
        let extra = format!("  resampler:\n    type: Synchronous\n{DELAY_IN_SAMPLES}");
        assert!(adapt_runs(base_config(48000, 2, &extra), 96000));
        let single = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000]);
        assert!(adapt_runs(single, 96000));
        let listed = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000, 96000]);
        assert!(adapt_runs(listed, 44100));
    }

    #[test]
    fn check_files_reports_rate_dependent_settings() {
        let problem = |conf| {
            check_files(&settings(None, Some(true)), Some(&entry(conf)))
                .adapt
                .unwrap()
                .problem
        };
        let reported = problem(base_config(48000, 2, DELAY_IN_SAMPLES)).unwrap();
        assert!(
            reported.contains("filter 'delay': delay in samples"),
            "{reported}"
        );
        let extra = format!("  resampler:\n    type: Synchronous\n{DELAY_IN_SAMPLES}");
        assert_eq!(problem(base_config(48000, 2, &extra)), None);
        let single = with_rate_list(base_config(48000, 2, DELAY_IN_SAMPLES), &[48000]);
        assert_eq!(problem(single), None);
    }

    #[test]
    fn check_files_lists_the_native_rates() {
        let conf = with_rate_list(base_config(48000, 2, ""), &[96000, 48000]);
        let checks = check_files(&settings(None, Some(true)), Some(&entry(conf)));
        let adapt = checks.adapt.unwrap();
        assert_eq!(adapt.samplerate, Some(96000));
        assert_eq!(adapt.samplerates, Some(vec![96000, 48000]));
        assert_eq!(adapt.problem, None);
    }
}
