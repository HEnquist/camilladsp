use std::sync::LazyLock;
use parking_lot::Mutex;

use alsa::{
    Direction, ValueOr,
    pcm::{Access, Format, HwParams},
};

use crate::{
    Res,
    alsa_backend::{
        device_buffer_manager::DeviceBufferManager,
        utils::{
            list_channels_as_text, list_device_names, list_formats_as_text,
            list_samplerates_as_text, pick_preferred_format,
        },
    },
    audiodevice::DeviceError,
    config::AlsaSampleFormat,
};

static ALSA_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Open an Alsa PCM device
///
/// Synchronized internally so you can launch playback and capture concurrently
pub fn open_pcm(
    devname: String,
    samplerate: u32,
    channels: u32,
    sample_format: &Option<AlsaSampleFormat>,
    buf_manager: &mut dyn DeviceBufferManager,
    capture: bool,
) -> Res<(alsa::PCM, AlsaSampleFormat)> {
    let direction = if capture { "Capture" } else { "Playback" };
    debug!(
        "Available {} devices: {:?}",
        direction,
        list_device_names(capture)
    );
    // Acquire the lock
    let _lock = ALSA_MUTEX.lock();
    // Open the device
    let pcmdev = if capture {
        alsa::PCM::new(&devname, Direction::Capture, true)?
    } else {
        alsa::PCM::new(&devname, Direction::Playback, true)?
    };
    // Set hardware parameters
    let chosen_format;
    {
        let hwp = HwParams::any(&pcmdev)?;

        // Set number of channels
        debug!("{}: {}", direction, list_channels_as_text(&hwp));
        debug!("{direction}: setting channels to {channels}");
        hwp.set_channels(channels)?;

        // Set samplerate
        debug!("{}: {}", direction, list_samplerates_as_text(&hwp));
        debug!("{direction}: setting rate to {samplerate}");
        hwp.set_rate(samplerate, ValueOr::Nearest)?;

        // Set sample format
        debug!("{}: {}", direction, list_formats_as_text(&hwp));
        chosen_format = match sample_format {
            Some(sfmt) => *sfmt,
            None => {
                let preferred = pick_preferred_format(&hwp)
                    .ok_or(DeviceError::new("Unable to find a supported sample format"))?;
                debug!("{direction}: Picked sample format {preferred:?}");
                preferred
            }
        };
        debug!("{direction}: setting format to {chosen_format:?}");
        match chosen_format {
            AlsaSampleFormat::S16_LE => hwp.set_format(Format::s16())?,
            AlsaSampleFormat::S24_4_LE => hwp.set_format(Format::s24())?,
            AlsaSampleFormat::S24_3_LE => hwp.set_format(Format::s24_3())?,
            AlsaSampleFormat::S32_LE => hwp.set_format(Format::s32())?,
            AlsaSampleFormat::F32_LE => hwp.set_format(Format::float())?,
            AlsaSampleFormat::F64_LE => hwp.set_format(Format::float64())?,
        }

        // Set access mode, buffersize and periods
        hwp.set_access(Access::RWInterleaved)?;
        buf_manager.apply_buffer_size(&hwp)?;
        buf_manager.apply_period_size(&hwp)?;

        // Apply
        pcmdev.hw_params(&hwp)?;
    }
    {
        // Set software parameters
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        buf_manager.apply_start_threshold(&swp)?;
        buf_manager.apply_avail_min(&swp)?;
        debug!("Opening {direction} device \"{devname}\" with parameters: {hwp:?}, {swp:?}");
        pcmdev.sw_params(&swp)?;
        debug!("{direction} device \"{devname}\" successfully opened");
    }
    Ok((pcmdev, chosen_format))
}
