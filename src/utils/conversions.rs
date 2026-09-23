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

use crate::CamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config::BinarySampleFormat;
use crate::utils::stash::{container_from_stash, recycle_chunk, vec_from_stash};
use audioadapter::{Adapter, AdapterMut};
use audioadapter_buffers::number_to_float::InterleavedNumbers;
use audioadapter_sample::sample::{F32_LE, F64_LE, I16_LE, I24_4LJ_LE, I24_4RJ_LE, I24_LE, I32_LE};

fn chunk_to_buffer_with_adapter<A>(
    chunk: AudioChunk,
    adapter: &mut A,
    bytes_per_sample: usize,
) -> (usize, usize)
where
    A: AdapterMut<CamillaFloat>,
{
    let mut clipped = 0;
    let mut peak: CamillaFloat = 0.0;
    let num_valid_bytes = chunk.valid_frames * chunk.channels * bytes_per_sample;
    for chan in 0..chunk.channels {
        if chunk.waveforms[chan].is_empty() {
            adapter.fill_channel_with(chan, &0.0);
        } else {
            let (nbr, clp) = adapter.copy_from_slice_to_channel(chan, 0, &chunk.waveforms[chan]);
            clipped += clp;
            if clp > 0 && nbr > 0 {
                let pk = chunk.waveforms[chan]
                    .iter()
                    .map(|x| x.abs())
                    .fold(0.0, CamillaFloat::max);
                if pk > peak {
                    peak = pk;
                }
            }
        }
    }
    xtrace!("Convert, nbr clipped: {}, peak: {}", clipped, peak);
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak +{:.2} dB ({:.1}%)",
            clipped,
            20.0 * peak.log10(),
            peak * 100.0
        );
    }
    recycle_chunk(chunk);
    (num_valid_bytes, clipped)
}

