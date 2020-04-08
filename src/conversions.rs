extern crate num_traits;
//use std::{iter, error};
use std::convert::TryInto;

//mod audiodevice;
use audiodevice::*;

use PrcFmt;

/// Convert an AudioChunk to an interleaved buffer of u8.
pub fn chunk_to_buffer_bytes(chunk: AudioChunk, buf: &mut [u8], scalefactor: PrcFmt, bits: i32)  -> usize {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value16;
    let mut value32;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = if (bits == 32) && cfg!(feature = "32bit") {
        (scalefactor - 128.0) / scalefactor
    } else {
        (scalefactor - 1.0) / scalefactor
    };
    let bytes_per_sample = match bits {
        16 => 2,
        24 | 32 => 4,
        _ => 1,
    };
    let num_valid_bytes = chunk.valid_frames * chunk.channels * bytes_per_sample;

    let minval = -1.0;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = chunk.waveforms[chan][frame];
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
                value16 = (float_val * scalefactor) as i16;
                let bytes = value16.to_le_bytes();
                for b in &bytes {
                    buf[idx] = *b;
                    idx += 1;
                }
            } else {
                value32 = (float_val * scalefactor) as i32;
                let bytes = value32.to_le_bytes();
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
    num_valid_bytes
}

/// Convert a buffer of interleaved u8 to an AudioChunk.
pub fn buffer_to_chunk_bytes(
    buffer: &[u8],
    channels: usize,
    scalefactor: PrcFmt,
    bits: i32,
    valid_bytes: usize,
) -> AudioChunk {
    let bytes_per_sample = match bits {
        16 => 2,
        24 | 32 => 4,
        _ => 1,
    };
    let num_frames = buffer.len()/bytes_per_sample/channels;
    let num_valid_frames = valid_bytes/bytes_per_sample/channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    let mut idx = 0;
    if bits == 16 {
        for _frame in 0..num_frames {
            for wf in wfs.iter_mut().take(channels) {
                value = i16::from_le_bytes(buffer[idx..idx + 2].try_into().unwrap()) as PrcFmt;
                idx += 2;
                value /= scalefactor;
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wf.push(value);
                //idx += 1;
            }
        }
    } else {
        for _frame in 0..num_frames {
            for wf in wfs.iter_mut().take(channels) {
                value = i32::from_le_bytes(buffer[idx..idx + 4].try_into().unwrap()) as PrcFmt;
                idx += 4;
                value /= scalefactor;
                if value > maxvalue {
                    maxvalue = value;
                }
                if value < minvalue {
                    minvalue = value;
                }
                //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
                wf.push(value);
                //idx += 1;
            }
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_valid_frames)
}

/// Convert an AudioChunk to an interleaved buffer of floats stored as u8.
pub fn chunk_to_buffer_float_bytes(chunk: AudioChunk, buf: &mut [u8], bits: i32) -> usize {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value64;
    let mut value32;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = 1.0;
    let minval = -1.0;
    let bytes_per_sample = bits as usize/8;
    let num_valid_bytes = chunk.valid_frames * chunk.channels * bytes_per_sample;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = chunk.waveforms[chan][frame];
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
    num_valid_bytes
}

/// Convert a buffer of interleaved u8 to an AudioChunk.
pub fn buffer_to_chunk_float_bytes(buffer: &[u8], channels: usize, bits: i32, valid_bytes: usize) -> AudioChunk {
    let bytes_per_sample = bits as usize/8;
    let num_frames = buffer.len()/bytes_per_sample/channels;
    let num_valid_frames = valid_bytes/bytes_per_sample/channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    let mut idx = 0;
    if bits == 32 {
        for _frame in 0..num_frames {
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
                wf.push(value);
                //idx += 1;
            }
        }
    } else {
        for _frame in 0..num_frames {
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
                wf.push(value);
                //idx += 1;
            }
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_valid_frames)
}

/// Convert an AudioChunk to an interleaved buffer of ints.
#[allow(dead_code)]
pub fn chunk_to_buffer_int<T: num_traits::cast::NumCast>(
    chunk: AudioChunk,
    buf: &mut [T],
    scalefactor: PrcFmt,
) {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value: T;
    let mut idx = 0;
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
            let mut float_val = chunk.waveforms[chan][frame];
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
            buf[idx] = value;
            idx += 1;
        }
    }
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak {}%",
            clipped,
            peak * 100.0
        );
    }
    //buf
}

