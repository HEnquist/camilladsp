use std::convert::TryInto;
use std::fs::File;
use std::io::BufReader;
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem;

use crate::config::{ConfigError, SampleFormat};
use crate::Res;

const RIFF: &[u8] = "RIFF".as_bytes();
const WAVE: &[u8] = "WAVE".as_bytes();
const DATA: &[u8] = "data".as_bytes();
const FMT: &[u8] = "fmt ".as_bytes();

/// Windows Guid
/// Used to give sample format in the extended WAVEFORMATEXTENSIBLE wav header
#[derive(Debug, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    fn from_slice(data: &[u8; 16]) -> Guid {
        let data1 = read_u32(data, 0);
        let data2 = read_u16(data, 4);
        let data3 = read_u16(data, 6);
        let data4 = data[8..16].try_into().unwrap_or([0; 8]);
        Guid {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_FLOAT: Guid = Guid {
    data1: 3,
    data2: 0,
    data3: 16,
    data4: [128, 0, 0, 170, 0, 56, 155, 113],
};

/// KSDATAFORMAT_SUBTYPE_PCM
const SUBTYPE_PCM: Guid = Guid {
    data1: 1,
    data2: 0,
    data3: 16,
    data4: [128, 0, 0, 170, 0, 56, 155, 113],
};

#[derive(Debug)]
pub struct WavParams {
    pub sample_format: SampleFormat,
    pub sample_rate: usize,
    pub data_offset: usize,
    pub data_length: usize,
    pub channels: usize,
}

fn read_u32(buffer: &[u8], start_index: usize) -> u32 {
    u32::from_le_bytes(
        buffer[start_index..start_index + mem::size_of::<u32>()]
            .try_into()
            .unwrap_or_default(),
    )
}

fn read_u16(buffer: &[u8], start_index: usize) -> u16 {
    u16::from_le_bytes(
        buffer[start_index..start_index + mem::size_of::<u16>()]
            .try_into()
            .unwrap_or_default(),
    )
}

fn compare_4cc(buffer: &[u8], bytes: &[u8]) -> bool {
    buffer.iter().take(4).zip(bytes).all(|(a, b)| *a == *b)
}

fn look_up_format(
    data: &[u8],
    formatcode: u16,
    bits: u16,
    bytes_per_sample: u16,
    chunk_length: u32,
) -> Res<SampleFormat> {
    match (formatcode, bits, bytes_per_sample) {
        (1, 16, 2) => Ok(SampleFormat::S16LE),
        (1, 24, 3) => Ok(SampleFormat::S24LE3),
        (1, 24, 4) => Ok(SampleFormat::S24LE),
        (1, 32, 4) => Ok(SampleFormat::S32LE),
        (3, 32, 4) => Ok(SampleFormat::FLOAT32LE),
        (3, 64, 8) => Ok(SampleFormat::FLOAT64LE),
        (0xFFFE, _, _) => look_up_extended_format(data, bits, bytes_per_sample, chunk_length),
        (_, _, _) => Err(ConfigError::new("Unsupported wav format").into()),
    }
}

fn look_up_extended_format(
    data: &[u8],
    bits: u16,
    bytes_per_sample: u16,
    chunk_length: u32,
) -> Res<SampleFormat> {
    if chunk_length != 40 {
        return Err(ConfigError::new("Invalid extended header").into());
    }
    let cb_size = read_u16(data, 16);
    let valid_bits_per_sample = read_u16(data, 18);
    let channel_mask = read_u32(data, 20);
    let subformat = &data[24..40];
    let subformat_guid = Guid::from_slice(subformat.try_into().unwrap());
    trace!(
        "Found extended wav fmt chunk: subformatcode: {:?}, cb_size: {}, channel_mask: {}, valid bits per sample: {}",
        subformat_guid, cb_size, channel_mask, valid_bits_per_sample
    );
    match (
        subformat_guid,
        bits,
        bytes_per_sample,
        valid_bits_per_sample,
    ) {
        (SUBTYPE_PCM, 16, 2, 16) => Ok(SampleFormat::S16LE),
        (SUBTYPE_PCM, 24, 3, 24) => Ok(SampleFormat::S24LE3),
        (SUBTYPE_PCM, 24, 4, 24) => Ok(SampleFormat::S24LE),
        (SUBTYPE_PCM, 32, 4, 32) => Ok(SampleFormat::S32LE),
        (SUBTYPE_FLOAT, 32, 4, 32) => Ok(SampleFormat::FLOAT32LE),
        (SUBTYPE_FLOAT, 64, 8, 64) => Ok(SampleFormat::FLOAT64LE),
        (_, _, _, _) => Err(ConfigError::new("Unsupported extended wav format").into()),
    }
}

pub fn find_data_in_wav(filename: &str) -> Res<WavParams> {
    let f = File::open(filename)?;
    find_data_in_wav_stream(f).map_err(|err| {
        ConfigError::new(&format!(
            "Unable to parse wav file '{}', error: {}",
            filename, err
        ))
        .into()
    })
}

pub fn find_data_in_wav_stream(mut f: impl Read + Seek) -> Res<WavParams> {
    let filesize = f.seek(SeekFrom::End(0))?;
    f.seek(SeekFrom::Start(0))?;
    let mut file = BufReader::new(f);
    let mut header = [0; 12];
    file.read_exact(&mut header)?;

    // The file must start with RIFF
    let riff_err = !compare_4cc(&header, RIFF);
    // Bytes 8 to 12 must be WAVE
    let wave_err = !compare_4cc(&header[8..], WAVE);
    if riff_err || wave_err {
        return Err(ConfigError::new("Invalid header").into());
    }

    let mut next_chunk_location = 12;
    let mut found_fmt = false;
    let mut found_data = false;
    let mut buffer = [0; 8];

    // Dummy values until we have found the real ones
    let mut sample_format = SampleFormat::S16LE;
    let mut sample_rate = 0;
    let mut channels = 0;
    let mut data_offset = 0;
    let mut data_length = 0;

    // Analyze each chunk to find format and data
    while (!found_fmt || !found_data) && next_chunk_location < filesize {
        file.seek(SeekFrom::Start(next_chunk_location))?;
        file.read_exact(&mut buffer)?;
        let chunk_length = read_u32(&buffer, 4);
        trace!("Analyzing wav chunk of length: {}", chunk_length);
        let is_data = compare_4cc(&buffer, DATA);
        let is_fmt = compare_4cc(&buffer, FMT);
        if is_fmt && (chunk_length == 16 || chunk_length == 18 || chunk_length == 40) {
            found_fmt = true;
            let mut data = vec![0; chunk_length as usize];
            file.read_exact(&mut data).unwrap();
            let formatcode: u16 = read_u16(&data, 0);
            channels = read_u16(&data, 2);
            sample_rate = read_u32(&data, 4);
            let bytes_per_frame = read_u16(&data, 12);
            let bits = read_u16(&data, 14);
            let bytes_per_sample = bytes_per_frame / channels;
            sample_format =
                look_up_format(&data, formatcode, bits, bytes_per_sample, chunk_length)?;
            trace!(
                "Found wav fmt chunk: formatcode: {}, channels: {}, samplerate: {}, bits: {}, bytes_per_frame: {}",
                formatcode, channels, sample_rate, bits, bytes_per_frame
            );
        } else if is_data {
            found_data = true;
            data_offset = next_chunk_location + 8;
            data_length = chunk_length;
            trace!(
                "Found wav data chunk, start: {}, length: {}",
                data_offset,
                data_length
            )
        }
        next_chunk_location += 8 + chunk_length as u64;
    }
    if found_data && found_fmt {
        trace!("Wav file with parameters: format: {:?},  samplerate: {}, channels: {}, data_length: {}, data_offset: {}", sample_format, sample_rate, channels, data_length, data_offset);
        return Ok(WavParams {
            sample_format,
            sample_rate: sample_rate as usize,
            channels: channels as usize,
            data_length: data_length as usize,
            data_offset: data_offset as usize,
        });
    }
    Err(ConfigError::new("Unable to parse as wav").into())
}

// Write a wav header.
// We don't know the final length so we set the file size and data length to u32::MAX.
pub fn write_wav_header(
    dest: &mut impl Write,
    channels: usize,
    sample_format: SampleFormat,
    samplerate: usize,
) -> std::io::Result<()> {
    // Header
    dest.write_all(RIFF)?;
    // file size, 4 bytes, unknown so set to max
    dest.write_all(&u32::MAX.to_le_bytes())?;
    dest.write_all(WAVE)?;

    let (formatcode, bits_per_sample, bytes_per_sample) = match sample_format {
        SampleFormat::S16LE => (1, 16, 2),
        SampleFormat::S24LE3 => (1, 24, 3),
        SampleFormat::S24LE => (1, 24, 4),
        SampleFormat::S32LE => (1, 32, 4),
        SampleFormat::FLOAT32LE => (3, 32, 4),
        SampleFormat::FLOAT64LE => (3, 64, 8),
    };

    // format block
    dest.write_all(FMT)?;
    // size of fmt block, 4 bytes
    dest.write_all(&16_u32.to_le_bytes())?;
    // format code, 2 bytes
    dest.write_all(&(formatcode as u16).to_le_bytes())?;
    // number of channels, 2 bytes
    dest.write_all(&(channels as u16).to_le_bytes())?;
    // samplerate, 4 bytes
    dest.write_all(&(samplerate as u32).to_le_bytes())?;
    // bytes per second, 4 bytes
    dest.write_all(&((channels * samplerate * bytes_per_sample) as u32).to_le_bytes())?;
    // block alignment, 2 bytes
    dest.write_all(&((channels * bytes_per_sample) as u16).to_le_bytes())?;
    // bits per sample, 2 bytes
    dest.write_all(&(bits_per_sample as u16).to_le_bytes())?;

    // data block
    dest.write_all(DATA)?;
    // data length, 4 bytes, unknown so set to max
    dest.write_all(&u32::MAX.to_le_bytes())?;

    // audio data starts from here
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::find_data_in_wav;
    use super::find_data_in_wav_stream;
    use super::write_wav_header;
    use crate::config::SampleFormat;
    use std::io::Cursor;

    #[test]
    pub fn test_analyze_wav() {
        let info = find_data_in_wav("testdata/int32.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, SampleFormat::S32LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    pub fn test_analyze_wavex() {
        let info = find_data_in_wav("testdata/f32_ex.wav").unwrap();
        println!("{info:?}");
        assert_eq!(info.sample_format, SampleFormat::FLOAT32LE);
        assert_eq!(info.data_offset, 104);
        assert_eq!(info.data_length, 20);
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    fn write_and_read_wav() {
        let bytes = vec![0_u8; 1000];
        let mut buffer = Cursor::new(bytes);
        write_wav_header(&mut buffer, 2, SampleFormat::S32LE, 44100).unwrap();
        let info = find_data_in_wav_stream(buffer).unwrap();
        assert_eq!(info.sample_format, SampleFormat::S32LE);
        assert_eq!(info.data_offset, 44);
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.data_length, u32::MAX as usize);
    }
}