fn buffer_to_chunk_with_adapter<A>(
    adapter: &A,
    channels: usize,
    num_frames: usize,
    num_valid_frames: usize,
    used_channels: &[bool],
    check_for_nan: bool,
) -> AudioChunk
where
    A: Adapter<CamillaFloat>,
{
    let mut maxvalue: CamillaFloat = 0.0;
    let mut minvalue: CamillaFloat = 0.0;
    let mut wfs = container_from_stash(channels);
    for (ch, used) in used_channels.iter().enumerate() {
        if *used {
            let mut wf = vec_from_stash(num_frames);
            let nbr = adapter.copy_from_channel_to_slice(ch, 0, &mut wf[0..num_valid_frames]);
            if nbr > 0 {
                let (mavx, minv) = if check_for_nan {
                    let mut maxv = 0.0;
                    let mut minv = 0.0;
                    let mut invalid_values = 0;
                    for value in wf.iter_mut() {
                        if !value.is_finite() {
                            invalid_values += 1;
                            *value = 0.0;
                        }
                        if *value > maxv {
                            maxv = *value;
                        } else if *value < minv {
                            minv = *value;
                        }
                    }
                    if invalid_values > 0 {
                        warn!("Ignored {invalid_values} infinite or NaN values in channel {ch}");
                    }
                    (maxv, minv)
                } else {
                    wf.iter().fold((0.0, 0.0), |(max, min), x| {
                        (CamillaFloat::max(max, *x), CamillaFloat::min(min, *x))
                    })
                };
                if mavx > maxvalue {
                    maxvalue = mavx;
                }
                if minv < minvalue {
                    minvalue = minv;
                }
            }
            wfs.push(wf);
        } else {
            wfs.push(vec_from_stash(0));
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames, num_valid_frames)
}

/// Convert an AudioChunk to an interleaved buffer of u8.
pub fn chunk_to_buffer_rawbytes(
    chunk: AudioChunk,
    buf: &mut [u8],
    sample_format: &BinarySampleFormat,
) -> (usize, usize) {
    let frames = chunk.frames;
    let channels = chunk.channels;
    let bytes_per_sample = sample_format.bytes_per_sample();
    match *sample_format {
        BinarySampleFormat::S16_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [I16_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::S24_3_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [I24_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::S24_4_RJ_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [I24_4RJ_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::S24_4_LJ_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [I24_4LJ_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::S32_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [I32_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::F32_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [F32_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
        BinarySampleFormat::F64_LE => chunk_to_buffer_with_adapter(
            chunk,
            &mut InterleavedNumbers::<&mut [F64_LE], CamillaFloat>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
            bytes_per_sample,
        ),
    }
}

/// Convert a buffer of interleaved u8 to an AudioChunk.
pub fn buffer_to_chunk_rawbytes(
    buffer: &[u8],
    channels: usize,
    sample_format: &BinarySampleFormat,
    valid_bytes: usize,
    used_channels: &[bool],
    check_for_nan: bool,
) -> AudioChunk {
    let num_frames = buffer.len() / sample_format.bytes_per_sample() / channels;
    let num_valid_frames = valid_bytes / sample_format.bytes_per_sample() / channels;
    match *sample_format {
        BinarySampleFormat::S16_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[I16_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            false,
        ),
        BinarySampleFormat::S24_3_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[I24_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            false,
        ),
        BinarySampleFormat::S24_4_RJ_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[I24_4RJ_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            false,
        ),
        BinarySampleFormat::S24_4_LJ_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[I24_4LJ_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            false,
        ),
        BinarySampleFormat::S32_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[I32_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            false,
        ),
        BinarySampleFormat::F32_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[F32_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            check_for_nan,
        ),
        BinarySampleFormat::F64_LE => buffer_to_chunk_with_adapter(
            &InterleavedNumbers::<&[F64_LE], CamillaFloat>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
            channels,
            num_frames,
            num_valid_frames,
            used_channels,
            check_for_nan,
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::audiochunk::AudioChunk;
    use crate::config::BinarySampleFormat;
    use crate::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};

    #[test]
    fn to_buffer_int16() {
        let sample_format = BinarySampleFormat::S16_LE;
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 2];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &sample_format);
        let expected = vec![0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_3() {
        let sample_format = BinarySampleFormat::S24_3_LE;
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 6];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &sample_format);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x33, 0x33, 0xF3];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_4() {
        let sample_format = BinarySampleFormat::S24_4_RJ_LE;
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &sample_format);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x00, 0x33, 0x33, 0xF3, 0x00];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn from_buffer_int24_3() {
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x34, 0x33, 0xF3];
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S24_3_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert!(
            (chunk.waveforms[0][0] - chunk2.waveforms[0][0]).abs() < 1.0e-6,
            "{} != {}",
            chunk.waveforms[0][0],
            chunk2.waveforms[0][0]
        );
        assert!(
            (chunk.waveforms[0][1] - chunk2.waveforms[0][1]).abs() < 1.0e-6,
            "{} != {}",
            chunk.waveforms[0][1],
            chunk2.waveforms[0][1]
        );
    }

    #[test]
    fn from_buffer_int24_4() {
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x00, 0x34, 0x33, 0xF3, 0x00];
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S24_4_RJ_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert!(
            (chunk.waveforms[0][0] - chunk2.waveforms[0][0]).abs() < 1.0e-6,
            "{} != {}",
            chunk.waveforms[0][0],
            chunk2.waveforms[0][0]
        );
        assert!(
            (chunk.waveforms[0][1] - chunk2.waveforms[0][1]).abs() < 1.0e-6,
            "{} != {}",
            chunk.waveforms[0][1],
            chunk2.waveforms[0][1]
        );
    }

    #[test]
    fn to_buffer_ignored_int24() {
        let waveforms = vec![vec![0.1, 0.1], Vec::new()];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S24_3_LE);
        let expected = vec![
            0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00,
        ];
        assert_eq!(buffer, expected);

        let waveforms = vec![Vec::new(), vec![0.1, 0.1]];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S24_3_LE);
        let expected = vec![
            0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C,
        ];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int32() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S32_LE);
        #[cfg(camillafloat_f32)]
        let expected = vec![0xD0, 0xCC, 0xCC, 0x0C];
        #[cfg(not(camillafloat_f32))]
        let expected = vec![0xCC, 0xCC, 0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float32() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::F32_LE);
        let expected = vec![0xCD, 0xCC, 0xCC, 0x3D];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float64() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms, 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::F64_LE);
        #[cfg(camillafloat_f32)]
        let expected = vec![0x00, 0x00, 0x00, 0xA0, 0x99, 0x99, 0xB9, 0x3F];
        #[cfg(not(camillafloat_f32))]
        let expected = vec![0x9A, 0x99, 0x99, 0x99, 0x99, 0x99, 0xB9, 0x3F];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_from_buffer_16() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S16_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S16_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S24_4_RJ_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S24_4_RJ_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24_3() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 3];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S24_3_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S24_3_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_32() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S32_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S32_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_16() {
        let waveforms = vec![vec![-1.0, 0.0, 32767.0 / 32768.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S16_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S16_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_24() {
        let waveforms = vec![vec![-1.0, 0.0, 8388607.0 / 8388608.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S24_4_RJ_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S24_4_RJ_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_32() {
        let waveforms = vec![vec![-1.0, 0.0, 2147483647.0 / 2147483648.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::S32_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::S32_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float32() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::F32_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::F32_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float64() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 8];
        chunk_to_buffer_rawbytes(chunk, &mut buffer, &BinarySampleFormat::F64_LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &BinarySampleFormat::F64_LE,
            buffer.len(),
            &[true; 1],
            false,
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }
}
