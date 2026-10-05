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

use crate::config::{AlsaSampleFormat, BinarySampleFormat};
use crate::controller::{SourceFormat, SourceState};
use crate::{CaptureStatus, PlaybackStatus, Res, StatusMessage};
use alsa::card::Iter;
use alsa::ctl::{Ctl, DeviceIter, ElemId, ElemIface, ElemType, ElemValue};
use alsa::device_name::HintIter;
use alsa::hctl::{Elem, HCtl};
use alsa::pcm::{Format, HwParams};
use alsa::{Card, Direction};
use alsa_sys;
use nix::errno::Errno;
use parking_lot::RwLock;
use std::error;
use std::ffi::CString;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::ProcessingParameters;

use crate::STANDARD_RATES;

const CHANNEL_LIST_LIMIT: u32 = 32;
const CAPABILITY_PROBE_CHANNEL_LIMIT: u32 = 128;

#[derive(Debug)]
pub enum SupportedValues {
    Range(u32, u32),
    Discrete(Vec<u32>),
}

pub struct CaptureParams {
    pub channels: usize,
    pub sample_format: BinarySampleFormat,
    pub silence_timeout: f64,
    pub silence_threshold: f64,
    pub chunksize: usize,
    pub store_bytes_per_sample: usize,
    pub bytes_per_frame: usize,
    pub samplerate: usize,
    pub capture_samplerate: usize,
    pub async_src: bool,
    pub capture_status: Arc<RwLock<CaptureStatus>>,
    pub stop_on_rate_change: bool,
    pub rate_measure_interval: f32,
    pub stop_on_inactive: bool,
    /// The controller follows the source format, see `audiodevice::new_capture_device`.
    pub follow: bool,
    pub link_volume_control: Option<String>,
    pub link_mute_control: Option<String>,
    pub linked_volume_value: Option<f32>,
    pub linked_mute_value: Option<bool>,
}

pub struct PlaybackParams {
    pub channels: usize,
    pub target_level: usize,
    pub adjust_period: f32,
    pub adjust_enabled: bool,
    pub sample_format: BinarySampleFormat,
    pub playback_status: Arc<RwLock<PlaybackStatus>>,
    pub bytes_per_frame: usize,
    pub samplerate: usize,
    pub chunksize: usize,
}

pub enum CaptureResult {
    Normal,
    Stalled,
    Done,
    /// The source changed format, and the capture has to stop for it.
    FormatChange(SourceFormat),
}

/// A capture open that stopped early while following, since the device only offers a
/// format other than the one asked for. Carries the format it offers.
#[derive(Debug)]
pub struct FormatPinned(pub SourceFormat);

impl fmt::Display for FormatPinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the capture device only offers {}", self.0)
    }
}

impl error::Error for FormatPinned {}

/// The rate, channel count and sample format a set of hw params allows, each one only
/// where it allows a single value.
///
/// A loopback capture opened while its playback end runs is held to the playback's
/// params, and so is any device locked to an external clock, so this is how such a
/// device tells what its source is doing.
#[derive(Debug)]
pub struct PinnedFormat {
    pub rate: Option<u32>,
    pub channels: Option<u32>,
    pub format: Option<Format>,
}

impl PinnedFormat {
    pub fn of(hwp: &HwParams) -> Self {
        let single = |min: alsa::Result<u32>, max: alsa::Result<u32>| match (min, max) {
            (Ok(min), Ok(max)) if min == max => Some(min),
            _ => None,
        };
        PinnedFormat {
            rate: single(hwp.get_rate_min(), hwp.get_rate_max()),
            channels: single(hwp.get_channels_min(), hwp.get_channels_max()),
            format: hwp.get_format().ok(),
        }
    }

    /// The pinned values that differ from the requested ones, as a format to follow.
    /// `None` if nothing is pinned to another value. A pinned sample format only counts
    /// when one is requested, an automatic one takes whatever the device has.
    pub fn differs_from(
        &self,
        rate: u32,
        channels: u32,
        format: &Option<AlsaSampleFormat>,
    ) -> Option<SourceFormat> {
        let rate_differs = self.rate.is_some_and(|r| r != rate);
        let channels_differ = self.channels.is_some_and(|c| c != channels);
        let format_differs = match (self.format, format) {
            (Some(pinned), Some(requested)) => {
                alsa_format_name(pinned) != alsa_format_to_str(*requested)
            }
            _ => false,
        };
        (rate_differs || channels_differ || format_differs).then(|| SourceFormat {
            samplerate: self.rate.unwrap_or(rate) as usize,
            channels: self.channels.map(|c| c as usize),
            format: self.format.map(alsa_format_name),
        })
    }

