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

use crate::PrcFmt;
use crate::audiochunk::AudioChunk;
use crate::config::BinarySampleFormat;
use crate::utils::stash::{container_from_stash, recycle_chunk, vec_from_stash};
use audioadapter::{Adapter, AdapterMut};
use audioadapter_buffers::number_to_float::InterleavedNumbers;
use audioadapter_sample::sample::{F32_LE, F64_LE, I16_LE, I24_4LJ_LE, I24_4RJ_LE, I24_LE, I32_LE};
#[cfg(feature = "cpal-backend")]
use num_traits;
#[cfg(feature = "cpal-backend")]
use std::collections::VecDeque;

/// Convert an AudioChunk to an interleaved buffer of u8.
pub fn chunk_to_buffer_rawbytes(
    chunk: AudioChunk,
    buf: &mut [u8],
    sample_format: &BinarySampleFormat,
) -> (usize, usize) {
    let frames = chunk.frames;
    let channels = chunk.channels;
    let mut adapter: Box<dyn AdapterMut<PrcFmt>> = match *sample_format {
        BinarySampleFormat::S16_LE => Box::new(
            InterleavedNumbers::<&mut [I16_LE], PrcFmt>::new_from_bytes_mut(buf, channels, frames)
                .unwrap(),
        ),
        BinarySampleFormat::S24_3_LE => Box::new(
            InterleavedNumbers::<&mut [I24_LE], PrcFmt>::new_from_bytes_mut(buf, channels, frames)
                .unwrap(),
        ),
        BinarySampleFormat::S24_4_RJ_LE => Box::new(
            InterleavedNumbers::<&mut [I24_4RJ_LE], PrcFmt>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
        ),
        BinarySampleFormat::S24_4_LJ_LE => Box::new(
            InterleavedNumbers::<&mut [I24_4LJ_LE], PrcFmt>::new_from_bytes_mut(
                buf, channels, frames,
            )
            .unwrap(),
        ),
        BinarySampleFormat::S32_LE => Box::new(
            InterleavedNumbers::<&mut [I32_LE], PrcFmt>::new_from_bytes_mut(buf, channels, frames)
                .unwrap(),
        ),
        BinarySampleFormat::F32_LE => Box::new(
            InterleavedNumbers::<&mut [F32_LE], PrcFmt>::new_from_bytes_mut(buf, channels, frames)
                .unwrap(),
        ),
        BinarySampleFormat::F64_LE => Box::new(
            InterleavedNumbers::<&mut [F64_LE], PrcFmt>::new_from_bytes_mut(buf, channels, frames)
                .unwrap(),
        ),
    };
    let mut clipped = 0;
    let mut peak: PrcFmt = 0.0;
    let num_valid_bytes = chunk.valid_frames * chunk.channels * sample_format.bytes_per_sample();
    for chan in 0..channels {
        if chunk.waveforms[chan].is_empty() {
            adapter.fill_channel_with(chan, &0.0);
        } else {
            let (nbr, clp) = adapter.copy_from_slice_to_channel(chan, 0, &chunk.waveforms[chan]);
            clipped += clp;
            if clp > 0 && nbr > 0 {
                let pk = chunk.waveforms[chan]
                    .iter()
                    .map(|x| x.abs())
                    .fold(0.0, PrcFmt::max);
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
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = container_from_stash(channels);
    let adapter: Box<dyn Adapter<PrcFmt>> = match *sample_format {
        BinarySampleFormat::S16_LE => Box::new(
            InterleavedNumbers::<&[I16_LE], PrcFmt>::new_from_bytes(buffer, channels, num_frames)
                .unwrap(),
        ),
        BinarySampleFormat::S24_3_LE => Box::new(
            InterleavedNumbers::<&[I24_LE], PrcFmt>::new_from_bytes(buffer, channels, num_frames)
                .unwrap(),
        ),
        BinarySampleFormat::S24_4_RJ_LE => Box::new(
            InterleavedNumbers::<&[I24_4RJ_LE], PrcFmt>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
        ),
        BinarySampleFormat::S24_4_LJ_LE => Box::new(
            InterleavedNumbers::<&[I24_4LJ_LE], PrcFmt>::new_from_bytes(
                buffer, channels, num_frames,
            )
            .unwrap(),
        ),
        BinarySampleFormat::S32_LE => Box::new(
            InterleavedNumbers::<&[I32_LE], PrcFmt>::new_from_bytes(buffer, channels, num_frames)
                .unwrap(),
        ),
        BinarySampleFormat::F32_LE => Box::new(
            InterleavedNumbers::<&[F32_LE], PrcFmt>::new_from_bytes(buffer, channels, num_frames)
                .unwrap(),
        ),
        BinarySampleFormat::F64_LE => Box::new(
            InterleavedNumbers::<&[F64_LE], PrcFmt>::new_from_bytes(buffer, channels, num_frames)
                .unwrap(),
        ),
    };
    for (ch, used) in used_channels.iter().enumerate() {
        if *used {
            let mut wf = vec_from_stash(num_frames);
            let nbr = adapter.copy_from_channel_to_slice(ch, 0, &mut wf[0..num_valid_frames]);
            if nbr > 0 {
                let (mavx, minv) = if check_for_nan
                    && (*sample_format == BinarySampleFormat::F32_LE
                        || *sample_format == BinarySampleFormat::F64_LE)
                {
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
                        (PrcFmt::max(max, *x), PrcFmt::min(min, *x))
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

/// Convert an AudioChunk to an interleaved queue of ints, only used by CPAL backend.
#[cfg(feature = "cpal-backend")]
pub fn chunk_to_queue_int<T: num_traits::cast::NumCast>(
    chunk: &AudioChunk,
    queue: &mut VecDeque<T>,
    scalefactor: PrcFmt,
) -> usize {
    let _num_samples = chunk.channels * chunk.frames;
    let mut value: T;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = if (scalefactor >= 2_147_483_648.0) && cfg!(feature = "32bit") {
        (scalefactor - 128.0) / scalefactor
    } else {
        (scalefactor - 1.0) / scalefactor
    };
    let minval = -1.0;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = if chunk.waveforms[chan].is_empty() {
                0.0
            } else {
                chunk.waveforms[chan][frame]
            };
            if float_val > maxval {
                clipped += 1;
                if float_val > peak {
                    peak = float_val;
                }
                float_val = maxval;
            } else if float_val < minval {
                clipped += 1;
                if -float_val > peak {
                    peak = -float_val;
                }
                float_val = minval;
            }
            value = match num_traits::cast(float_val * scalefactor) {
                Some(val) => val,
                None => {
                    debug!("bad float {}", float_val);
                    num_traits::cast(0.0).unwrap()
                }
            };
            queue.push_back(value);
        }
    }
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak +{:.2} dB ({:.1}%)",
            clipped,
            20.0 * peak.log10(),
            peak * 100.0
        );
    }
    clipped
}

/// Convert a buffer of interleaved ints to an AudioChunk, only used by CPAL backend.
#[cfg(feature = "cpal-backend")]
pub fn queue_to_chunk_int<T: num_traits::cast::AsPrimitive<PrcFmt>>(
    queue: &mut VecDeque<T>,
    num_frames: usize,
    channels: usize,
    scalefactor: PrcFmt,
) -> AudioChunk {
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    for _frame in 0..num_frames {
        for wf in wfs.iter_mut().take(channels) {
            value = queue.pop_front().unwrap().as_();
            value /= scalefactor;
            if value > maxvalue {
                maxvalue = value;
            }
            if value < minvalue {
                minvalue = value;
            }
            wf.push(value);
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames, num_frames)
}

/// Convert an AudioChunk to an interleaved buffer of floats, only used by cpal backend.
#[cfg(feature = "cpal-backend")]
pub fn chunk_to_queue_float<T: num_traits::cast::NumCast>(
    chunk: &AudioChunk,
    queue: &mut VecDeque<T>,
) -> usize {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value: T;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = 1.0;
    let minval = -1.0;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = if chunk.waveforms[chan].is_empty() {
                0.0
            } else {
                chunk.waveforms[chan][frame]
            };
            if float_val > maxval {
                clipped += 1;
                if float_val > peak {
                    peak = float_val;
                }
                float_val = maxval;
            } else if float_val < minval {
                clipped += 1;
                if -float_val > peak {
                    peak = -float_val;
                }
                float_val = minval;
            }
            value = match num_traits::cast(float_val) {
                Some(val) => val,
                None => {
                    debug!("bad float{}", float_val);
                    num_traits::cast(0.0).unwrap()
                }
            };
            queue.push_back(value);
        }
    }
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak +{:.2} dB ({:.1}%)",
            clipped,
            20.0 * peak.log10(),
            peak * 100.0
        );
    }
    clipped
}

/// Convert a buffer of interleaved floats to an AudioChunk, only used by CPAL backend.
#[cfg(feature = "cpal-backend")]
pub fn queue_to_chunk_float<T: num_traits::cast::AsPrimitive<PrcFmt>>(
    queue: &mut VecDeque<T>,
    num_frames: usize,
    channels: usize,
) -> AudioChunk {
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    for _frame in 0..num_frames {
        for wf in wfs.iter_mut().take(channels) {
            value = queue.pop_front().unwrap().as_();
            if value > maxvalue {
                maxvalue = value;
            }
            if value < minvalue {
                minvalue = value;
            }
            wf.push(value);
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames, num_frames)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "cpal-backend")]
    use crate::PrcFmt;
    use crate::audiochunk::AudioChunk;
    use crate::config::BinarySampleFormat;
    use crate::utils::conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
    #[cfg(feature = "cpal-backend")]
    use crate::utils::conversions::{
        chunk_to_queue_float, chunk_to_queue_int, queue_to_chunk_float, queue_to_chunk_int,
    };
    #[cfg(feature = "cpal-backend")]
    use std::collections::VecDeque;

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
        #[cfg(feature = "32bit")]
        let expected = vec![0xD0, 0xCC, 0xCC, 0x0C];
        #[cfg(not(feature = "32bit"))]
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
        #[cfg(feature = "32bit")]
        let expected = vec![0x00, 0x00, 0x00, 0xA0, 0x99, 0x99, 0xB9, 0x3F];
        #[cfg(not(feature = "32bit"))]
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

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn to_from_queue_i16() {
        let bits = 16;
        let scalefactor = (2.0 as PrcFmt).powf((bits - 1) as PrcFmt);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut queue = VecDeque::<i16>::new();
        chunk_to_queue_int(&chunk, &mut queue, scalefactor);
        assert_eq!(queue.len(), 3);
        let chunk2 = queue_to_chunk_int(&mut queue, 3, 1, scalefactor);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
        assert_eq!(queue.len(), 0);
    }

    #[cfg(feature = "cpal-backend")]
    #[test]
    fn to_from_queue_f32() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut queue = VecDeque::<f32>::new();
        chunk_to_queue_float(&chunk, &mut queue);
        assert_eq!(queue.len(), 3);
        let chunk2 = queue_to_chunk_float(&mut queue, 3, 1);
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
        assert_eq!(queue.len(), 0);
    }
}
