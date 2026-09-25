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

// WASAPI device capability probing.
//
// WASAPI does not expose a structured capability API. The only way to
// discover what an exclusive-mode device supports is to call
// `IsFormatSupported` for every combination of sample rate, channel
// count, sample format, and channel mask. A naive brute-force scan
// over the full matrix would issue thousands of COM calls and take
// several seconds per device, so this module uses a staged strategy
// to prune the search space as quickly as possible:
//
// 1. Probe the 48-kHz and 44.1-kHz rate families interleaved from
//    base rate upward. The very first successful rate establishes
//    an upper channel-count limit and a reduced sample-format set
//    that all later probes reuse.
//
// 2. Within each rate sweep, narrow the sample-format candidate list
//    as soon as the first channel count succeeds with fewer formats
//    than the full candidate set.
//
// 3. Cache each channel count's accepted channel mask and reuse it
//    for subsequent probes at other rates, avoiding expensive mask
//    renegotiation.
//
// 4. Apply per-family early cutoff: once a family has produced at
//    least one hit, a miss at the next higher rate deactivates that
//    family. When both families are inactive the upward scan stops.
//
// 5. Probe the remaining low and 32-kHz family rates using only the
//    channel counts discovered during the upward scan (or the full
//    range if nothing was found).
//
// These heuristics dramatically reduce probe time on typical hardware
// while still covering multi-channel and high-rate devices. Because
// the probing is heuristic, unusual devices may have valid
// configurations that fall outside the probed combinations.

use crate::Res;
use crate::config::WasapiSampleFormat;

use super::device::get_supported_wave_format_with_channel_mask;
use wasapi::DeviceCollection;

pub fn list_device_names(input: bool) -> Vec<(String, String)> {
    let direction = if input {
        wasapi::Direction::Capture
    } else {
        wasapi::Direction::Render
    };
    let _ = wasapi::initialize_mta();
    let enumerator = wasapi::DeviceEnumerator::new();

    let names = enumerator
        .map(|en| {
            en.get_device_collection(&direction)
                .map(|coll| list_device_names_in_collection(&coll).unwrap_or_default())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    names
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect()
}

/// Convert a `WasapiSampleFormat` to the canonical string used in YAML configs.
fn wasapi_format_to_str(fmt: WasapiSampleFormat) -> &'static str {
    match fmt {
        WasapiSampleFormat::S16 => "S16",
        WasapiSampleFormat::S24 => "S24",
        WasapiSampleFormat::S32 => "S32",
        WasapiSampleFormat::F32 => "F32",
    }
}

pub(super) fn list_device_names_in_collection(collection: &DeviceCollection) -> Res<Vec<String>> {
    let mut names = Vec::new();
    let count = collection.get_nbr_devices()?;
    for index in 0..count {
        let device = collection.get_device_at_index(index)?;
        let name = device.get_friendlyname()?;
        names.push(name);
    }
    Ok(names)
}

type CapabilitiesMap =
    std::collections::HashMap<usize, std::collections::HashMap<usize, Vec<String>>>;
type ChannelMaskMap = std::collections::HashMap<usize, u32>;

// Standard rates in each family, from base rate upward through multiples.
const FAMILY_48_RATES: &[usize] = &[48_000, 96_000, 192_000, 384_000, 768_000];
const FAMILY_44_RATES: &[usize] = &[44_100, 88_200, 176_400, 352_800, 705_600];
const FAMILY_NAMES: &[&str] = &["48-kHz family", "44.1-kHz family"];

// Sub-multiples and 32-kHz family rates, probed unconditionally.
const REMAINING_RATES: &[usize] = &[
    24_000, 12_000, 6_000, 22_050, 11_025, 5_512, 16_000, 8_000, 32_000, 64_000,
];

const EXCLUSIVE_SAMPLE_FORMATS: &[WasapiSampleFormat] = &[
    WasapiSampleFormat::S16,
    WasapiSampleFormat::S24,
    WasapiSampleFormat::S32,
    WasapiSampleFormat::F32,
];

#[derive(Default)]
struct RateProbeResult {
    max_supported_channels: Option<usize>,
    supported_formats: Vec<WasapiSampleFormat>,
}

fn format_labels(formats: &[WasapiSampleFormat]) -> Vec<&'static str> {
    formats
        .iter()
        .map(|fmt| wasapi_format_to_str(*fmt))
        .collect()
}