    /// The pinned values as a source format, if at least the rate is pinned.
    pub fn source_format(&self) -> Option<SourceFormat> {
        Some(SourceFormat {
            samplerate: self.rate? as usize,
            channels: self.channels.map(|c| c as usize),
            format: self.format.map(alsa_format_name),
        })
    }
}

/// The name of an ALSA sample format as CamillaDSP configs write it, see
/// [`alsa_format_to_str`]. A format CamillaDSP has no name for keeps the ALSA name.
pub fn alsa_format_name(format: Format) -> String {
    let known = match format {
        Format::S16LE => Some(AlsaSampleFormat::S16_LE),
        Format::S243LE => Some(AlsaSampleFormat::S24_3_LE),
        Format::S24LE => Some(AlsaSampleFormat::S24_4_LE),
        Format::S32LE => Some(AlsaSampleFormat::S32_LE),
        Format::FloatLE => Some(AlsaSampleFormat::F32_LE),
        Format::Float64LE => Some(AlsaSampleFormat::F64_LE),
        _ => None,
    };
    match known {
        Some(fmt) => alsa_format_to_str(fmt).to_string(),
        None => format.to_string(),
    }
}

/// An ALSA sample format from its kernel number, as a `PCM Slave Format` control gives it.
fn format_from_number(number: i32) -> Option<Format> {
    Format::all().iter().copied().find(|f| *f as i32 == number)
}

/// Sets `PCM Notify` on the loopback cable of a capture while following, so that a
/// player can start at any format and the kernel stops the capture for it, rather than
/// holding the player to the capture's format. Puts the old value back when dropped.
///
/// Opening anything but a loopback gives `None`, since only the loopback has the control.
pub struct LoopbackNotify {
    ctl: Ctl,
    id: ElemId,
}

static KERNEL_HINT: std::sync::Once = std::sync::Once::new();

impl LoopbackNotify {
    pub fn enable(pcm: &alsa::PCM) -> Option<Self> {
        let info = pcm.info().ok()?;
        let card = info.get_card();
        if card < 0 {
            return None;
        }
        let ctl = Ctl::new(&format!("hw:{card}"), false).ok()?;
        // The cable's controls are named after its capture end, which is this device.
        let mut id = ElemId::new(ElemIface::PCM);
        id.set_device(info.get_device());
        id.set_subdevice(info.get_subdevice());
        id.set_name(c"PCM Notify");
        let mut value = ElemValue::new(ElemType::Boolean).ok()?;
        value.set_id(&id);
        ctl.elem_read(&mut value).ok()?;
        KERNEL_HINT.call_once(|| {
            info!(
                "Following a loopback capture needs a kernel with the snd-aloop PCM Notify fix, Linux 7.4 or newer, or a stable kernel with the backport"
            );
        });
        if value.get_boolean(0) == Some(true) {
            debug!("PCM Notify is already set on the loopback cable, leaving it alone");
            return None;
        }
        value.set_boolean(0, true)?;
        if let Err(err) = ctl.elem_write(&value) {
            warn!("Unable to set PCM Notify on the loopback cable, error: {err}");
            return None;
        }
        debug!("Set PCM Notify on the loopback cable");
        Some(LoopbackNotify { ctl, id })
    }
}

impl Drop for LoopbackNotify {
    fn drop(&mut self) {
        if let Ok(mut value) = ElemValue::new(ElemType::Boolean) {
            value.set_id(&self.id);
            if value.set_boolean(0, false).is_some() && self.ctl.elem_write(&value).is_ok() {
                debug!("Cleared PCM Notify on the loopback cable");
            }
        }
    }
}

/// Ask an ALSA capture device what its source is doing, without capturing from it.
///
/// The PCM is opened and its hw params constraint read, but no params are set. That
/// doesn't hold a loopback cable to any format, since the kernel only does that when
/// a stream is prepared.
pub fn query_capture_source(device: &str) -> SourceState {
    let pcm = match alsa::PCM::new(device, Direction::Capture, true) {
        Ok(pcm) => pcm,
        Err(err) => {
            debug!("Unable to open capture device {device} to query its source, error: {err}");
            return SourceState::Unknown;
        }
    };
    let pinned = match HwParams::any(&pcm) {
        Ok(hwp) => PinnedFormat::of(&hwp),
        Err(err) => {
            debug!("Unable to read the hw params of capture device {device}, error: {err}");
            return SourceState::Unknown;
        }
    };
    let Ok(info) = pcm.info() else {
        return SourceState::Unknown;
    };
    let card = info.get_card();
    if card >= 0
        && let Ok(h) = HCtl::new(&format!("hw:{card}"), false)
        && h.load().is_ok()
    {
        let (device, subdevice) = (Some(info.get_device()), Some(info.get_subdevice()));
        // A loopback: the constraint is what the playback end runs, and is reliable
        // where the `PCM Slave` controls may be left over from an earlier stream.
        if let Some(active) = find_elem(&h, ElemIface::PCM, device, subdevice, "PCM Slave Active") {
            return match (active.read_as_bool(), pinned.source_format()) {
                (Some(false), _) => SourceState::Inactive,
                (Some(true), Some(format)) => SourceState::Format(format),
                _ => SourceState::Unknown,
            };
        }
        // A USB gadget: the rate the host plays at, 0 when it plays nothing.
        if let Some(rate) = find_elem(&h, ElemIface::PCM, device, subdevice, "Capture Rate") {
            return match rate.read_as_int() {
                Some(0) => SourceState::Inactive,
                Some(rate) => SourceState::Format(SourceFormat::rate(rate as usize)),
                None => SourceState::Unknown,
            };
        }
    }
    // Anything else can only tell when it is locked to a single rate.
    match pinned.source_format() {
        Some(format) => SourceState::Format(format),
        None => SourceState::Unknown,
    }
}

