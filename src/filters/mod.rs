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

pub mod basicfilters;
pub mod biquad;
pub mod biquadcombo;
pub mod diffeq;
pub mod dither;
pub mod fftconv;
pub mod limiter;
pub mod loudness;

use crate::config;
use crate::config::BinarySampleFormat;
use audioadapter_sample::readwrite::ReadSamples;
use audioadapter_sample::sample::{F32_LE, F64_LE, I16_LE, I24_4LJ_LE, I24_4RJ_LE, I24_LE, I32_LE};
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Seek, SeekFrom};

use crate::PrcFmt;
use crate::Res;

use crate::utils::wavtools::find_data_in_wav;

pub trait Filter {
    // Filter a Vec
    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()>;

    fn update_parameters(&mut self, config: config::Filter);

    fn name(&self) -> &str;
}

pub fn pad_vector(values: &[PrcFmt], length: usize) -> Vec<PrcFmt> {
    let new_len = if values.len() > length {
        values.len()
    } else {
        length
    };
    let mut new_values: Vec<PrcFmt> = vec![0.0; new_len];
    new_values[0..values.len()].copy_from_slice(values);
    new_values
}

pub fn read_coeff_file(
    filename: &str,
    format: &config::FileSampleFormat,
    read_bytes_lines: usize,
    skip_bytes_lines: usize,
) -> Res<Vec<PrcFmt>> {
    let mut coefficients = Vec::<PrcFmt>::new();
    let f = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("Could not open coefficient file '{filename}'. Reason: {err}");
            return Err(config::ConfigError::new(&msg).into());
        }
    };
    let mut file = BufReader::new(&f);
    let read_bytes_lines = if read_bytes_lines > 0 {
        read_bytes_lines
    } else {
        usize::MAX
    };

    match format {
        // Handle TEXT separately
        config::FileSampleFormat::TEXT => {
            for (nbr, line) in file
                .lines()
                .skip(skip_bytes_lines)
                .take(read_bytes_lines)
                .enumerate()
            {
                match line {
                    Err(err) => {
                        let msg = format!(
                            "Can't read line {} of file '{}'. Reason: {}",
                            nbr + 1 + skip_bytes_lines,
                            filename,
                            err
                        );
                        return Err(config::ConfigError::new(&msg).into());
                    }
                    Ok(l) => match l.trim().parse() {
                        Ok(val) => coefficients.push(val),
                        Err(err) => {
                            let msg = format!(
                                "Can't parse value on line {} of file '{}'. Reason: {}",
                                nbr + 1 + skip_bytes_lines,
                                filename,
                                err
                            );
                            return Err(config::ConfigError::new(&msg).into());
                        }
                    },
                }
            }
        }
        // All other formats
        _ => {
            let binary_format = BinarySampleFormat::from_file_sample_format(format);
            file.seek(SeekFrom::Start(skip_bytes_lines as u64))?;
            let nbr_coeffs = read_bytes_lines / binary_format.bytes_per_sample();
            let limit = if nbr_coeffs > 0 {
                Some(nbr_coeffs)
            } else {
                None
            };

            match binary_format {
                config::BinarySampleFormat::S16_LE => {
                    file.read_converted_to_limit_or_end::<I16_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_3_LE => {
                    file.read_converted_to_limit_or_end::<I24_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_4_RJ_LE => {
                    file.read_converted_to_limit_or_end::<I24_4RJ_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_4_LJ_LE => {
                    file.read_converted_to_limit_or_end::<I24_4LJ_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S32_LE => {
                    file.read_converted_to_limit_or_end::<I32_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::F32_LE => {
                    file.read_converted_to_limit_or_end::<F32_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::F64_LE => {
                    file.read_converted_to_limit_or_end::<F64_LE, PrcFmt>(
                        &mut coefficients,
                        limit,
                    )?;
                }
            }
            debug!("Read {} coeffs from file", coefficients.len());
        }
    }
    debug!(
        "Read raw data from: '{}', format: {:?}, number of coeffs: {}",
        filename,
        format,
        coefficients.len()
    );
    Ok(coefficients)
}

pub fn read_wav(filename: &str, channel: usize) -> Res<Vec<PrcFmt>> {
    let params = find_data_in_wav(filename)?;
    if channel >= params.channels {
        let msg = format!(
            "Cant read channel {} of file '{}' which contains {} channels.",
            channel, filename, params.channels
        );
        return Err(config::ConfigError::new(&msg).into());
    }

    let alldata = read_coeff_file(
        filename,
        &params.sample_format.to_file_sample_format(),
        params.data_length,
        params.data_offset,
    )?;

    let data = alldata
        .iter()
        .skip(channel)
        .step_by(params.channels)
        .copied()
        .collect::<Vec<PrcFmt>>();
    debug!(
        "Read wav file '{}', format: {:?}, channel: {} of {}, samplerate: {}, length: {}",
        filename,
        params.sample_format,
        channel,
        params.channels,
        params.sample_rate,
        data.len()
    );
    Ok(data)
}

/// Validate the filter config, to give a helpful message intead of a panic.
pub fn validate_filter(fs: usize, filter_config: &config::Filter) -> Res<()> {
    match filter_config {
        config::Filter::Conv { parameters, .. } => fftconv::validate_config(parameters),
        config::Filter::Biquad { parameters, .. } => biquad::validate_config(fs, parameters),
        config::Filter::Delay { parameters, .. } => basicfilters::validate_delay_config(parameters),
        config::Filter::Gain { parameters, .. } => basicfilters::validate_gain_config(parameters),
        config::Filter::Dither { parameters, .. } => dither::validate_config(parameters),
        config::Filter::DiffEq { parameters, .. } => diffeq::validate_config(parameters),
        config::Filter::Volume { parameters, .. } => {
            basicfilters::validate_volume_config(parameters)
        }
        config::Filter::Loudness { parameters, .. } => loudness::validate_config(parameters),
        config::Filter::BiquadCombo { parameters, .. } => {
            biquadcombo::validate_config(fs, parameters)
        }
        config::Filter::Limiter { parameters, .. } => limiter::validate_config(parameters),
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use crate::config::FileSampleFormat;
    use crate::filters::read_wav;
    use crate::filters::{pad_vector, read_coeff_file};

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{} - {} = {}", left, right, left - right);
        let res = (left - right).abs() < maxdiff;
        println!("Ok: {res}");
        res
    }

    fn compare_waveforms(left: &[PrcFmt], right: &[PrcFmt], maxdiff: PrcFmt) -> bool {
        if left.len() != right.len() {
            println!("wrong length");
            return false;
        }
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn read_float32() {
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileSampleFormat::F32_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float32.raw", &FileSampleFormat::F32_LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_float64() {
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileSampleFormat::F64_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/float64.raw", &FileSampleFormat::F64_LE, 24, 8).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-15),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int16() {
        let loaded =
            read_coeff_file("testdata/int16.raw", &FileSampleFormat::S16_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int16.raw", &FileSampleFormat::S16_LE, 6, 2).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-4),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn read_int24() {
        let loaded =
            read_coeff_file("testdata/int24.raw", &FileSampleFormat::S24_4_RJ_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int24.raw", &FileSampleFormat::S24_4_RJ_LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int24_3() {
        let loaded =
            read_coeff_file("testdata/int243.raw", &FileSampleFormat::S24_3_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int243.raw", &FileSampleFormat::S24_3_LE, 9, 3).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-6),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_int32() {
        let loaded =
            read_coeff_file("testdata/int32.raw", &FileSampleFormat::S32_LE, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/int32.raw", &FileSampleFormat::S32_LE, 12, 4).unwrap();
        let expected: Vec<PrcFmt> = vec![-0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }
    #[test]
    fn read_text() {
        let loaded = read_coeff_file("testdata/text.txt", &FileSampleFormat::TEXT, 0, 0).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
        let loaded =
            read_coeff_file("testdata/text_header.txt", &FileSampleFormat::TEXT, 4, 1).unwrap();
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5];
        assert!(
            compare_waveforms(&loaded, &expected, 1e-9),
            "{loaded:?} != {expected:?}"
        );
    }

    #[test]
    fn test_padding() {
        let values: Vec<PrcFmt> = vec![1.0, 0.5];
        let values_padded: Vec<PrcFmt> = vec![1.0, 0.5, 0.0, 0.0, 0.0];
        let values_0 = pad_vector(&values, 0);
        assert!(compare_waveforms(&values, &values_0, 1e-15));
        let values_5 = pad_vector(&values, 5);
        assert!(compare_waveforms(&values_padded, &values_5, 1e-15));
    }

    #[test]
    pub fn test_read_wav() {
        let values = read_wav("testdata/int32.wav", 0).unwrap();
        println!("{values:?}");
        let expected: Vec<PrcFmt> = vec![-1.0, -0.5, 0.0, 0.5, 1.0];
        assert!(compare_waveforms(&values, &expected, 1e-9));
        let bad = read_wav("testdata/int32.wav", 1);
        assert!(bad.is_err());
    }
}
