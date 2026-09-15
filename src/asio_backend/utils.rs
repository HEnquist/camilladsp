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

// Helpers for the ASIO backend.
// The driver is accessed through the azo crate, which talks to the driver COM objects
// directly instead of going through the Steinberg ASIO SDK.

use std::collections::VecDeque;
use std::ffi::c_void;

use azo::dto::ChannelId;
use azo::sys::{Callbacks, SampleType};

use crate::asio_backend::driver::with_driver;
use crate::config::{AsioSampleFormat, BinarySampleFormat, ConfigError};

/// The pair of double-buffer pointers a driver hands out for one channel.
///
/// Index with the buffer index passed to the buffer-switch callback.
pub(crate) type ChannelBuffers = [*mut c_void; 2];

/// Read the currently active ASIO sample rate in Hz.
///
/// Returns `None` if the driver call fails or returns a non-finite/non-positive value.
pub(crate) fn read_current_asio_sample_rate_hz(devname: &str) -> Option<usize> {
    let rate = with_driver(devname, |driver| {
        driver
            .get_sample_rate()
            .map_err(|err| ConfigError::new(&format!("Failed to read ASIO sample rate: {err:?}")))
    })
    .ok()?;
    if rate.is_finite() && rate > 0.0 {
        Some(rate.round() as usize)
    } else {
        None
    }
}

/// Copy bytes from a `VecDeque` at `offset` into `dst`.
///
/// Handles split head/tail storage without allocating.
pub(crate) fn copy_from_queue_at_offset(queue: &VecDeque<u8>, offset: usize, dst: &mut [u8]) {
    let (head, tail) = queue.as_slices();
    if offset < head.len() {
        let first = (head.len() - offset).min(dst.len());
        dst[..first].copy_from_slice(&head[offset..offset + first]);
        if first < dst.len() {
            let remaining = dst.len() - first;
            dst[first..].copy_from_slice(&tail[..remaining]);
        }
    } else {
        let tail_offset = offset - head.len();
        dst.copy_from_slice(&tail[tail_offset..tail_offset + dst.len()]);
    }
}

/// Build the channel identifiers for the first `num_channels` channels of one direction.
pub(crate) fn make_channel_ids(num_channels: usize, is_input: bool) -> Vec<ChannelId> {
    (0..num_channels)
        .map(|ch| ChannelId {
            input: is_input,
            index: ch as i32,
        })
        .collect()
}

/// Resolve ASIO sample format to a `BinarySampleFormat`.
pub(crate) fn resolve_binary_format(format: &AsioSampleFormat) -> BinarySampleFormat {
    match format {
        AsioSampleFormat::S16_LE => BinarySampleFormat::S16_LE,
        AsioSampleFormat::S24_4_LE => BinarySampleFormat::S24_4_LJ_LE,
        AsioSampleFormat::S24_3_LE => BinarySampleFormat::S24_3_LE,
        AsioSampleFormat::S32_LE => BinarySampleFormat::S32_LE,
        AsioSampleFormat::F32_LE => BinarySampleFormat::F32_LE,
        AsioSampleFormat::F64_LE => BinarySampleFormat::F64_LE,
    }
}

/// Convert an `AsioSampleFormat` to the canonical string used in YAML configs.
///
/// These must match how the enum serialises, since the strings are handed out through the
/// capabilities API for clients to put straight into a config.
pub(crate) fn asio_format_to_str(fmt: AsioSampleFormat) -> &'static str {
    match fmt {
        AsioSampleFormat::S16_LE => "S16_LE",
        AsioSampleFormat::S24_3_LE => "S24_3_LE",
        AsioSampleFormat::S24_4_LE => "S24_4_LE",
        AsioSampleFormat::S32_LE => "S32_LE",
        AsioSampleFormat::F32_LE => "F32_LE",
        AsioSampleFormat::F64_LE => "F64_LE",
    }
}