pub fn get_card_names(card: &Card, input: bool, names: &mut Vec<(String, String)>) -> Res<()> {
    let dir = if input {
        Direction::Capture
    } else {
        Direction::Playback
    };

    // Get a Ctl for the card
    let ctl_id = format!("hw:{}", card.get_index());
    let ctl = Ctl::new(&ctl_id, false)?;

    // Read card id and name
    let cardinfo = ctl.card_info()?;
    let card_id = cardinfo.get_id()?;
    let card_name = cardinfo.get_name()?;
    for device in DeviceIter::new(&ctl) {
        // Read info from Ctl
        let pcm_info = ctl.pcm_info(device as u32, 0, dir)?;

        // Read PCM name
        let pcm_name = pcm_info.get_name()?.to_string();

        // Loop through subdevices and get their names
        let subdevs = pcm_info.get_subdevices_count();
        for subdev in 0..subdevs {
            let pcm_info = ctl.pcm_info(device as u32, subdev, dir)?;
            // Build the full device id
            let subdevice_id = format!("hw:{card_id},{device},{subdev}").to_string();

            // Get subdevice name and build a descriptive device name
            let subdev_name = pcm_info.get_subdevice_name()?;
            let name = format!("{card_name}, {pcm_name}, {subdev_name}").to_string();

            //println!("{} - {}", subdevice_id, name);
            names.push((subdevice_id, name))
        }
    }

    Ok(())
}

pub fn list_hw_devices(input: bool) -> Vec<(String, String)> {
    let mut names = Vec::new();
    let cards = Iter::new();
    for card in cards.flatten() {
        get_card_names(&card, input, &mut names).unwrap_or_default();
    }
    names
}

pub fn list_pcm_devices(input: bool) -> Vec<(String, String)> {
    let mut names = Vec::new();
    let hints = HintIter::new_str(None, "pcm").unwrap();
    let direction = if input {
        Direction::Capture
    } else {
        Direction::Playback
    };
    for hint in hints {
        if let Some(name) = hint.name
            && (hint.direction.is_none()
                || hint
                    .direction
                    .map(|dir| dir == direction)
                    .unwrap_or_default())
        {
            let description = hint.desc.unwrap_or(name.clone());
            names.push((name, description))
        }
    }
    names
}

pub fn list_device_names(input: bool) -> Vec<(String, String)> {
    let mut hw_names = list_hw_devices(input);
    let mut pcm_names = list_pcm_devices(input);
    hw_names.append(&mut pcm_names);
    hw_names
}

