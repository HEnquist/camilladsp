use audiodevice::*;
use config::{FileFormat, SampleFormat};
#[cfg(feature = "cpal-backend")]
use num_traits;
use rawsample;
use rawsample::{SampleReader, SampleWriter};
#[cfg(feature = "cpal-backend")]
use std::collections::VecDeque;
#[cfg(feature = "cpal-backend")]
use std::convert::TryInto;
use std::io::Cursor;
use PrcFmt;

pub fn map_formats(sampleformat: &SampleFormat) -> rawsample::SampleFormat {
    match sampleformat {
        SampleFormat::S16LE => rawsample::SampleFormat::S16LE,
        SampleFormat::S24LE3 => rawsample::SampleFormat::S24LE3,
        SampleFormat::S24LE => rawsample::SampleFormat::S24LE4,
        SampleFormat::S32LE => rawsample::SampleFormat::S32LE,
        SampleFormat::FLOAT32LE => rawsample::SampleFormat::F32LE,
        SampleFormat::FLOAT64LE => rawsample::SampleFormat::F64LE,
    }
}

pub fn map_file_formats(fileformat: &FileFormat) -> rawsample::SampleFormat {
    match fileformat {
        FileFormat::S16LE => rawsample::SampleFormat::S16LE,
        FileFormat::S24LE3 => rawsample::SampleFormat::S24LE3,
        FileFormat::S24LE => rawsample::SampleFormat::S24LE4,
        FileFormat::S32LE => rawsample::SampleFormat::S32LE,
        FileFormat::FLOAT32LE => rawsample::SampleFormat::F32LE,
        FileFormat::FLOAT64LE => rawsample::SampleFormat::F64LE,
        FileFormat::TEXT => panic!("This function does not support text format."),
    }
}

/// Convert an AudioChunk to an interleaved buffer of u8.
pub fn chunk_to_buffer_rawbytes(
    chunk: &AudioChunk,
    buf: &mut [u8],
    sampleformat: &SampleFormat,
) -> (usize, usize) {
    //let _num_samples = chunk.channels * chunk.frames;
    //let data_bytes_per_sample = bits as usize / 8;
    let rawformat = map_formats(sampleformat);
    let mut cursor = Cursor::new(buf);
    let mut clipped = 0;
    let mut peak = 0.0;
    let num_valid_bytes = chunk.valid_frames * chunk.channels * sampleformat.bytes_per_sample();
    let mut nextframe = vec![0.0; chunk.channels];
    for frame in 0..chunk.frames {
        //for chan in 0..chunk.channels {
        for (chan, value) in nextframe.iter_mut().enumerate() {
            let float_val = if chunk.waveforms[chan].is_empty() {
                0.0
            } else {
                chunk.waveforms[chan][frame]
            };
            if float_val.abs() > peak {
                peak = float_val.abs();
            }
            *value = float_val;
        }
        clipped += f64::write_samples(&nextframe, &mut cursor, &rawformat).unwrap();
    }
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak {}%",
            clipped,
            peak * 100.0
        );
    }
    (num_valid_bytes, clipped)
}

/// Convert a buffer of interleaved u8 to an AudioChunk.
pub fn buffer_to_chunk_rawbytes(
    buffer: &[u8],
    channels: usize,
    sampleformat: &SampleFormat,
    valid_bytes: usize,
    used_channels: &[bool],
) -> AudioChunk {
    let num_frames = buffer.len() / sampleformat.bytes_per_sample() / channels;
    let num_valid_frames = valid_bytes / sampleformat.bytes_per_sample() / channels;
    let rawformat = map_formats(sampleformat);
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut slice: &[u8] = &buffer;
    let mut wfs = Vec::with_capacity(channels);
    for used in used_channels.iter() {
        if *used {
            wfs.push(Vec::with_capacity(num_frames));
        } else {
            wfs.push(Vec::new());
        }
    }
    let mut nextframe = vec![0.0; channels];
    for _frame in 0..num_frames {
        PrcFmt::read_samples(&mut slice, &mut nextframe, &rawformat).unwrap();
        for ((wf, value), used) in wfs.iter_mut().zip(&nextframe).zip(used_channels) {
            if *used {
                if *value > maxvalue {
                    maxvalue = *value;
                } else if *value < minvalue {
                    minvalue = *value;
                }
                wf.push(*value);
            }
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
            "Clipping detected, {} samples clipped, peak {}%",
            clipped,
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
            "Clipping detected, {} samples clipped, peak {}%",
            clipped,
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
    use audiodevice::AudioChunk;
    use config::SampleFormat;
    use conversions::{buffer_to_chunk_rawbytes, chunk_to_buffer_rawbytes};
    #[cfg(feature = "cpal-backend")]
    use conversions::{
        chunk_to_queue_float, chunk_to_queue_int, queue_to_chunk_float, queue_to_chunk_int,
    };
    #[cfg(feature = "cpal-backend")]
    use std::collections::VecDeque;

    #[test]
    fn to_buffer_int16() {
        let sampleformat = SampleFormat::S16LE;
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 2];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &sampleformat);
        let expected = vec![0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_3() {
        let sampleformat = SampleFormat::S24LE3;
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 6];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &sampleformat);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x33, 0x33, 0xF3];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_4() {
        let sampleformat = SampleFormat::S24LE;
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &sampleformat);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x00, 0x33, 0x33, 0xF3, 0x00];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn from_buffer_int24_3() {
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x34, 0x33, 0xF3];
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S24LE3,
            buffer.len(),
            &vec![true; 1],
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
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x00, 0x34, 0x33, 0xF3, 0x00];
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S24LE,
            buffer.len(),
            &vec![true; 1],
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
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S24LE3);
        let expected = vec![
            0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00,
        ];
        assert_eq!(buffer, expected);

        let waveforms = vec![Vec::new(), vec![0.1, 0.1]];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S24LE3);
        let expected = vec![
            0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C,
        ];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int32() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S32LE);
        let expected = vec![0xCC, 0xCC, 0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float32() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::FLOAT32LE);
        let expected = vec![0xCD, 0xCC, 0xCC, 0x3D];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float64() {
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::FLOAT64LE);
        let expected = vec![0x9A, 0x99, 0x99, 0x99, 0x99, 0x99, 0xB9, 0x3F];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_from_buffer_16() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S16LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S16LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S24LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S24LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24_3() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 3];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S24LE3);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S24LE3,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_32() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S32LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S32LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_16() {
        let waveforms = vec![vec![-1.0, 0.0, 32767.0 / 32768.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S16LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S16LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_24() {
        let waveforms = vec![vec![-1.0, 0.0, 8388607.0 / 8388608.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S24LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S24LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_32() {
        #[cfg(feature = "32bit")]
        let waveforms = vec![vec![-1.0, 0.0, 2147483520.0 / 2147483648.0]; 1];
        #[cfg(not(feature = "32bit"))]
        let waveforms = vec![vec![-1.0, 0.0, 2147483647.0 / 2147483648.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::S32LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &SampleFormat::S32LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float32() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::FLOAT32LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &&SampleFormat::FLOAT32LE,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float64() {
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 8];
        chunk_to_buffer_rawbytes(&chunk, &mut buffer, &SampleFormat::FLOAT64LE);
        let chunk2 = buffer_to_chunk_rawbytes(
            &buffer,
            1,
            &&SampleFormat::FLOAT64LE,
            buffer.len(),
            &vec![true; 1],
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
