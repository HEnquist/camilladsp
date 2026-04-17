// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// This file is licensed under the GNU General Public License version 3 only.
// It links against the ASIO SDK, which is licensed under GPLv3.
//
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

// ASIO backend for playback and capture.
// This implementation uses the asio-sys crate to interface with the ASIO driver system.

use std::collections::VecDeque;
use std::ptr;

use asio_sys::bindings::asio_import::{
    ASIOBufferInfo, ASIOCallbacks, ASIOChannelInfo, ASIOCreateBuffers, ASIOGetBufferSize,
    ASIOGetChannelInfo, get_sample_rate,
};

use crate::config::{AsioSampleFormat, BinarySampleFormat, ConfigError};

/// Read the currently active ASIO sample rate in Hz.
///
/// Returns `None` if the driver call fails or returns a non-finite/non-positive value.
pub(crate) fn read_current_asio_sample_rate_hz() -> Option<usize> {
    let mut rate = 0.0f64;
    let res = unsafe { get_sample_rate(&mut rate) };
    if res == 0 && rate.is_finite() && rate > 0.0 {
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

/// Create an `ASIOBufferInfo` array for the given number of channels.
pub(crate) fn make_buffer_infos(num_channels: usize, is_input: bool) -> Vec<ASIOBufferInfo> {
    (0..num_channels)
        .map(|ch| ASIOBufferInfo {
            isInput: if is_input { 1 } else { 0 },
            channelNum: ch as i32,
            buffers: [ptr::null_mut(), ptr::null_mut()],
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
pub(crate) fn asio_format_to_str(fmt: AsioSampleFormat) -> &'static str {
    match fmt {
        AsioSampleFormat::S16_LE => "S16_LE",
        AsioSampleFormat::S24_4_LE => "S24_4_LE",
        AsioSampleFormat::S24_3_LE => "S24_3_LE",
        AsioSampleFormat::S32_LE => "S32_LE",
        AsioSampleFormat::F32_LE => "F32_LE",
        AsioSampleFormat::F64_LE => "F64_LE",
    }
}

const ASIO_ST_INT16_MSB: i32 = 0;
const ASIO_ST_INT24_MSB: i32 = 1;
const ASIO_ST_INT32_MSB: i32 = 2;
const ASIO_ST_FLOAT32_MSB: i32 = 3;
const ASIO_ST_FLOAT64_MSB: i32 = 4;
const ASIO_ST_INT32_MSB_16: i32 = 8;
const ASIO_ST_INT32_MSB_18: i32 = 9;
const ASIO_ST_INT32_MSB_20: i32 = 10;
const ASIO_ST_INT32_MSB_24: i32 = 11;
const ASIO_ST_INT16_LSB: i32 = 16;
const ASIO_ST_INT24_LSB: i32 = 17;
const ASIO_ST_INT32_LSB: i32 = 18;
const ASIO_ST_FLOAT32_LSB: i32 = 19;
const ASIO_ST_FLOAT64_LSB: i32 = 20;
const ASIO_ST_INT32_LSB_16: i32 = 24;
const ASIO_ST_INT32_LSB_18: i32 = 25;
const ASIO_ST_INT32_LSB_20: i32 = 26;
const ASIO_ST_INT32_LSB_24: i32 = 27;
const ASIO_ST_DSD_INT8_LSB_1: i32 = 32;
const ASIO_ST_DSD_INT8_MSB_1: i32 = 33;
const ASIO_ST_DSD_INT8_NER8: i32 = 40;

/// Return a human-readable name for an ASIO sample type
/// (from `ASIOChannelInfo::type_`).
pub(crate) fn asio_sample_type_name(type_id: i32) -> &'static str {
    match type_id {
        ASIO_ST_INT16_MSB => "Int16 MSB (big-endian)",
        ASIO_ST_INT24_MSB => "Int24 MSB (3-byte packed, big-endian)",
        ASIO_ST_INT32_MSB => "Int32 MSB (big-endian)",
        ASIO_ST_FLOAT32_MSB => "Float32 MSB (big-endian)",
        ASIO_ST_FLOAT64_MSB => "Float64 MSB (big-endian)",
        ASIO_ST_INT32_MSB_16 => "Int32 MSB 16-bit (big-endian)",
        ASIO_ST_INT32_MSB_18 => "Int32 MSB 18-bit (big-endian)",
        ASIO_ST_INT32_MSB_20 => "Int32 MSB 20-bit (big-endian)",
        ASIO_ST_INT32_MSB_24 => "Int32 MSB 24-bit (big-endian)",
        ASIO_ST_INT16_LSB => "Int16 LSB",
        ASIO_ST_INT24_LSB => "Int24 LSB (3-byte packed)",
        ASIO_ST_INT32_LSB => "Int32 LSB",
        ASIO_ST_FLOAT32_LSB => "Float32 LSB",
        ASIO_ST_FLOAT64_LSB => "Float64 LSB",
        ASIO_ST_INT32_LSB_16 => "Int32 LSB 16-bit",
        ASIO_ST_INT32_LSB_18 => "Int32 LSB 18-bit",
        ASIO_ST_INT32_LSB_20 => "Int32 LSB 20-bit",
        ASIO_ST_INT32_LSB_24 => "Int32 LSB 24-bit",
        ASIO_ST_DSD_INT8_LSB_1 => "DSD Int8 LSB 1",
        ASIO_ST_DSD_INT8_MSB_1 => "DSD Int8 MSB 1",
        ASIO_ST_DSD_INT8_NER8 => "DSD Int8 NER8",
        _ => "Unknown",
    }
}

/// Map an ASIO sample type to an `AsioSampleFormat`.
///
/// Returns `None` for types that CamillaDSP cannot handle (e.g. big-endian, DSD).
pub(crate) fn asio_sample_type_to_format(type_id: i32) -> Option<AsioSampleFormat> {
    match type_id {
        ASIO_ST_INT16_LSB => Some(AsioSampleFormat::S16_LE),
        ASIO_ST_INT24_LSB => Some(AsioSampleFormat::S24_3_LE),
        ASIO_ST_INT32_LSB => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_16 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_18 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_20 => Some(AsioSampleFormat::S32_LE),
        ASIO_ST_INT32_LSB_24 => Some(AsioSampleFormat::S24_4_LE),
        ASIO_ST_FLOAT32_LSB => Some(AsioSampleFormat::F32_LE),
        ASIO_ST_FLOAT64_LSB => Some(AsioSampleFormat::F64_LE),
        _ => None,
    }
}

/// Convert a fixed-size C char buffer to `String` without reading past the buffer.
///
/// Some ASIO drivers may return char arrays without NUL termination; this helper
/// safely truncates at the first NUL if present, otherwise uses the full buffer.
pub(crate) fn fixed_cstr_buf_to_string(buf: &[i8]) -> String {
    let end = buf.iter().position(|&ch| ch == 0).unwrap_or(buf.len());
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, end) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Query the native sample format of channel 0 for the given direction.
/// Must be called after the driver is loaded and initialized.
pub(crate) fn query_device_format(is_input: bool) -> Result<i32, ConfigError> {
    let mut info = ASIOChannelInfo {
        channel: 0,
        isInput: if is_input { 1 } else { 0 },
        isActive: 0,
        channelGroup: 0,
        type_: 0,
        name: [0; 32],
    };
    let res = unsafe { ASIOGetChannelInfo(&mut info) };
    if res != 0 {
        let direction = if is_input { "input" } else { "output" };
        return Err(ConfigError::new(&format!(
            "ASIOGetChannelInfo failed for {direction} channel 0 (error code {res})"
        )));
    }
    debug!(
        "ASIO channel 0 ({}): type={} ({})",
        if is_input { "input" } else { "output" },
        info.type_,
        asio_sample_type_name(info.type_),
    );
    Ok(info.type_)
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
    configured: &Option<AsioSampleFormat>,
    is_input: bool,
) -> Result<AsioSampleFormat, ConfigError> {
    let device_type = query_device_format(is_input)?;
    let device_format = asio_sample_type_to_format(device_type);
    let direction = if is_input { "capture" } else { "playback" };

    let native_format = match device_format {
        Some(fmt) => fmt,
        None => {
            return Err(ConfigError::new(&format!(
                "ASIO {direction}: device uses unsupported sample type {} ({})",
                device_type,
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

/// Create ASIO buffers and register callbacks.
pub(crate) fn create_asio_buffers(
    buffer_infos: &mut [ASIOBufferInfo],
    num_channels: i32,
    buffer_size: i32,
    callbacks: &mut ASIOCallbacks,
) -> Result<(), ConfigError> {
    trace!(
        "Calling ASIOCreateBuffers: infos_ptr={:p}, channels={}, buffer_size={}, callbacks_ptr={:p}",
        buffer_infos.as_mut_ptr(),
        num_channels,
        buffer_size,
        callbacks as *mut ASIOCallbacks
    );
    let res = unsafe {
        ASIOCreateBuffers(
            buffer_infos.as_mut_ptr(),
            num_channels,
            buffer_size,
            callbacks,
        )
    };
    trace!("ASIOCreateBuffers returned {}.", res);
    if res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOCreateBuffers failed with error code {res}"
        )));
    }
    Ok(())
}

/// Query preferred ASIO buffer size.
pub(crate) fn get_preferred_buffer_size() -> Result<i32, ConfigError> {
    let mut min_buf: i32 = 0;
    let mut max_buf: i32 = 0;
    let mut preferred_buf: i32 = 0;
    let mut granularity: i32 = 0;
    let res = unsafe {
        ASIOGetBufferSize(
            &mut min_buf,
            &mut max_buf,
            &mut preferred_buf,
            &mut granularity,
        )
    };
    if res != 0 {
        return Err(ConfigError::new(&format!(
            "ASIOGetBufferSize failed with error code {res}"
        )));
    }
    trace!(
        "ASIOGetBufferSize: min={}, max={}, preferred={}, granularity={}",
        min_buf, max_buf, preferred_buf, granularity
    );
    Ok(preferred_buf)
}