pub fn get_device_capabilities(
    device_name: &str,
    input: bool,
) -> Result<crate::AudioDeviceDescriptor, crate::DeviceError> {
    let direction = if input {
        Direction::Capture
    } else {
        Direction::Playback
    };

    let pcm = match alsa::PCM::new(device_name, direction, false) {
        Ok(p) => p,
        Err(e) => {
            let errno = Errno::from_raw(e.errno());
            return Err(match errno {
                Errno::EBUSY => crate::DeviceError::DeviceBusy(device_name.to_string()),
                Errno::ENOENT | Errno::ENODEV => {
                    crate::DeviceError::DeviceNotFound(device_name.to_string())
                }
                _ => crate::DeviceError::Other(format!("{e}")),
            });
        }
    };

    let hwp = HwParams::any(&pcm).map_err(|err| {
        crate::DeviceError::Other(format!(
            "Failed to query ALSA hardware parameters for '{device_name}': {err}"
        ))
    })?;
    let channel_values =
        supported_channel_values(&hwp, CAPABILITY_PROBE_CHANNEL_LIMIT).map_err(|err| {
            crate::DeviceError::Other(format!(
                "Failed to query ALSA channel limits for '{device_name}': {err}"
            ))
        })?;

    let mut channel_capabilities = Vec::new();
    for channels in channel_values {
        let mut samplerates = Vec::new();
        if let Ok(hwp_ch) = HwParams::any(&pcm)
            && hwp_ch.set_channels(channels).is_ok()
            && let Ok(rates_values) = list_samplerates(&hwp_ch)
        {
            let rates = match rates_values {
                SupportedValues::Discrete(r) => r,
                SupportedValues::Range(min, max) => STANDARD_RATES
                    .iter()
                    .filter(|&&r| r >= min && r <= max)
                    .copied()
                    .collect(),
            };

            for rate in rates {
                let mut formats = Vec::new();
                if let Ok(hwp_rate) = HwParams::any(&pcm)
                    && hwp_rate.set_channels(channels).is_ok()
                    && hwp_rate.set_rate(rate, alsa::ValueOr::Nearest).is_ok()
                    && hwp_rate.get_rate().ok() == Some(rate)
                    && let Ok(supported_formats) = list_formats(&hwp_rate)
                {
                    for fmt in supported_formats {
                        formats.push(alsa_format_to_str(fmt).to_string());
                    }
                }
                if !formats.is_empty() {
                    samplerates.push(crate::SamplerateCapability {
                        samplerate: rate as usize,
                        formats,
                    });
                }
            }
        }
        if !samplerates.is_empty() {
            channel_capabilities.push(crate::ChannelCapability {
                channels: channels as usize,
                samplerates,
            });
        }
    }

    Ok(crate::AudioDeviceDescriptor {
        name: device_name.to_string(),
        description: device_name.to_string(), // In ALSA we only have device_name as name
        capability_sets: vec![crate::DeviceCapabilitySet {
            mode: crate::CapabilityMode::Unified,
            capabilities: channel_capabilities,
        }],
    })
}

pub fn state_desc(state: u32) -> String {
    match state {
        alsa_sys::SND_PCM_STATE_OPEN => "SND_PCM_STATE_OPEN, Open".to_string(),
        alsa_sys::SND_PCM_STATE_SETUP => "SND_PCM_STATE_SETUP, Setup installed".to_string(),
        alsa_sys::SND_PCM_STATE_PREPARED => "SND_PCM_STATE_PREPARED, Ready to start".to_string(),
        alsa_sys::SND_PCM_STATE_RUNNING => "SND_PCM_STATE_RUNNING, Running".to_string(),
        alsa_sys::SND_PCM_STATE_XRUN => {
            "SND_PCM_STATE_XRUN, Stopped: underrun (playback) or overrun (capture) detected"
                .to_string()
        }
        alsa_sys::SND_PCM_STATE_DRAINING => {
            "SND_PCM_STATE_DRAINING, Draining: running (playback) or stopped (capture)".to_string()
        }
        alsa_sys::SND_PCM_STATE_PAUSED => "SND_PCM_STATE_PAUSED, Paused".to_string(),
        alsa_sys::SND_PCM_STATE_SUSPENDED => {
            "SND_PCM_STATE_SUSPENDED, Hardware is suspended".to_string()
        }
        alsa_sys::SND_PCM_STATE_DISCONNECTED => {
            "SND_PCM_STATE_DISCONNECTED, Hardware is disconnected".to_string()
        }
        _ => format!("Unknown state with number {state}"),
    }
}

