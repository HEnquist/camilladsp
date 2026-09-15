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

// Thin adapter around the `waveadapter` crate, which handles the actual wav
// header parsing and writing. This module only bridges between the crate's
// `SampleFormat` and the CamillaDSP `BinarySampleFormat` config type.

use std::fs::File;
use std::io::{Read, Seek, Write};

use waveadapter::SampleFormat;
use waveadapter::header::read_wav_header;

use crate::Res;
use crate::config::{BinarySampleFormat, ConfigError};

/// The subset of wav header parameters used by CamillaDSP.
#[derive(Debug)]
pub struct WavParams {
    pub sample_format: BinarySampleFormat,
    pub sample_rate: usize,
    pub data_offset: usize,
    pub data_length: usize,
    pub channels: usize,
}

/// Map a `waveadapter` sample format to the CamillaDSP config format.
/// Returns `None` for formats CamillaDSP does not support.
fn to_binary_format(format: SampleFormat) -> Option<BinarySampleFormat> {
    match format {
        SampleFormat::I16 => Some(BinarySampleFormat::S16_LE),
        SampleFormat::I24_3 => Some(BinarySampleFormat::S24_3_LE),
        SampleFormat::I24_4 => Some(BinarySampleFormat::S24_4_LJ_LE),
        SampleFormat::I32 => Some(BinarySampleFormat::S32_LE),
        SampleFormat::F32 => Some(BinarySampleFormat::F32_LE),
        SampleFormat::F64 => Some(BinarySampleFormat::F64_LE),
        // CamillaDSP has no unsigned 8-bit sample format.
        SampleFormat::U8 => None,
    }
}

/// Map a CamillaDSP config format to a `waveadapter` sample format.
pub fn to_wave_format(format: BinarySampleFormat) -> Res<SampleFormat> {
    match format {
        BinarySampleFormat::S16_LE => Ok(SampleFormat::I16),
        BinarySampleFormat::S24_3_LE => Ok(SampleFormat::I24_3),
        BinarySampleFormat::S24_4_LJ_LE => Ok(SampleFormat::I24_4),
        BinarySampleFormat::S32_LE => Ok(SampleFormat::I32),
        BinarySampleFormat::F32_LE => Ok(SampleFormat::F32),
        BinarySampleFormat::F64_LE => Ok(SampleFormat::F64),
        BinarySampleFormat::S24_4_RJ_LE => {
            Err(ConfigError::new("Wav files do not support the S24_4_RJ_LE sample format").into())
        }
    }
}

pub fn find_data_in_wav(filename: &str) -> Res<WavParams> {
    let f = File::open(filename)?;
    find_data_in_wav_stream(f).map_err(|err| {
        ConfigError::new(&format!(
            "Unable to parse wav file '{filename}', error: {err}"
        ))
        .into()
    })
}

pub fn find_data_in_wav_stream(f: impl Read + Seek) -> Res<WavParams> {
    let params = read_wav_header(f)
        .map_err(|err| ConfigError::new(&format!("Unable to parse as wav: {err}")))?;
    let sample_format = match params.sample_format.and_then(to_binary_format) {
        Some(format) => format,
        None => return Err(ConfigError::new("Unsupported wav format").into()),
    };
    Ok(WavParams {
        sample_format,
        sample_rate: params.sample_rate,
        data_offset: params.data_offset,
        data_length: params.data_length,
        channels: params.channels,
    })
}

// Write a wav header.
// The final length is unknown when streaming, so the RIFF and data chunk sizes
// are set to the u32::MAX placeholder.
pub fn write_wav_header(
    dest: &mut impl Write,
    channels: usize,
    sample_format: BinarySampleFormat,
    samplerate: usize,
) -> Res<()> {
    let format = to_wave_format(sample_format)?;
    waveadapter::header::write_wav_header(dest, channels, format, samplerate, u32::MAX, u32::MAX)
        .map_err(|err| ConfigError::new(&format!("Failed to write wav header: {err}")).into())
}

#[cfg(test)]
mod tests {
    use super::find_data_in_wav;
    use super::find_data_in_wav_stream;
    use super::write_wav_header;
    use crate::config::BinarySampleFormat;
    use std::io::Cursor;

    #[test]
    pub fn test_analyze_wav() {
        let info = find_data_in_wav("testdata/int32.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, BinarySampleFormat::S32_LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    pub fn test_analyze_wavex() {
        let info = find_data_in_wav("testdata/f32_ex.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, BinarySampleFormat::F32_LE);
        assert_eq!(info.data_offset, 104);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    pub fn test_analyze_rf64() {
        // The data chunk size field is the 0xFFFFFFFF placeholder; the real
        // length is resolved from the ds64 chunk.
        let info = find_data_in_wav("testdata/int32_rf64.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, BinarySampleFormat::S32_LE);
        assert_eq!(info.data_offset, 138);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    fn write_and_read_wav() {
        let bytes = vec![0_u8; 1000];
        let mut buffer = Cursor::new(bytes);
        write_wav_header(&mut buffer, 2, BinarySampleFormat::S32_LE, 44100).unwrap();
        buffer.set_position(0);
        let info = find_data_in_wav_stream(&mut buffer).unwrap();

        assert_eq!(info.sample_format, BinarySampleFormat::S32_LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_length, 4294967295);
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    fn test_invalid_wav() {
        let bytes = vec![0_u8; 1000];
        let mut buffer = Cursor::new(bytes);
        let info = find_data_in_wav_stream(&mut buffer);

        assert!(info.is_err());
    }
}