/// Human-readable name of an ASIO sample type, for logging.
pub(crate) fn asio_sample_type_name(sample_type: SampleType) -> &'static str {
    match sample_type {
        SampleType::PCM_I16_MSB => "Int16 MSB (big-endian)",
        SampleType::PCM_I24_MSB => "Int24 MSB (3-byte packed, big-endian)",
        SampleType::PCM_I32_MSB => "Int32 MSB (big-endian)",
        SampleType::PCM_F32_MSB => "Float32 MSB (big-endian)",
        SampleType::PCM_F64_MSB => "Float64 MSB (big-endian)",
        SampleType::PCM_I32_MSB_16 => "Int32 MSB 16-bit (big-endian)",
        SampleType::PCM_I32_MSB_18 => "Int32 MSB 18-bit (big-endian)",
        SampleType::PCM_I32_MSB_20 => "Int32 MSB 20-bit (big-endian)",
        SampleType::PCM_I32_MSB_24 => "Int32 MSB 24-bit (big-endian)",
        SampleType::PCM_I16_LSB => "Int16 LSB",
        SampleType::PCM_I24_LSB => "Int24 LSB (3-byte packed)",
        SampleType::PCM_I32_LSB => "Int32 LSB",
        SampleType::PCM_F32_LSB => "Float32 LSB",
        SampleType::PCM_F64_LSB => "Float64 LSB",
        SampleType::PCM_I32_LSB_16 => "Int32 LSB 16-bit",
        SampleType::PCM_I32_LSB_18 => "Int32 LSB 18-bit",
        SampleType::PCM_I32_LSB_20 => "Int32 LSB 20-bit",
        SampleType::PCM_I32_LSB_24 => "Int32 LSB 24-bit",
        SampleType::DSD_I8_LSB_1 => "DSD Int8 LSB 1",
        SampleType::DSD_I8_MSB_1 => "DSD Int8 MSB 1",
        SampleType::DSD_I8_NER_8 => "DSD Int8 NER8",
        _ => "Unknown",
    }
}

/// Map an ASIO sample type to the matching `AsioSampleFormat`, if supported.
pub(crate) fn asio_sample_type_to_format(sample_type: SampleType) -> Option<AsioSampleFormat> {
    match sample_type {
        SampleType::PCM_I16_LSB => Some(AsioSampleFormat::S16_LE),
        SampleType::PCM_I24_LSB => Some(AsioSampleFormat::S24_3_LE),
        SampleType::PCM_I32_LSB => Some(AsioSampleFormat::S32_LE),
        SampleType::PCM_I32_LSB_16 => Some(AsioSampleFormat::S32_LE),
        SampleType::PCM_I32_LSB_18 => Some(AsioSampleFormat::S32_LE),
        SampleType::PCM_I32_LSB_20 => Some(AsioSampleFormat::S32_LE),
        SampleType::PCM_I32_LSB_24 => Some(AsioSampleFormat::S24_4_LE),
        SampleType::PCM_F32_LSB => Some(AsioSampleFormat::F32_LE),
        SampleType::PCM_F64_LSB => Some(AsioSampleFormat::F64_LE),
        _ => None,
    }
}

/// Query the native sample format of channel 0 for the given direction.
/// Must be called after the driver is loaded and initialized.
pub(crate) fn query_device_format(
    devname: &str,
    is_input: bool,
) -> Result<SampleType, ConfigError> {
    let direction = if is_input { "input" } else { "output" };
    let info = with_driver(devname, |driver| {
        driver
            .channel_info(ChannelId {
                input: is_input,
                index: 0,
            })
            .map_err(|err| {
                ConfigError::new(&format!(
                    "getChannelInfo failed for {direction} channel 0: {err:?}"
                ))
            })
    })?;
    debug!(
        "ASIO channel 0 ({}): type={} ({})",
        direction,
        info.sample_type.0,
        asio_sample_type_name(info.sample_type),
    );
    Ok(info.sample_type)
}

/// Resolve the sample format to use for a given direction.
///
/// ASIO drivers do not perform sample format conversion — the application must
/// use the device's native format. This function queries the device for its
/// native sample type and returns the corresponding `AsioSampleFormat`.
///
/// If the user specified a format in the config that differs from the native
/// format, an error is returned. If the format is `None`, auto-detect from the device.
/// Must be called after the driver is loaded and initialized.
pub(crate) fn resolve_format(
    devname: &str,
    configured: &Option<AsioSampleFormat>,
    is_input: bool,
) -> Result<AsioSampleFormat, ConfigError> {
    let device_type = query_device_format(devname, is_input)?;
    let device_format = asio_sample_type_to_format(device_type);
    let direction = if is_input { "capture" } else { "playback" };

    let native_format = match device_format {
        Some(fmt) => fmt,
        None => {
            return Err(ConfigError::new(&format!(
                "ASIO {direction}: device uses unsupported sample type {} ({})",
                device_type.0,
                asio_sample_type_name(device_type),
            )));
        }
    };

    if let Some(fmt) = configured {
        if *fmt != native_format {
            return Err(ConfigError::new(&format!(
                "ASIO {direction}: configured format {fmt:?} does not match device native format \
                 {native_format:?} ({}). ASIO drivers do not convert sample formats. \
                 Please remove the format setting to auto-detect, or set it to {native_format:?}",
                asio_sample_type_name(device_type),
            )));
        }
        debug!("ASIO {direction}: configured format {fmt:?} matches device native format.");
    } else {
        debug!("ASIO {direction}: auto-detected format {native_format:?} from device.");
    }

    Ok(native_format)
}