pub fn recover_suspended_pcm(pcmdevice: &alsa::PCM, direction: &str) -> Res<()> {
    warn!("{direction}: device is suspended, trying to resume");

    let mut attempt = 0usize;
    loop {
        match pcmdevice.resume() {
            Ok(()) => {
                info!("{direction}: resumed suspended ALSA device");
                return Ok(());
            }
            Err(err) => match Errno::from_raw(err.errno()) {
                Errno::EAGAIN => {
                    attempt += 1;
                    if attempt >= 200 {
                        warn!(
                            "{direction}: resume is still pending after {attempt} attempts, falling back to prepare"
                        );
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                errno => {
                    debug!("{direction}: resume failed with {errno:?}, falling back to prepare");
                    break;
                }
            },
        }
    }

    pcmdevice.prepare()?;
    Ok(())
}

pub fn list_samplerates(hwp: &HwParams) -> Res<SupportedValues> {
    let min_rate = hwp.get_rate_min()?;
    let max_rate = hwp.get_rate_max()?;
    if min_rate == max_rate {
        // Only one rate is supported.
        return Ok(SupportedValues::Discrete(vec![min_rate]));
    } else if hwp.test_rate(min_rate + 1).is_ok() {
        // If min_rate + 1 is sipported, then this must be a range.
        return Ok(SupportedValues::Range(min_rate, max_rate));
    }
    let mut rates = Vec::with_capacity(STANDARD_RATES.len());
    // Loop through and test all the standard rates.
    for rate in STANDARD_RATES.iter() {
        if hwp.test_rate(*rate).is_ok() {
            rates.push(*rate);
        }
    }
    rates.shrink_to_fit();
    Ok(SupportedValues::Discrete(rates))
}

pub fn list_samplerates_as_text(hwp: &HwParams) -> String {
    let supported_rates_res = list_samplerates(hwp);
    if let Ok(rates) = supported_rates_res {
        format!("supported samplerates: {rates:?}")
    } else {
        "failed checking supported samplerates".to_string()
    }
}

fn supported_channel_values(hwp: &HwParams, limit: u32) -> Res<Vec<u32>> {
    let min_channels = hwp.get_channels_min()?;
    let max_channels = hwp.get_channels_max()?;
    if min_channels == max_channels {
        return Ok(vec![min_channels]);
    }

    let check_max = max_channels.min(limit);

    let mut channels = Vec::with_capacity((check_max - min_channels + 1) as usize);
    for chan in min_channels..=check_max {
        if hwp.test_channels(chan).is_ok() {
            channels.push(chan);
        }
    }
    channels.shrink_to_fit();
    Ok(channels)
}

pub fn list_nbr_channels(hwp: &HwParams) -> Res<(u32, u32, Vec<u32>)> {
    let min_channels = hwp.get_channels_min()?;
    let max_channels = hwp.get_channels_max()?;
    let channels = supported_channel_values(hwp, CHANNEL_LIST_LIMIT)?;
    Ok((min_channels, max_channels, channels))
}

pub fn list_channels_as_text(hwp: &HwParams) -> String {
    let min_ch = match hwp.get_channels_min() {
        Ok(value) => value,
        Err(_) => return "failed checking supported channels".to_string(),
    };
    let max_ch = match hwp.get_channels_max() {
        Ok(value) => value,
        Err(_) => return "failed checking supported channels".to_string(),
    };

    if min_ch == max_ch {
        return format!("supported channels: exactly {min_ch}");
    }

    match supported_channel_values(hwp, CHANNEL_LIST_LIMIT) {
        Ok(ch_list) => {
            let omitted_note = if CHANNEL_LIST_LIMIT < max_ch {
                format!(", channel counts above {CHANNEL_LIST_LIMIT} omitted")
            } else {
                String::new()
            };
            format!(
                "supported channels: discrete values {ch_list:?} (reported min: {min_ch}, max: {max_ch}{omitted_note})"
            )
        }
        Err(_) => "failed checking supported channels".to_string(),
    }
}

pub fn alsa_format_to_str(fmt: AlsaSampleFormat) -> &'static str {
    match fmt {
        AlsaSampleFormat::S16_LE => "S16_LE",
        AlsaSampleFormat::S24_3_LE => "S24_3_LE",
        AlsaSampleFormat::S24_4_LE => "S24_4_LE",
        AlsaSampleFormat::S32_LE => "S32_LE",
        AlsaSampleFormat::F32_LE => "F32_LE",
        AlsaSampleFormat::F64_LE => "F64_LE",
    }
}

pub fn list_formats(hwp: &HwParams) -> Res<Vec<AlsaSampleFormat>> {
    let mut formats = Vec::with_capacity(6);
    // Let's just check the formats supported by CamillaDSP
    if hwp.test_format(Format::s16()).is_ok() {
        formats.push(AlsaSampleFormat::S16_LE);
    }
    if hwp.test_format(Format::s24()).is_ok() {
        formats.push(AlsaSampleFormat::S24_4_LE);
    }
    if hwp.test_format(Format::S243LE).is_ok() {
        formats.push(AlsaSampleFormat::S24_3_LE);
    }
    if hwp.test_format(Format::s32()).is_ok() {
        formats.push(AlsaSampleFormat::S32_LE);
    }
    if hwp.test_format(Format::float()).is_ok() {
        formats.push(AlsaSampleFormat::F32_LE);
    }
    if hwp.test_format(Format::float64()).is_ok() {
        formats.push(AlsaSampleFormat::F64_LE);
    }
    formats.shrink_to_fit();
    Ok(formats)
}

pub fn pick_preferred_format(hwp: &HwParams) -> Option<AlsaSampleFormat> {
    // Start with integer formats, in descending quality
    if hwp.test_format(Format::s32()).is_ok() {
        return Some(AlsaSampleFormat::S32_LE);
    }
    // The two 24-bit formats are equivalent, the order does not matter
    if hwp.test_format(Format::S243LE).is_ok() {
        return Some(AlsaSampleFormat::S24_3_LE);
    }
    if hwp.test_format(Format::s24()).is_ok() {
        return Some(AlsaSampleFormat::S24_4_LE);
    }
    if hwp.test_format(Format::s16()).is_ok() {
        return Some(AlsaSampleFormat::S16_LE);
    }
    // float formats are unusual, try these last
    if hwp.test_format(Format::float()).is_ok() {
        return Some(AlsaSampleFormat::F32_LE);
    }
    if hwp.test_format(Format::float64()).is_ok() {
        return Some(AlsaSampleFormat::F64_LE);
    }
    None
}

pub fn list_formats_as_text(hwp: &HwParams) -> String {
    let supported_formats_res = list_formats(hwp);
    if let Ok(formats) = supported_formats_res {
        format!("supported sample formats: {formats:?}")
    } else {
        "failed checking supported sample formats".to_string()
    }
}

pub struct ElemData<'a> {
    element: Elem<'a>,
    numid: u32,
}

