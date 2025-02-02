use crate::config::SampleFormat;
use crate::{CaptureStatus, PlaybackStatus, PrcFmt, Res, StatusMessage};
use alsa::card::Iter;
use alsa::ctl::{Ctl, DeviceIter, ElemId, ElemIface, ElemType, ElemValue};
use alsa::device_name::HintIter;
use alsa::hctl::{Elem, HCtl};
use alsa::pcm::{Format, HwParams};
use alsa::{Card, Direction};
use alsa_sys;
use parking_lot::RwLock;
use std::ffi::CString;
use std::sync::Arc;

use crate::ProcessingParameters;

const STANDARD_RATES: [u32; 17] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    352800, 384000, 705600, 768000,
];

#[derive(Debug)]
pub enum SupportedValues {
    Range(u32, u32),
    Discrete(Vec<u32>),
}

pub struct CaptureParams {
    pub channels: usize,
    pub sample_format: SampleFormat,
    pub silence_timeout: PrcFmt,
    pub silence_threshold: PrcFmt,
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
    pub sample_format: SampleFormat,
    pub playback_status: Arc<RwLock<PlaybackStatus>>,
    pub bytes_per_frame: usize,
    pub samplerate: usize,
    pub chunksize: usize,
}

pub enum CaptureResult {
    Normal,
    Stalled,
    Done,
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
            let subdevice_id = format!("hw:{},{},{}", card_id, device, subdev).to_string();

