use crate::config::SampleFormat;
use crate::Res;
use alsa::card::Iter;
use alsa::ctl::{Ctl, DeviceIter};
use alsa::device_name::HintIter;
use alsa::pcm::{Format, HwParams};
use alsa::Card;
use alsa::Direction;
use alsa_sys;

const STANDARD_RATES: [u32; 17] = [
    5512, 8000, 11025, 16000, 22050, 32000, 44100, 48000, 64000, 88200, 96000, 176400, 192000,
    352800, 384000, 705600, 768000,
];

#[derive(Debug)]
pub enum SupportedValues {
    Range(u32, u32),
    Discrete(Vec<u32>),
}

fn get_card_names(card: &Card, input: bool, names: &mut Vec<(String, String)>) -> Res<()> {
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
            && hint.desc.is_some()
            && (hint.direction.is_none()
                || hint
                    .direction
                    .map(|dir| dir == direction)
                    .unwrap_or_default())
        {
            names.push((hint.name.unwrap(), hint.desc.unwrap()))
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

pub fn list_formats_as_text(hwp: &HwParams) -> String {
    let supported_formats_res = list_formats(hwp);
    if let Ok(formats) = supported_formats_res {
        format!("supported sample formats: {formats:?}")
    } else {
        "failed checking supported sample formats".to_string()
    }
}

pub fn adjust_speed(
    avg_delay: f64,
    target_delay: usize,
    prev_diff: Option<f64>,
    mut capture_speed: f64,
) -> (f64, f64) {
    let latency = avg_delay * capture_speed;
    let diff = latency - target_delay as f64;
    match prev_diff {
        None => (1.0, diff),
        Some(prev_diff) => {
            let equality_range = target_delay as f64 / 100.0; // in frames
            let speed_delta = 1e-5;
            if diff > 0.0 {
                if diff > (prev_diff + equality_range) {
                    // playback latency grows, need to slow down capture more
                    capture_speed -= 3.0 * speed_delta;
                } else if is_within(diff, prev_diff, equality_range) {
                    // positive, not changed from last cycle, need to slow down capture a bit
                    capture_speed -= speed_delta;
                }
            } else if diff < 0.0 {
                if diff < (prev_diff - equality_range) {
                    // playback latency sinks, need to speed up capture more
                    capture_speed += 3.0 * speed_delta;
                } else if is_within(diff, prev_diff, equality_range) {
                    // negative, not changed from last cycle, need to speed up capture a bit
                    capture_speed += speed_delta
                }
            }
            debug!(
                "Avg. buffer delay: {:.1}, target delay: {:.1}, diff: {}, prev_div: {}, corrected capture rate: {:.4}%",
                avg_delay,
                target_delay,
                diff,
                prev_diff,
                100.0 * capture_speed
            );
            (capture_speed, diff)
        }
    }
}

pub fn is_within(value: f64, target: f64, equality_range: f64) -> bool {
    value <= (target + equality_range) && value >= (target - equality_range)
}