impl<'a> ElemData<'a> {
    pub fn into_element(self) -> Elem<'a> {
        self.element
    }

    pub fn read_as_int(&self) -> Option<i32> {
        self.element
            .read()
            .ok()
            .and_then(|elval| elval.get_integer(0))
    }

    pub fn read_as_bool(&self) -> Option<bool> {
        self.element
            .read()
            .ok()
            .and_then(|elval| elval.get_boolean(0))
    }

    pub fn read_volume_in_db(&self, ctl: &Ctl) -> Option<f32> {
        self.read_as_int().and_then(|intval| {
            ctl.convert_to_db(&self.element.get_id().unwrap(), intval as i64)
                .ok()
                .map(|v| v.to_db())
        })
    }

    pub fn write_volume_in_db(&self, ctl: &Ctl, value: f32) {
        let intval = ctl.convert_from_db(
            &self.element.get_id().unwrap(),
            alsa::mixer::MilliBel::from_db(value),
            alsa::Round::Floor,
        );
        if let Ok(val) = intval {
            self.write_as_int(val as i32);
        }
    }

    pub fn write_as_int(&self, value: i32) {
        let mut elval = ElemValue::new(ElemType::Integer).unwrap();
        if elval.set_integer(0, value).is_some() {
            self.element.write(&elval).unwrap_or_default();
        }
    }

    pub fn write_as_bool(&self, value: bool) {
        let mut elval = ElemValue::new(ElemType::Boolean).unwrap();
        if elval.set_boolean(0, value).is_some() {
            self.element.write(&elval).unwrap_or_default();
        }
    }
}

#[derive(Default)]
pub struct CaptureElements<'a> {
    pub loopback_active: Option<ElemData<'a>>,
    pub loopback_rate: Option<ElemData<'a>>,
    pub loopback_format: Option<ElemData<'a>>,
    pub loopback_channels: Option<ElemData<'a>>,
    pub gadget_rate: Option<ElemData<'a>>,
    pub volume: Option<ElemData<'a>>,
    pub mute: Option<ElemData<'a>>,
}

impl CaptureElements<'_> {
    /// Whether a capture the kernel stopped means that the source changed format.
    ///
    /// While following, a loopback cable has `PCM Notify` set, and then the kernel stops
    /// the capture when its playback end starts with another format. The stop leaves the
    /// capture in DRAINING, then SETUP once drained, and reads fail with EBADFD.
    /// CamillaDSP never puts a capture in those states itself.
    pub fn kernel_stop_is_format_change(&self, params: &CaptureParams) -> bool {
        params.follow && self.loopback_rate.is_some()
    }

    /// The format the playback end of the loopback cable switched to.
    ///
    /// The `PCM Slave` controls only change when the format does, so they are only up
    /// to date right after the kernel stopped the capture for a new format.
    pub fn loopback_format_change(&self) -> SourceFormat {
        let read = |elem: &Option<ElemData>| elem.as_ref().and_then(|e| e.read_as_int());
        let format = SourceFormat {
            samplerate: read(&self.loopback_rate).unwrap_or(0) as usize,
            channels: read(&self.loopback_channels).map(|c| c as usize),
            format: read(&self.loopback_format)
                .and_then(format_from_number)
                .map(alsa_format_name),
        };
        info!("The kernel stopped the loopback capture, its playback end changed to {format}");
        format
    }
}

pub struct FileDescriptors {
    pub fds: Vec<alsa::poll::pollfd>,
    pub nbr_pcm_fds: usize,
}

#[derive(Debug)]
pub struct PollResult {
    pub poll_res: usize,
    pub pcm: bool,
    pub ctl: bool,
}

