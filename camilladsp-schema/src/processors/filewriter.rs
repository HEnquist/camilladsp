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

use crate::config;
use crate::config::{BinarySampleFormat, Issues, issue_path};
use crate::processors::check_channel_lists;
use std::path::{Path, PathBuf};

/// Token in a FileWriter filename, replaced by the local time when the file is created.
pub const TIMESTAMP_TOKEN: &str = "$timestamp$";

/// The key that identifies an output file: its canonical parent directory
/// joined with the file name.
///
/// The file itself need not exist, but its directory must.
pub fn file_key(filename: &str) -> std::io::Result<PathBuf> {
    let path = Path::new(filename);
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the path has no file name"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(parent)?.join(name))
}

/// Validate the file writer config.
///
/// Issue paths are relative to the parameters.
pub fn validate_file_writer(config: &config::FileWriterParameters) -> Result<(), Issues> {
    let mut issues = Issues::new();
    check_channel_lists(
        &mut issues,
        config.channels,
        &[],
        &config.process_channels(),
    );
    if config.filename.is_empty() {
        issues.invalid(issue_path!["filename"], "Filename must not be empty.");
    } else {
        let in_directory = Path::new(&config.filename)
            .parent()
            .is_some_and(|dir| dir.to_string_lossy().contains(TIMESTAMP_TOKEN));
        if in_directory {
            issues.invalid(
                issue_path!["filename"],
                "Can only use $timestamp$ in the file name, not the directory.",
            );
        } else if let Err(err) = file_key(&config.filename) {
            let msg = format!("Cannot write to '{}': {}", config.filename, err);
            issues.invalid(issue_path!["filename"], msg);
        }
    }
    if config.wav_header() && config.format == BinarySampleFormat::S24_4_RJ_LE {
        issues.invalid(
            issue_path!["format"],
            "Wav files do not support the S24_4_RJ_LE sample format",
        );
    }
    issues.into_result(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_config(filename: &str) -> config::FileWriterParameters {
        config::FileWriterParameters {
            channels: 2,
            process_channels: None,
            filename: filename.to_string(),
            format: BinarySampleFormat::F32_LE,
            wav_header: Some(false),
        }
    }

    fn temp_file(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn validate_rejects_invalid_process_channel() {
        let mut config = f32_config(&temp_file("cap.raw"));
        config.process_channels = Some(vec![2]);
        assert!(validate_file_writer(&config).is_err());
    }

    #[test]
    fn validate_timestamp_only_in_file_name() {
        let dir = std::env::temp_dir();
        let config = f32_config(dir.join("$timestamp$").join("cap.wav").to_str().unwrap());
        assert!(validate_file_writer(&config).is_err());
        let config = f32_config(&temp_file("cap_$timestamp$.wav"));
        assert!(validate_file_writer(&config).is_ok());
    }

    #[test]
    fn validate_rejects_missing_directory() {
        let config = f32_config("/no/such/dir/capture.raw");
        assert!(validate_file_writer(&config).is_err());
    }

    #[test]
    fn validate_rejects_right_justified_wav() {
        let mut config = f32_config(&temp_file("test.wav"));
        config.format = BinarySampleFormat::S24_4_RJ_LE;
        config.wav_header = Some(true);
        assert!(validate_file_writer(&config).is_err());
    }

    #[test]
    fn file_key_folds_the_parent_directory() {
        let dir = std::env::temp_dir();
        let plain = dir.join("capture.raw");
        let dotted = dir.join(".").join("capture.raw");
        assert_eq!(
            file_key(plain.to_str().unwrap()).unwrap(),
            file_key(dotted.to_str().unwrap()).unwrap()
        );
        assert!(file_key("/no/such/dir/capture.raw").is_err());
    }
}