            // Get subdevice name and build a descriptive device name
            let subdev_name = pcm_info.get_subdevice_name()?;
            let name = format!("{}, {}, {}", card_name, pcm_name, subdev_name).to_string();

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
        if hint.name.is_some()
            && (hint.direction.is_none()
                || hint
                    .direction
                    .map(|dir| dir == direction)
                    .unwrap_or_default())
        {
            let name = hint.name.unwrap();
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

pub fn list_nbr_channels(hwp: &HwParams) -> Res<(u32, u32, Vec<u32>)> {
    let min_channels = hwp.get_channels_min()?;
    let max_channels = hwp.get_channels_max()?;
    if min_channels == max_channels {
        return Ok((min_channels, max_channels, vec![min_channels]));
    }

    let mut check_max = max_channels;
    if check_max > 32 {
        check_max = 32;
    }

    let mut channels = Vec::with_capacity(check_max as usize);
    for chan in min_channels..=check_max {
        if hwp.test_channels(chan).is_ok() {
            channels.push(chan);
        }
    }
    channels.shrink_to_fit();
    Ok((min_channels, max_channels, channels))
}

pub fn list_channels_as_text(hwp: &HwParams) -> String {
    let supported_channels_res = list_nbr_channels(hwp);
    if let Ok((min_ch, max_ch, ch_list)) = supported_channels_res {
        format!("supported channels, min: {min_ch}, max: {max_ch}, list: {ch_list:?}")
    } else {
        "failed checking supported channels".to_string()
    }
}

pub fn list_formats(hwp: &HwParams) -> Res<Vec<SampleFormat>> {
    let mut formats = Vec::with_capacity(6);
    // Let's just check the formats supported by CamillaDSP
    if hwp.test_format(Format::s16()).is_ok() {
        formats.push(SampleFormat::S16LE);
    }
    if hwp.test_format(Format::s24()).is_ok() {
        formats.push(SampleFormat::S24LE);
    }
    if hwp.test_format(Format::S243LE).is_ok() {
        formats.push(SampleFormat::S24LE3);
    }
    if hwp.test_format(Format::s32()).is_ok() {
        formats.push(SampleFormat::S32LE);
    }
    if hwp.test_format(Format::float()).is_ok() {
        formats.push(SampleFormat::FLOAT32LE);
    }
    if hwp.test_format(Format::float64()).is_ok() {
        formats.push(SampleFormat::FLOAT64LE);
    }
    formats.shrink_to_fit();
    Ok(formats)
}

pub fn pick_preferred_format(hwp: &HwParams) -> Option<SampleFormat> {
    // Start with integer formats, in descending quality
    if hwp.test_format(Format::s32()).is_ok() {
        return Some(SampleFormat::S32LE);
    }
    // The two 24-bit formats are equivalent, the order does not matter
    if hwp.test_format(Format::S243LE).is_ok() {
        return Some(SampleFormat::S24LE3);
    }
    if hwp.test_format(Format::s24()).is_ok() {
        return Some(SampleFormat::S24LE);
    }
    if hwp.test_format(Format::s16()).is_ok() {
        return Some(SampleFormat::S16LE);
    }
    // float formats are unusual, try these last
    if hwp.test_format(Format::float()).is_ok() {
        return Some(SampleFormat::FLOAT32LE);
    }
    if hwp.test_format(Format::float64()).is_ok() {
        return Some(SampleFormat::FLOAT64LE);
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

impl ElemData<'_> {
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
    // pub loopback_rate: Option<ElemData<'a>>,
    // pub loopback_format: Option<ElemData<'a>>,
    // pub loopback_channels: Option<ElemData<'a>>,
    pub gadget_rate: Option<ElemData<'a>>,
    pub volume: Option<ElemData<'a>>,
    pub mute: Option<ElemData<'a>>,
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
        trace!("Got {} ready fds", nbr_ready);
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
        debug!("Event from numid {}", nid);
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
                debug!("Stopping, capture device sample format changed");
                status_channel
                    .send(StatusMessage::CaptureFormatChange(value))
                    .unwrap_or_default();
                return CaptureResult::Done;
            }
            EventAction::SetVolume(vol) => {
                debug!("Alsa volume change event, set main fader to {} dB", vol);
                processing_params.set_target_volume(0, vol);
                params.linked_volume_value = Some(vol);
                //status_channel
                //    .send(StatusMessage::SetVolume(vol))
                //    .unwrap_or_default();
            }
            EventAction::SetMute(mute) => {
                debug!("Alsa mute change event, set mute state to {}", mute);
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
    if let Some(eldata) = &elems.loopback_active {
        if eldata.numid == numid {
            let value = eldata.read_as_bool();
            debug!("Loopback active: {:?}", value);
            if let Some(active) = value {
                if active {
                    return EventAction::None;
                }
                return EventAction::SourceInactive;
            }
        }
    }
    // Include this if the notify functionality of the loopback gets fixed
    /*
    if let Some(eldata) = &elems.loopback_rate {
        if eldata.numid == numid {
            let value = eldata.read_as_int();
            debug!("Gadget rate: {:?}", value);
            if let Some(rate) = value {
                debug!("Loopback rate: {}", rate);
                return EventAction::FormatChange(rate);
            }
        }
    }
    if let Some(eldata) = &elems.loopback_format {
        if eldata.numid == numid {
            let value = eldata.read_as_int();
            debug!("Gadget rate: {:?}", value);
            if let Some(format) = value {
                debug!("Loopback format: {}", format);
                return EventAction::FormatChange(TODO add sample format!);
            }
        }
    }
    if let Some(eldata) = &elems.loopback_channels {
        if eldata.numid == numid {
            debug!("Gadget rate: {:?}", value);
            if let Some(chans) = value {
                debug!("Loopback channels: {}", chans);
                return EventAction::FormatChange(TODO add channels!);
            }
        }
    } */
    if let Some(eldata) = &elems.volume {
        if eldata.numid == numid {
            let vol_db = eldata.read_volume_in_db(ctl);
            debug!("Mixer volume control: {:?} dB", vol_db);
            if let Some(vol) = vol_db {
                params.linked_volume_value = Some(vol);
                return EventAction::SetVolume(vol);
            }
        }
    }
    if let Some(eldata) = &elems.mute {
        if eldata.numid == numid {
            let active = eldata.read_as_bool();
            debug!("Mixer switch active: {:?}", active);
            if let Some(active_val) = active {
                params.linked_mute_value = Some(!active_val);
                return EventAction::SetMute(!active_val);
            }
        }
    }
    if let Some(eldata) = &elems.gadget_rate {
        if eldata.numid == numid {
            let value = eldata.read_as_int();
            debug!("Gadget rate: {:?}", value);
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
    }
    trace!("Ignoring event from control with numid {}", numid);
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
        // self.loopback_rate = find_elem(h, ElemIface::PCM, device, subdevice, "PCM Slave Rate");
        // self.loopback_format = find_elem(h, ElemIface::PCM, device, subdevice, "PCM Slave Format");
        // self.loopback_channels = find_elem(h, ElemIface::PCM, device, subdevice, "PCM Slave Channels");
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
    debug!("Look up element with name {}", name);
    element.map(|e| {
        let numid = e.get_id().map(|id| id.get_numid()).unwrap_or_default();
        debug!("Found element with name {} and numid {}", name, numid);
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
                debug!("Updating linked volume control to {} dB", target_vol);
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