impl FileDescriptors {
    pub fn wait(&mut self, timeout: i32) -> alsa::Result<PollResult> {
        let nbr_ready = alsa::poll::poll(&mut self.fds, timeout)?;
        trace!("Got {nbr_ready} ready fds");
        let mut nbr_found = 0;
        let mut pcm_res = false;
        for fd in self.fds.iter().take(self.nbr_pcm_fds) {
            if fd.revents > 0 {
                pcm_res = true;
                nbr_found += 1;
                if nbr_found == nbr_ready {
                    // We are done, let's return early

                    return Ok(PollResult {
                        poll_res: nbr_ready,
                        pcm: pcm_res,
                        ctl: false,
                    });
                }
            }
        }
        // There were other ready file descriptors than PCM, must be controls
        Ok(PollResult {
            poll_res: nbr_ready,
            pcm: pcm_res,
            ctl: true,
        })
    }
}

pub fn process_events(
    ctl: &Ctl,
    elems: &CaptureElements,
    status_channel: &crossbeam_channel::Sender<StatusMessage>,
    params: &mut CaptureParams,
    processing_params: &Arc<ProcessingParameters>,
) -> CaptureResult {
    while let Ok(Some(ev)) = ctl.read() {
        let nid = ev.get_id().get_numid();
        debug!("Event from numid {nid}");
        let action = get_event_action(nid, elems, ctl, params);
        match action {
            EventAction::SourceInactive => {
                if params.stop_on_inactive {
                    debug!(
                        "Stopping, capture device is inactive and stop_on_inactive is set to true"
                    );
                    status_channel
                        .send(StatusMessage::CaptureDone)
                        .unwrap_or_default();
                    return CaptureResult::Done;
                }
            }
            EventAction::FormatChange(value) => {
                debug!("Stopping, capture device sample rate changed");
                return CaptureResult::FormatChange(SourceFormat::rate(value));
            }
            EventAction::SetVolume(vol) => {
                debug!("Alsa volume change event, set main fader to {vol} dB");
                processing_params.set_target_volume(0, vol);
                params.linked_volume_value = Some(vol);
                //status_channel
                //    .send(StatusMessage::SetVolume(vol))
                //    .unwrap_or_default();
            }
            EventAction::SetMute(mute) => {
                debug!("Alsa mute change event, set mute state to {mute}");
                processing_params.set_mute(0, mute);
                params.linked_mute_value = Some(mute);
                //status_channel
                //    .send(StatusMessage::SetMute(mute))
                //    .unwrap_or_default();
            }
            EventAction::None => {}
        }
    }
    CaptureResult::Normal
}

pub enum EventAction {
    None,
    SetVolume(f32),
    SetMute(bool),
    FormatChange(usize),
    SourceInactive,
}

pub fn get_event_action(
    numid: u32,
    elems: &CaptureElements,
    ctl: &Ctl,
    params: &mut CaptureParams,
) -> EventAction {
    if let Some(eldata) = &elems.loopback_active
        && eldata.numid == numid
    {
        let value = eldata.read_as_bool();
        debug!("Loopback active: {value:?}");
        if let Some(active) = value {
            if active {
                return EventAction::None;
            }
            return EventAction::SourceInactive;
        }
    }
    // The `PCM Slave` rate, format and channels events are ignored. A format change
    // on a loopback is followed when the kernel stops the capture for it, see
    // `CaptureElements::kernel_stop_is_format_change`.
    if let Some(eldata) = &elems.volume
        && eldata.numid == numid
    {
        let vol_db = eldata.read_volume_in_db(ctl);
        debug!("Mixer volume control: {vol_db:?} dB");
        if let Some(vol) = vol_db {
            params.linked_volume_value = Some(vol);
            return EventAction::SetVolume(vol);
        }
    }
    if let Some(eldata) = &elems.mute
        && eldata.numid == numid
    {
        let active = eldata.read_as_bool();
        debug!("Mixer switch active: {active:?}");
        if let Some(active_val) = active {
            params.linked_mute_value = Some(!active_val);
            return EventAction::SetMute(!active_val);
        }
    }
    if let Some(eldata) = &elems.gadget_rate
        && eldata.numid == numid
    {
        let value = eldata.read_as_int();
        debug!("Gadget rate: {value:?}");
        if let Some(rate) = value {
            if rate == 0 {
                return EventAction::SourceInactive;
            }
            if rate as usize != params.capture_samplerate {
                return EventAction::FormatChange(rate as usize);
            }
            debug!("Capture device resumed with unchanged sample rate");
            return EventAction::None;
        }
    }
    trace!("Ignoring event from control with numid {numid}");
    EventAction::None
}