/// Create ASIO buffers for `channels` and register `callbacks`.
///
/// Returns one [`ChannelBuffers`] entry per requested channel, in the same order.
///
/// # Safety
/// `callbacks` must remain valid until the buffers are disposed.
pub(crate) unsafe fn create_asio_buffers(
    devname: &str,
    channels: &[ChannelId],
    buffer_size: i32,
    callbacks: *const Callbacks,
) -> Result<Vec<ChannelBuffers>, ConfigError> {
    trace!(
        "Calling createBuffers: channels={}, buffer_size={}, callbacks_ptr={:p}",
        channels.len(),
        buffer_size,
        callbacks
    );
    let buffers = with_driver(devname, |driver| {
        // SAFETY: the caller guarantees `callbacks` outlives the buffers.
        unsafe { driver.create_buffers(channels.iter().copied(), buffer_size, callbacks) }
            .map(|iter| iter.collect::<Vec<_>>())
            .map_err(|err| ConfigError::new(&format!("createBuffers failed: {err:?}")))
    })?;
    trace!("createBuffers returned {} channel buffers.", buffers.len());
    Ok(buffers)
}

/// Dispose all buffers previously created on the driver.
pub(crate) fn dispose_asio_buffers(devname: &str) -> Result<(), ConfigError> {
    with_driver(devname, |driver| {
        driver
            .dispose_all_buffers()
            .map_err(|err| ConfigError::new(&format!("disposeBuffers failed: {err:?}")))
    })
}

/// Start the ASIO stream.
pub(crate) fn start_asio_stream(devname: &str) -> Result<(), ConfigError> {
    with_driver(devname, |driver| {
        driver
            .start()
            .map_err(|err| ConfigError::new(&format!("start failed: {err:?}")))
    })
}

/// Stop the ASIO stream.
pub(crate) fn stop_asio_stream(devname: &str) -> Result<(), ConfigError> {
    with_driver(devname, |driver| {
        driver
            .stop()
            .map_err(|err| ConfigError::new(&format!("stop failed: {err:?}")))
    })
}

/// Log the latencies the driver reports, for diagnostics.
///
/// Must be called after the buffers have been created, since the reported values depend on
/// the buffer size. This is the driver's own latency and does not include the buffering
/// CamillaDSP adds through `chunksize` and `target_level`.
pub(crate) fn log_asio_latencies(devname: &str) {
    // Querying the driver costs two calls, so skip it entirely unless it will be logged.
    if !log_enabled!(log::Level::Debug) {
        return;
    }
    let latencies = with_driver(devname, |driver| {
        driver
            .latencies()
            .map_err(|err| ConfigError::new(&format!("getLatencies failed: {err:?}")))
    });
    let samplerate = read_current_asio_sample_rate_hz(devname).unwrap_or(0);
    match latencies {
        Ok(latencies) if samplerate > 0 => {
            let as_ms = |frames: i32| 1000.0 * frames as f64 / samplerate as f64;
            debug!(
                "ASIO driver reported latencies: capture {} frames ({:.1} ms), \
                 playback {} frames ({:.1} ms).",
                latencies.in_,
                as_ms(latencies.in_),
                latencies.out,
                as_ms(latencies.out),
            );
        }
        Ok(latencies) => debug!(
            "ASIO driver reported latencies: capture {} frames, playback {} frames.",
            latencies.in_, latencies.out
        ),
        Err(err) => debug!("Could not read ASIO latencies: {err}"),
    }
}

/// Query preferred ASIO buffer size.
pub(crate) fn get_preferred_buffer_size(devname: &str) -> Result<i32, ConfigError> {
    let sizes = with_driver(devname, |driver| {
        driver
            .buffer_size()
            .map_err(|err| ConfigError::new(&format!("getBufferSize failed: {err:?}")))
    })?;
    trace!(
        "getBufferSize: min={}, max={}, preferred={}, granularity={:?}",
        sizes.min, sizes.max, sizes.preferred, sizes.granularity
    );
    Ok(sizes.preferred)
}
