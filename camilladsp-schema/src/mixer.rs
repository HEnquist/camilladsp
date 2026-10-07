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
use crate::config::{Issues, issue_path};
use crate::filters::basicfilters::check_gain;

/// Validate the mixer config, to give a helpful message intead of a panic.
///
/// Issue paths are relative to the mixer.
pub fn validate_mixer(mixer_config: &config::Mixer) -> Result<(), Issues> {
    let mut issues = Issues::new();
    let chan_in = mixer_config.channels.input();
    let chan_out = mixer_config.channels.output();
    let mut output_channels: Vec<usize> = Vec::with_capacity(chan_out);
    let mut input_channels: Vec<usize> = Vec::with_capacity(chan_in);
    for (idx, mapping) in mixer_config.mapping.iter().enumerate() {
        if mapping.dest >= chan_out {
            let msg = format!(
                "Invalid destination channel {}, max is {}.",
                mapping.dest,
                chan_out - 1
            );
            issues.invalid(issue_path!["mapping", idx, "dest"], msg);
        } else if output_channels.contains(&mapping.dest) {
            let msg = format!(
                "There is more than one mapping for destination channel {}",
                mapping.dest,
            );
            issues.invalid(issue_path!["mapping", idx, "dest"], msg);
        }
        output_channels.push(mapping.dest);
        input_channels.clear();
        for (n, source) in mapping.sources.iter().enumerate() {
            if source.channel >= chan_in {
                let msg = format!(
                    "Invalid source channel {}, max is {}.",
                    source.channel,
                    chan_in - 1
                );
                issues.invalid(issue_path!["mapping", idx, "sources", n, "channel"], msg);
            }
            if input_channels.contains(&source.channel) {
                let msg = format!(
                    "Input channel {} is listed more than once for destination channel {}",
                    source.channel, mapping.dest,
                );
                issues.invalid(issue_path!["mapping", idx, "sources", n, "channel"], msg);
            }
            input_channels.push(source.channel);
            let mut gain_issues = Issues::new();
            check_gain(&mut gain_issues, "gain", source.gain(), source.scale());
            issues.nest(issue_path!["mapping", idx, "sources", n], gain_issues);
        }
    }
    issues.into_result(())
}

/// Get a vector showing which input channels are used
pub fn used_input_channels(mixer_config: &config::Mixer) -> Vec<bool> {
    let chan_in = mixer_config.channels.input();
    let mut used_channels = vec![false; chan_in];
    for mapping in mixer_config.mapping.iter() {
        if !mapping.is_mute() {
            for source in mapping.sources.iter() {
                if !source.is_mute() {
                    used_channels[source.channel] = true;
                }
            }
        }
    }
    used_channels
}

#[cfg(test)]
mod tests {
    use super::validate_mixer;
    use crate::config::{Mixer, format_path};

    fn mixer(sources: &str) -> Mixer {
        let yaml = format!(
            "channels: {{in: 2, out: 2}}\nmapping:\n  - dest: 0\n    sources: {sources}\n  \
             - dest: 1\n    sources: [{{channel: 0}}]\n"
        );
        yaml_serde::from_str(&yaml).unwrap()
    }

    #[test]
    fn source_listed_twice_is_rejected() {
        assert!(validate_mixer(&mixer("[{channel: 0}, {channel: 1}]")).is_ok());
        let issues = validate_mixer(&mixer("[{channel: 1}, {channel: 0}, {channel: 1}]"))
            .expect_err("a source listed twice should be rejected");
        let paths: Vec<String> = issues.iter().map(|i| format_path(&i.path)).collect();
        assert_eq!(paths, vec!["mapping[0].sources[2].channel"]);
        // The same input may feed several destinations.
        assert!(validate_mixer(&mixer("[{channel: 0}]")).is_ok());
    }
}