impl<'a> CaptureElements<'a> {
    pub fn find_elements(
        &mut self,
        h: &'a HCtl,
        device: u32,
        subdevice: u32,
        volume_name: &Option<String>,
        mute_name: &Option<String>,
    ) {
        self.loopback_active = find_elem(
            h,
            ElemIface::PCM,
            Some(device),
            Some(subdevice),
            "PCM Slave Active",
        );
        let pcm_elem = |name| find_elem(h, ElemIface::PCM, Some(device), Some(subdevice), name);
        self.loopback_rate = pcm_elem("PCM Slave Rate");
        self.loopback_format = pcm_elem("PCM Slave Format");
        self.loopback_channels = pcm_elem("PCM Slave Channels");
        self.gadget_rate = find_elem(
            h,
            ElemIface::PCM,
            Some(device),
            Some(subdevice),
            "Capture Rate",
        );
        self.volume = volume_name
            .as_ref()
            .and_then(|name| find_elem(h, ElemIface::Mixer, None, None, name));
        self.mute = mute_name
            .as_ref()
            .and_then(|name| find_elem(h, ElemIface::Mixer, None, None, name));
    }
}

pub fn find_elem<'a>(
    hctl: &'a HCtl,
    iface: ElemIface,
    device: Option<u32>,
    subdevice: Option<u32>,
    name: &str,
) -> Option<ElemData<'a>> {
    let mut elem_id = ElemId::new(iface);
    if let Some(dev) = device {
        elem_id.set_device(dev);
    }
    if let Some(subdev) = subdevice {
        elem_id.set_subdevice(subdev);
    }
    elem_id.set_name(&CString::new(name).unwrap());
    let element = hctl.find_elem(&elem_id);
    debug!("Look up element with name {name}");
    element.map(|e| {
        let numid = e.get_id().map(|id| id.get_numid()).unwrap_or_default();
        debug!("Found element with name {name} and numid {numid}");
        ElemData { element: e, numid }
    })
}

pub fn sync_linked_controls(
    processing_params: &Arc<ProcessingParameters>,
    capture_params: &mut CaptureParams,
    elements: &mut CaptureElements,
    ctl: &Option<Ctl>,
) {
    if let Some(c) = ctl {
        if let Some(vol) = capture_params.linked_volume_value {
            let target_vol = processing_params.target_volume(0);
            if (vol - target_vol).abs() > 0.1 {
                debug!("Updating linked volume control to {target_vol} dB");
            }
            if let Some(vol_elem) = &elements.volume {
                vol_elem.write_volume_in_db(c, target_vol);
            }
        }
        if let Some(mute) = capture_params.linked_mute_value {
            let target_mute = processing_params.is_mute(0);
            if mute != target_mute {
                debug!("Updating linked switch control to {}", !target_mute);
                if let Some(mute_elem) = &elements.mute {
                    mute_elem.write_as_bool(!target_mute);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(rate: Option<u32>, channels: Option<u32>, format: Option<Format>) -> PinnedFormat {
        PinnedFormat {
            rate,
            channels,
            format,
        }
    }

    #[test]
    fn a_pinned_value_that_differs_is_followed() {
        let device = pinned(Some(44100), Some(2), Some(Format::S243LE));
        let change = device.differs_from(48000, 2, &None).unwrap();
        assert_eq!(change.samplerate, 44100);
        assert_eq!(change.channels, Some(2));
        assert_eq!(change.format.as_deref(), Some("S24_3_LE"));
        let change = pinned(None, Some(4), None)
            .differs_from(48000, 2, &None)
            .unwrap();
        assert_eq!(change.samplerate, 48000);
        assert_eq!(change.channels, Some(4));
    }

    #[test]
    fn a_pinned_format_only_counts_when_one_is_requested() {
        let device = pinned(Some(48000), Some(2), Some(Format::S32LE));
        assert!(device.differs_from(48000, 2, &None).is_none());
        assert!(
            device
                .differs_from(48000, 2, &Some(AlsaSampleFormat::S32_LE))
                .is_none()
        );
        let change = device
            .differs_from(48000, 2, &Some(AlsaSampleFormat::S16_LE))
            .unwrap();
        assert_eq!(change.format.as_deref(), Some("S32_LE"));
    }

    #[test]
    fn nothing_pinned_is_no_change() {
        assert!(
            pinned(None, None, None)
                .differs_from(48000, 2, &None)
                .is_none()
        );
        assert!(pinned(None, None, None).source_format().is_none());
    }

    #[test]
    fn format_names_follow_the_config_names() {
        assert_eq!(alsa_format_name(Format::S24LE), "S24_4_LE");
        assert_eq!(alsa_format_name(Format::FloatLE), "F32_LE");
        // A format CamillaDSP can't capture keeps its ALSA name.
        assert_eq!(alsa_format_name(Format::U8), "U8");
        assert_eq!(
            format_from_number(Format::S243LE as i32),
            Some(Format::S243LE)
        );
    }
}
