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

/// Basic gain, delay, and volume filters.
pub mod basicfilters;
/// Second-order IIR biquad filters.
pub mod biquad;
/// Multi-section biquad combinations (shelves, butterworth, etc.).
pub mod biquadcombo;
/// Hard/soft-clipping filter.
pub mod clipper;
/// Difference-equation (IIR) filter with arbitrary coefficients.
pub mod diffeq;
/// Dithering noise filters.
pub mod dither;
/// Convolution filter via FFT overlap-save.
pub mod fftconv;
/// Lookahead limiter.
pub mod lookahead_limiter;
/// Loudness compensation filter.
pub mod loudness;

use crate::config;
use crate::config::{BinarySampleFormat, Issues, issue_path};
use audioadapter_sample::readwrite::ReadSamples;
use audioadapter_sample::sample::{F32_LE, F64_LE, I16_LE, I24_4LJ_LE, I24_4RJ_LE, I24_LE, I32_LE};
use std::fs::File;
use std::io::BufReader;
use std::io::{BufRead, Seek, SeekFrom};

use crate::CamillaFloat;
use crate::Res;

use audioadapter::Adapter;
use waveadapter::read_wav_file;

/// Read filter coefficients from a file in the specified sample format.
///
/// `skip_bytes_lines` is a byte offset for binary formats or a line count for TEXT.
/// `read_bytes_lines` limits the number of bytes/lines read (0 means read all).
pub fn read_coeff_file(
    filename: &str,
    format: &config::FileSampleFormat,
    read_bytes_lines: usize,
    skip_bytes_lines: usize,
) -> Res<Vec<CamillaFloat>> {
    let mut coefficients = Vec::<CamillaFloat>::new();
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
                    file.read_converted_to_limit_or_end::<I16_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_3_LE => {
                    file.read_converted_to_limit_or_end::<I24_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_4_RJ_LE => {
                    file.read_converted_to_limit_or_end::<I24_4RJ_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S24_4_LJ_LE => {
                    file.read_converted_to_limit_or_end::<I24_4LJ_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::S32_LE => {
                    file.read_converted_to_limit_or_end::<I32_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::F32_LE => {
                    file.read_converted_to_limit_or_end::<F32_LE, CamillaFloat>(
                        &mut coefficients,
                        limit,
                    )?;
                }
                config::BinarySampleFormat::F64_LE => {
                    file.read_converted_to_limit_or_end::<F64_LE, CamillaFloat>(
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

/// Read a single channel of samples from a WAV file, returned as filter coefficients.
pub fn read_wav(filename: &str, channel: usize) -> Res<Vec<CamillaFloat>> {
    let audio = read_wav_file::<CamillaFloat, _>(filename).map_err(|err| {
        config::ConfigError::new(&format!("Can't read wav file '{filename}'. Reason: {err}"))
    })?;
    let channels = audio.channels();
    if channel >= channels {
        let msg = format!(
            "Cant read channel {} of file '{}' which contains {} channels.",
            channel, filename, channels
        );
        return Err(config::ConfigError::new(&msg).into());
    }

    let frames = audio.frames();
    let mut data = vec![0.0; frames];
    audio
        .samples
        .copy_from_channel_to_slice(channel, 0, &mut data);
    debug!(
        "Read wav file '{}', channel: {} of {}, samplerate: {}, length: {}",
        filename,
        channel,
        channels,
        audio.sample_rate,
        data.len()
    );
    Ok(data)
}

/// Validate the filter config, to give a helpful message intead of a panic.
///
/// Issue paths are relative to the filter, so a problem with the cutoff of a
/// biquad is at `["parameters", "freq"]`.
///
/// A convolution filter is validated by reading its impulse response, so the
/// result of that read is kept in `impulses` rather than thrown away. See
/// [`fftconv::ImpulseCache`].
pub fn validate_filter(
    fs: usize,
    name: &str,
    filter_config: &config::Filter,
    impulses: &mut fftconv::ImpulseCache,
) -> Result<(), Issues> {
    let result = match filter_config {
        config::Filter::Conv { parameters, .. } => {
            fftconv::validate_config(name, parameters, impulses)
        }
        config::Filter::Biquad { parameters, .. } => biquad::validate_config(fs, parameters),
        config::Filter::Delay { parameters, .. } => basicfilters::validate_delay_config(parameters),
        config::Filter::Gain { parameters, .. } => basicfilters::validate_gain_config(parameters),
        config::Filter::Dither { parameters, .. } => dither::validate_config(parameters),
        config::Filter::DiffEq { parameters, .. } => diffeq::validate_config(parameters),
        config::Filter::Volume { parameters, .. } => {
            basicfilters::validate_volume_config(parameters)
        }
        config::Filter::Loudness { parameters, .. } => loudness::validate_config(fs, parameters),
        config::Filter::BiquadCombo { parameters, .. } => {
            biquadcombo::validate_config(fs, parameters)
        }
        config::Filter::Clipper { parameters, .. } => clipper::validate_config(parameters),
        config::Filter::LookaheadLimiter { parameters, .. } => {
            lookahead_limiter::validate_config(parameters, fs)
        }
    };
    let mut issues = Issues::new();
    issues.nest_result(issue_path!["parameters"], result);
    issues.into_result(())
}