/// Convert a buffer of interleaved ints to an AudioChunk.
#[allow(dead_code)]
pub fn buffer_to_chunk_int<T: num_traits::cast::AsPrimitive<PrcFmt>>(
    buffer: &[T],
    channels: usize,
    scalefactor: PrcFmt,
) -> AudioChunk {
    let num_samples = buffer.len();
    let num_frames = num_samples / channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    //let mut idx = 0;
    //let mut samples = buffer.iter();
    let mut idx = 0;
    for _frame in 0..num_frames {
        for wf in wfs.iter_mut().take(channels) {
            value = buffer[idx].as_();
            idx += 1;
            value /= scalefactor;
            if value > maxvalue {
                maxvalue = value;
            }
            if value < minvalue {
                minvalue = value;
            }
            //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
            wf.push(value);
            //idx += 1;
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames)
}

/// Convert an AudioChunk to an interleaved buffer of floats.
#[allow(dead_code)]
pub fn chunk_to_buffer_float<T: num_traits::cast::NumCast>(chunk: AudioChunk, buf: &mut [T]) {
    let _num_samples = chunk.channels * chunk.frames;
    //let mut buf = Vec::with_capacity(num_samples);
    let mut value: T;
    let mut idx = 0;
    let mut clipped = 0;
    let mut peak = 0.0;
    let maxval = 1.0;
    let minval = -1.0;
    for frame in 0..chunk.frames {
        for chan in 0..chunk.channels {
            let mut float_val = chunk.waveforms[chan][frame];
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
            buf[idx] = value;
            idx += 1;
        }
    }
    if clipped > 0 {
        warn!(
            "Clipping detected, {} samples clipped, peak {}%",
            clipped,
            peak * 100.0
        );
    }
    //buf
}

/// Convert a buffer of interleaved ints to an AudioChunk.
#[allow(dead_code)]
pub fn buffer_to_chunk_float<T: num_traits::cast::AsPrimitive<PrcFmt>>(
    buffer: &[T],
    channels: usize,
) -> AudioChunk {
    let num_samples = buffer.len();
    let num_frames = num_samples / channels;
    let mut value: PrcFmt;
    let mut maxvalue: PrcFmt = 0.0;
    let mut minvalue: PrcFmt = 0.0;
    let mut wfs = Vec::with_capacity(channels);
    for _chan in 0..channels {
        wfs.push(Vec::with_capacity(num_frames));
    }
    //let mut idx = 0;
    //let mut samples = buffer.iter();
    let mut idx = 0;
    for _frame in 0..num_frames {
        for wf in wfs.iter_mut().take(channels) {
            value = buffer[idx].as_();
            idx += 1;
            if value > maxvalue {
                maxvalue = value;
            }
            if value < minvalue {
                minvalue = value;
            }
            //value = (self.buffer[idx] as f32) / ((1<<15) as f32);
            wf.push(value);
            //idx += 1;
        }
    }
    AudioChunk::new(wfs, maxvalue, minvalue, num_frames)
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use audiodevice::AudioChunk;
    use conversions::{
        buffer_to_chunk_bytes, buffer_to_chunk_float_bytes, chunk_to_buffer_bytes,
        chunk_to_buffer_float_bytes,
    };

    #[test]
    fn to_from_buffer_16() {
        let bits = 16;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_24() {
        let bits = 24;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_32() {
        let bits = 32;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_16() {
        let bits = 16;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-1.0, 0.0, 32767.0 / 32768.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 2];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_24() {
        let bits = 24;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        let waveforms = vec![vec![-1.0, 0.0, 8388607.0 / 8388608.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn clipping_32() {
        let bits = 32;
        let scalefactor = (2.0 as PrcFmt).powi(bits - 1);
        #[cfg(feature = "32bit")]
        let waveforms = vec![vec![-1.0, 0.0, 2147483520.0 / 2147483648.0]; 1];
        #[cfg(not(feature = "32bit"))]
        let waveforms = vec![vec![-1.0, 0.0, 2147483647.0 / 2147483648.0]; 1];
        let chunk = AudioChunk::new(vec![vec![-2.0, 0.0, 2.0]; 1], 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_bytes(chunk, &mut buffer, scalefactor, bits);
        let chunk2 = buffer_to_chunk_bytes(&buffer, 1, scalefactor, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float32() {
        let bits = 32;
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 4];
        chunk_to_buffer_float_bytes(chunk, &mut buffer, bits);
        let chunk2 = buffer_to_chunk_float_bytes(&buffer, 1, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }

    #[test]
    fn to_from_buffer_float64() {
        let bits = 64;
        let waveforms = vec![vec![-0.5, 0.0, 0.5]; 1];
        let chunk = AudioChunk::new(waveforms.clone(), 0.0, 0.0, 3);
        let mut buffer = vec![0u8; 3 * 8];
        chunk_to_buffer_float_bytes(chunk, &mut buffer, bits);
        let chunk2 = buffer_to_chunk_float_bytes(&buffer, 1, bits, buffer.len());
        assert_eq!(waveforms[0], chunk2.waveforms[0]);
    }
}
