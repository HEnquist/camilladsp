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
// Shared mode is described by the mix format alone. Exclusive mode is probed
// with the `CapabilityProbe` of the wasapi crate, which narrows the search
// with the ranges the driver declares, and falls back to a heuristic staged
// scan for drivers that declare none. See its documentation for the details.

use std::collections::BTreeMap;

use crate::Res;
use crate::config::WasapiSampleFormat;

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

/// Map a format accepted by the device to the `WasapiSampleFormat` that selects it.
/// Both 24 bit layouts, packed in three bytes and padded in four, are S24.
fn wasapi_format_from_wave_format(wave_format: &wasapi::WaveFormat) -> Option<WasapiSampleFormat> {
    let storebits = wave_format.get_bitspersample();
    let validbits = wave_format.get_validbitspersample();
    match (wave_format.get_subformat().ok()?, storebits, validbits) {
        (wasapi::SampleType::Int, 16, 16) => Some(WasapiSampleFormat::S16),
        (wasapi::SampleType::Int, 24 | 32, 24) => Some(WasapiSampleFormat::S24),
        (wasapi::SampleType::Int, 32, 32) => Some(WasapiSampleFormat::S32),
        (wasapi::SampleType::Float, 32, 32) => Some(WasapiSampleFormat::F32),
        _ => None,
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

/// Group the accepted formats by channel count and sample rate,
/// into the public sorted capability list.
fn capabilities_from_wave_formats(
    wave_formats: &[wasapi::WaveFormat],
) -> Vec<crate::ChannelCapability> {
    let mut map: BTreeMap<usize, BTreeMap<usize, Vec<String>>> = BTreeMap::new();
    for wave_format in wave_formats {
        let Some(fmt) = wasapi_format_from_wave_format(wave_format) else {
            debug!("WASAPI capability probe: ignoring unexpected format {wave_format:?}.");
            continue;
        };
        let formats = map
            .entry(wave_format.get_nchannels() as usize)
            .or_default()
            .entry(wave_format.get_samplespersec() as usize)
            .or_default();
        let label = wasapi_format_to_str(fmt).to_string();
        if !formats.contains(&label) {
            formats.push(label);
        }
    }
    map.into_iter()
        .map(|(channels, rate_map)| crate::ChannelCapability {
            channels,
            samplerates: rate_map
                .into_iter()
                .map(|(samplerate, formats)| crate::SamplerateCapability {
                    samplerate,
                    formats,
                })
                .collect(),
        })
        .collect()
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

    // --- Exclusive mode: probe independently of the mix format ---
    // GetMixFormat describes the shared-mode engine format and is not a valid upper
    // bound for exclusive-mode support.
    let mut probe = match wasapi::CapabilityProbe::new(&device) {
        Ok(probe) => probe,
        Err(err) => {
            return Err(crate::DeviceError::Other(format!("{err}")));
        }
    };
    debug!(
        "WASAPI capability probe: starting exclusive-mode scan, the driver declares {} data ranges.",
        probe.data_ranges().len()
    );
    let exclusive_caps = capabilities_from_wave_formats(&probe.supported_formats_all_rates());
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
