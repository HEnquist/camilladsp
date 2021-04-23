use audiodevice::*;
use num_traits;
use std::collections::VecDeque;
use std::convert::TryInto;
use PrcFmt;

/// Convert an AudioChunk to an interleaved buffer of u8.
pub fn chunk_to_buffer_bytes(
    chunk: &AudioChunk,
    buf: &mut [u8],
    scalefactor: PrcFmt,
    bits: i32,
    bytes_per_sample: usize,
) -> (usize, usize) {
    //let _num_samples = chunk.channels * chunk.frames;
    let data_bytes_per_sample = bits as usize / 8;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = if (bits == 32) && cfg!(feature = "32bit") {
        (scalefactor - 128.0) / scalefactor
    } else {
        (scalefactor - 1.0) / scalefactor
    };
    let num_valid_bytes = chunk.valid_frames * chunk.channels * bytes_per_sample;

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
            if bits == 16 {
                let value16 = (float_val * scalefactor) as i16;
                let bytes = value16.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            } else {
                let mut value32 = (float_val * scalefactor) as i32;
                value32 <<= 8 * (bytes_per_sample - data_bytes_per_sample);
                let bytes = value32.to_le_bytes();
                for b in bytes
                    .iter()
                    .skip(bytes_per_sample - data_bytes_per_sample)
                    .take(data_bytes_per_sample)
                {
                    buf[idx] = *b;
                    idx += 1;
                }
                for _ in 0..(bytes_per_sample - data_bytes_per_sample) {
                    buf[idx] = 0;
                    idx += 1;
                }
            }
        }
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
pub fn buffer_to_chunk_bytes(
    buffer: &[u8],
    channels: usize,
    scalefactor: PrcFmt,
    bits: i32,
    bytes_per_sample: usize,
    valid_bytes: usize,
    used_channels: &[bool],
) -> AudioChunk {
    let data_bytes_per_sample = bits as usize / 8;
    let num_frames = buffer.len() / bytes_per_sample / channels;
    let num_valid_frames = valid_bytes / bytes_per_sample / channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for used in used_channels.iter() {
        if *used {
            wfs.push(vec![0.0; num_frames]);
        } else {
            wfs.push(Vec::new());
        }
    }
    let mut idx = 0;
    let mut valbuf: [u8; 4] = [0; 4];
    for frame in 0..num_frames {
        for wf in wfs.iter_mut().take(channels) {
            if !wf.is_empty() {
                for (n, b) in buffer[idx..idx + data_bytes_per_sample].iter().enumerate() {
                    valbuf[n + 4 - data_bytes_per_sample] = *b;
                }
                value = (i32::from_le_bytes(valbuf) >> (8 * (4 - data_bytes_per_sample))) as PrcFmt;
                value /= scalefactor;
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                wf[frame] = value;
            }
            idx += bytes_per_sample;
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames, num_valid_frames)
}

/// Convert an AudioChunk to an interleaved buffer of floats stored as u8.
pub fn chunk_to_buffer_float_bytes(
    chunk: &AudioChunk,
    buf: &mut [u8],
    bits: i32,
) -> (usize, usize) {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value64;
    let mut value32;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = 1.0;
    let minval = -1.0;
    let bytes_per_sample = bits as usize / 8;
    let num_valid_bytes = chunk.valid_frames * chunk.channels * bytes_per_sample;
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
            if bits == 32 {
                value32 = float_val as f32;
                let bytes = value32.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            } else {
                value64 = float_val as f64;
                let bytes = value64.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            }
        }
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
pub fn buffer_to_chunk_float_bytes(
    buffer: &[u8],
    channels: usize,
    bits: i32,
    valid_bytes: usize,
) -> AudioChunk {
    let bytes_per_sample = bits as usize / 8;
    let num_frames = buffer.len() / bytes_per_sample / channels;
    let num_valid_frames = valid_bytes / bytes_per_sample / channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(vec![0.0; num_frames]);
    }
    let mut idx = 0;
    if bits == 32 {
        for frame in 0..num_frames {
            for wf in wfs.iter_mut().take(channels) {
                value = f32::from_le_bytes(buffer[idx..idx + 4].try_into().unwrap()) as PrcFmt;
                idx += 4;
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wf[frame] = value;
                //idx += 1;
            }
        }
    } else {
        for frame in 0..num_frames {
            for wf in wfs.iter_mut().take(channels) {
                value = f64::from_le_bytes(buffer[idx..idx + 8].try_into().unwrap()) as PrcFmt;
                idx += 8;
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wf[frame] = value;
                //idx += 1;
            }
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames, num_valid_frames)
}

/// Convert an AudioChunk to an interleaved queue of ints.
#[allow(dead_code)]
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

/// Convert a buffer of interleaved ints to an AudioChunk.
#[allow(dead_code)]
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

/// Convert an AudioChunk to an interleaved buffer of floats.
#[allow(dead_code)]
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

/// Convert a buffer of interleaved ints to an AudioChunk.
#[allow(dead_code)]
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
    use crate::PrcFmt;
    use audiodevice::AudioChunk;
    use conversions::{
        buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
        chunk_to_buffer_float_bytes, chunk_to_queue_float, chunk_to_queue_int,
        queue_to_chunk_float, queue_to_chunk_int,
    };
    use std::collections::VecDeque;

    #[test]
    fn to_buffer_int16() {
        let bits = 16;
        let bytes_per_sample = 2;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 2];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_3() {
        let bits = 24;
        let bytes_per_sample = 3;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 6];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x34, 0x33, 0xF3];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int24_4() {
        let bits = 24;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![0xCC, 0xCC, 0x0C, 0x00, 0x34, 0x33, 0xF3, 0x00];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn from_buffer_int24_3() {
        let bits = 24;
        let bytes_per_sample = 3;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x34, 0x33, 0xF3];
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
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
        let bits = 24;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1, -0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let buffer = vec![0xCC, 0xCC, 0x0C, 0x00, 0x34, 0x33, 0xF3, 0x00];
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
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
        let bits = 24;
        let bytes_per_sample = 3;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1, 0.1], Vec::new()];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![
            0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00,
        ];
        assert_eq!(buffer, expected);

        let waveforms = vec![Vec::new(), vec![0.1, 0.1]];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 2, 2);
        let mut buffer = vec![0u8; 12];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![
            0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0x0C,
        ];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_int32() {
        let bits = 32;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let expected = vec![0xCC, 0xCC, 0xCC, 0x0C];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float32() {
        let bits = 32;
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 4];
        chunk_to_buffer_float_bytes(&chunk, &mut buffer, bits);
        let expected = vec![0xCD, 0xCC, 0xCC, 0x3D];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_buffer_float64() {
        let bits = 64;
        let waveforms = vec![vec![0.1]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 1, 1);
        let mut buffer = vec![0u8; 8];
        chunk_to_buffer_float_bytes(&chunk, &mut buffer, bits);
        let expected = vec![0x9A, 0x99, 0x99, 0x99, 0x99, 0x99, 0xB9, 0x3F];
        assert_eq!(buffer, expected);
    }

    #[test]
    fn to_from_buffer_16() {
        let bits = 16;
        let bytes_per_sample = 2;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24() {
        let bits = 24;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24_3() {
        let bits = 24;
        let bytes_per_sample = 3;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 3];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_32() {
        let bits = 32;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_16() {
        let bits = 16;
        let bytes_per_sample = 2;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-1.0, 0.0, 32767.0 / 32768.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_24() {
        let bits = 24;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-1.0, 0.0, 8388607.0 / 8388608.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_32() {
        let bits = 32;
        let bytes_per_sample = 4;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        #[cfg(feature = "32bit")]
        let waveforms = vec![vec![-1.0, 0.0, 2147483520.0 / 2147483648.0]; 1];
        #[cfg(not(feature = "32bit"))]
        let waveforms = vec![vec![-1.0, 0.0, 2147483647.0 / 2147483648.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(&chunk, &mut buffer, scalefactor, bits, bytes_per_sample);
        let chunk2 = buffer_to_chunk_bytes(
            &buffer,
            1,
            scalefactor,
            bits,
            bytes_per_sample,
            buffer.len(),
            &vec![true; 1],
        );
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float32() {
        let bits = 32;
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_float_bytes(&chunk, &mut buffer, bits);
        let chunk2 = buffer_to_chunk_float_bytes(&buffer, 1, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float64() {
        let bits = 64;
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3, 3);
        let mut buffer = vec![0u8; 3 * 8];
        chunk_to_buffer_float_bytes(&chunk, &mut buffer, bits);
        let chunk2 = buffer_to_chunk_float_bytes(&buffer, 1, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

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