fn supported_exclusive_formats(
    audio_client: &wasapi::AudioClient,
    samplerate: usize,
    channels: usize,
    candidate_formats: &[WasapiSampleFormat],
    channel_masks: &mut ChannelMaskMap,
) -> Vec<WasapiSampleFormat> {
    let mut formats = Vec::new();
    let mut preferred_mask = channel_masks.get(&channels).copied();
    if let Some(_channel_mask) = preferred_mask {
        xtrace!(
            "WASAPI capability probe: probing {samplerate} Hz, {channels} ch using cached channel mask {_channel_mask:#010x}."
        );
    }
    for &fmt in candidate_formats {
        xtrace!("WASAPI capability probe: testing {samplerate} Hz, {channels} ch, format {fmt:?}.");
        if let Ok((wave_format, _)) = get_supported_wave_format_with_channel_mask(
            audio_client,
            &fmt,
            samplerate,
            channels,
            &wasapi::ShareMode::Exclusive,
            preferred_mask,
        ) {
            let channel_mask = wave_format.get_dwchannelmask();
            xtrace!(
                "WASAPI capability probe: supported {samplerate} Hz, {channels} ch, format {fmt:?}."
            );
            let previous_mask = channel_masks.insert(channels, channel_mask);
            if previous_mask != Some(channel_mask) {
                xtrace!(
                    "WASAPI capability probe: channel count {channels} will use channel mask {channel_mask:#010x} for subsequent probes."
                );
            }
            preferred_mask = Some(channel_mask);
            formats.push(fmt);
        } else {
            xtrace!(
                "WASAPI capability probe: unsupported {samplerate} Hz, {channels} ch, format {fmt:?}."
            );
        }
    }
    formats
}

/// Convert the intermediate capabilities map into the public sorted capability list.
fn capabilities_from_map(map: CapabilitiesMap) -> Vec<crate::ChannelCapability> {
    let mut capabilities: Vec<crate::ChannelCapability> = map
        .into_iter()
        .map(|(channels, rate_map)| {
            let mut samplerates: Vec<crate::SamplerateCapability> = rate_map
                .into_iter()
                .map(|(samplerate, formats)| crate::SamplerateCapability {
                    samplerate,
                    formats,
                })
                .collect();
            samplerates.sort_unstable_by_key(|s| s.samplerate);
            crate::ChannelCapability {
                channels,
                samplerates,
            }
        })
        .collect();
    capabilities.sort_unstable_by_key(|c| c.channels);
    capabilities
}

/// Probe one sample rate for the provided channel counts.
/// Returns the highest supported channel count and the union of supported formats at that rate.
fn probe_and_store_rate_with_candidates<I>(
    capabilities_map: &mut CapabilitiesMap,
    audio_client: &wasapi::AudioClient,
    samplerate: usize,
    channel_counts: I,
    candidate_formats: &[WasapiSampleFormat],
    channel_masks: &mut ChannelMaskMap,
) -> RateProbeResult
where
    I: IntoIterator<Item = usize>,
{
    trace!(
        "WASAPI capability probe: probing {samplerate} Hz using sample formats {:?}.",
        format_labels(candidate_formats)
    );
    let mut result = RateProbeResult::default();
    let mut narrowed_formats: Option<Vec<WasapiSampleFormat>> = None;
    for channels in channel_counts {
        let active_formats = narrowed_formats.as_deref().unwrap_or(candidate_formats);
        xtrace!("WASAPI capability probe: probing {samplerate} Hz, {channels} channels.");
        let formats = supported_exclusive_formats(
            audio_client,
            samplerate,
            channels,
            active_formats,
            channel_masks,
        );
        if !formats.is_empty() {
            let format_labels = format_labels(&formats);
            xtrace!(
                "WASAPI capability probe: found support at {samplerate} Hz, {channels} ch with formats {format_labels:?}."
            );
            if narrowed_formats.is_none() && formats.len() < candidate_formats.len() {
                debug!(
                    "WASAPI capability probe: narrowing sample formats for the rest of the {samplerate} Hz sweep to {format_labels:?}."
                );
                narrowed_formats = Some(formats.clone());
            }
            result.max_supported_channels = Some(channels);
            for &fmt in &formats {
                if !result.supported_formats.contains(&fmt) {
                    result.supported_formats.push(fmt);
                }
            }
            capabilities_map.entry(channels).or_default().insert(
                samplerate,
                format_labels
                    .into_iter()
                    .map(|fmt| fmt.to_string())
                    .collect(),
            );
        } else {
            xtrace!(
                "WASAPI capability probe: no supported formats at {samplerate} Hz, {channels} ch."
            );
        }
    }
    if let Some(channels) = result.max_supported_channels {
        trace!(
            "WASAPI capability probe: highest supported channel count at {samplerate} Hz is {channels}."
        );
    } else {
        trace!("WASAPI capability probe: no support found at {samplerate} Hz.");
    }
    result
}

pub fn get_device_capabilities(
    device_name: &str,
    input: bool,
) -> Result<crate::AudioDeviceDescriptor, crate::DeviceError> {
    let direction = if input {
        wasapi::Direction::Capture
    } else {
        wasapi::Direction::Render
    };
    let _ = wasapi::initialize_mta();

    let enumerator = match wasapi::DeviceEnumerator::new() {
        Ok(e) => e,
        Err(_) => {
            return Err(crate::DeviceError::Other(
                "Failed to initialize DeviceEnumerator".to_string(),
            ));
        }
    };

    let collection = match enumerator.get_device_collection(&direction) {
        Ok(c) => c,
        Err(_) => {
            return Err(crate::DeviceError::Other(
                "Failed to get device collection".to_string(),
            ));
        }
    };

    let count = collection.get_nbr_devices().unwrap_or(0);
    let mut target_device = None;

    for index in 0..count {
        if let Ok(device) = collection.get_device_at_index(index)
            && let Ok(name) = device.get_friendlyname()
            && name == device_name
        {
            target_device = Some(device);
            break;
        }
    }

    let device = match target_device {
        Some(device) => device,
        None => {
            return Err(crate::DeviceError::DeviceNotFound(device_name.to_string()));
        }
    };

    let audio_client = match device.get_iaudioclient() {
        Ok(client) => client,
        Err(err) => {
            return Err(crate::DeviceError::Other(format!("{err}")));
        }
    };

    debug!(
        "WASAPI capability probe: starting capability scan for device {device_name:?}, input={input}."
    );

    let mut capability_sets = Vec::new();

    // --- Shared mode: use GetMixFormat as the sole authoritative descriptor ---
    // WASAPI shared mode operates through the audio engine at a single fixed mix
    // format; probing a synthetic channel/rate grid would misrepresent what the
    // shared path can actually honour (CamillaDSP uses autoconvert=false).
    if let Ok(mix_fmt) = audio_client.get_mixformat() {
        let channels = mix_fmt.get_nchannels() as usize;
        let rate = mix_fmt.get_samplespersec() as usize;
        let fmt = WasapiSampleFormat::F32;
        debug!(
            "WASAPI capability probe: shared mode mix format is {rate} Hz, {channels} ch, format {fmt:?}."
        );
        let shared_caps = vec![crate::ChannelCapability {
            channels,
            samplerates: vec![crate::SamplerateCapability {
                samplerate: rate,
                formats: vec![wasapi_format_to_str(fmt).to_string()],
            }],
        }];
        capability_sets.push(crate::DeviceCapabilitySet {
            mode: crate::CapabilityMode::Shared,
            capabilities: shared_caps,
        });
    }

    // --- Exclusive mode: probe independently with a generous channel ceiling ---
    // GetMixFormat describes the shared-mode engine format and is not a valid upper
    // bound for exclusive-mode channel support. Probe up to MAX_EXCLUSIVE_CHANNELS
    // so that e.g. 8-channel or multi-channel exclusive devices are not under-reported.
    const MAX_EXCLUSIVE_CHANNELS: usize = 32;
    let mut exclusive_capabilities_map: CapabilitiesMap = std::collections::HashMap::new();
    debug!(
        "WASAPI capability probe: starting exclusive-mode scan with channel ceiling {MAX_EXCLUSIVE_CHANNELS}."
    );

    // Probe the two main families interleaved from base rate upward.
    // The first hit at any rate establishes the channel limit for all
    // subsequent probes. Per-family early cutoff stops a family after
    // the first miss that follows a hit.
    let families = [FAMILY_48_RATES, FAMILY_44_RATES];
    let mut channel_limit = 0usize;
    let mut hit = [false; 2];
    let mut active = [true; 2];
    let mut learned_formats: Option<Vec<WasapiSampleFormat>> = None;
    let mut learned_channel_masks: ChannelMaskMap = ChannelMaskMap::new();

    for i in 0..FAMILY_48_RATES.len().max(FAMILY_44_RATES.len()) {
        if active.iter().all(|is_active| !is_active) {
            debug!(
                "WASAPI capability probe: stopping upward family scan early because all families are inactive."
            );
            break;
        }
        for (f, family) in families.iter().enumerate() {
            if !active[f] {
                continue;
            }
            if let Some(&rate) = family.get(i) {
                let limit = if channel_limit > 0 {
                    channel_limit
                } else {
                    MAX_EXCLUSIVE_CHANNELS
                };
                trace!(
                    "WASAPI capability probe: probing {} rate {} Hz with channel limit {}.",
                    FAMILY_NAMES[f], rate, limit
                );
                let candidate_formats = learned_formats
                    .as_deref()
                    .unwrap_or(EXCLUSIVE_SAMPLE_FORMATS);
                let result = probe_and_store_rate_with_candidates(
                    &mut exclusive_capabilities_map,
                    &audio_client,
                    rate,
                    1..=limit,
                    candidate_formats,
                    &mut learned_channel_masks,
                );
                if let Some(ch) = result.max_supported_channels {
                    hit[f] = true;
                    let old_limit = channel_limit;
                    channel_limit = channel_limit.max(ch);
                    debug!(
                        "WASAPI capability probe: {} rate {} Hz succeeded with max {} channels; channel limit changed from {} to {}.",
                        FAMILY_NAMES[f], rate, ch, old_limit, channel_limit
                    );
                    if learned_formats.is_none() {
                        debug!(
                            "WASAPI capability probe: learned supported sample formats {:?} from the first successful rate {}; reusing them for subsequent probes.",
                            format_labels(&result.supported_formats),
                            rate
                        );
                        learned_formats = Some(result.supported_formats);
                    }
                } else if hit[f] {
                    active[f] = false;
                    debug!(
                        "WASAPI capability probe: stopping {} after miss at {} Hz following earlier hits.",
                        FAMILY_NAMES[f], rate
                    );
                } else {
                    trace!(
                        "WASAPI capability probe: {} rate {} Hz had no hits; keeping family active until the first success is found.",
                        FAMILY_NAMES[f], rate
                    );
                }
            }
        }
    }

    // Probe sub-multiples and 32-kHz family rates.
    // Reuse discovered channel counts when available, otherwise probe the full range.
    let remaining_channel_counts: Vec<usize> = {
        let mut ch: Vec<usize> = exclusive_capabilities_map.keys().copied().collect();
        ch.sort_unstable();
        if ch.is_empty() {
            debug!(
                "WASAPI capability probe: probing remaining low-rate set with full channel range because no channel counts were discovered in the upward scan."
            );
            (1..=MAX_EXCLUSIVE_CHANNELS).collect()
        } else {
            debug!(
                "WASAPI capability probe: probing remaining low-rate set using previously discovered channel counts {ch:?}."
            );
            ch
        }
    };
    for &rate in REMAINING_RATES {
        trace!("WASAPI capability probe: probing remaining rate {rate} Hz.");
        let candidate_formats = learned_formats
            .as_deref()
            .unwrap_or(EXCLUSIVE_SAMPLE_FORMATS);
        let result = probe_and_store_rate_with_candidates(
            &mut exclusive_capabilities_map,
            &audio_client,
            rate,
            remaining_channel_counts.iter().copied(),
            candidate_formats,
            &mut learned_channel_masks,
        );
        if learned_formats.is_none() && result.max_supported_channels.is_some() {
            debug!(
                "WASAPI capability probe: learned supported sample formats {:?} from the first successful rate {}; reusing them for subsequent probes.",
                format_labels(&result.supported_formats),
                rate
            );
            learned_formats = Some(result.supported_formats);
        }
    }

    let exclusive_caps = capabilities_from_map(exclusive_capabilities_map);
    if !exclusive_caps.is_empty() {
        debug!(
            "WASAPI capability probe: exclusive-mode scan found {} channel capability entries.",
            exclusive_caps.len()
        );
        capability_sets.push(crate::DeviceCapabilitySet {
            mode: crate::CapabilityMode::Exclusive,
            capabilities: exclusive_caps,
        });
    } else {
        debug!("WASAPI capability probe: exclusive-mode scan found no supported combinations.");
    }

    debug!("WASAPI capability probe: completed capability scan for device {device_name:?}.");

    Ok(crate::AudioDeviceDescriptor {
        name: device_name.to_string(),
        description: device_name.to_string(),
        capability_sets,
    })
}
