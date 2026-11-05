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
use crate::ToCamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::utils::decibels::gain_from_value;
use crate::utils::stash::{container_from_stash, recycle_chunk, vec_from_stash};

/// Mixer validation lives in `camilladsp-schema`.
pub use camilladsp_schema::mixer::{used_input_channels, validate_mixer};

/// A runtime mixer that combines and routes channels with per-source gain.
#[derive(Clone)]
pub struct Mixer {
    /// Name of this mixer as defined in the configuration.
    pub name: String,
    /// Number of input channels.
    pub channels_in: usize,
    /// Number of output channels.
    pub channels_out: usize,
    /// `mapping[out_channel]` is the list of sources summed into that output channel.
    pub mapping: Vec<Vec<MixerSource>>,
}

/// A single source contribution within a [`Mixer`] output channel.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerSource {
    /// Index of the input channel.
    pub channel: usize,
    /// Linear gain applied to this source.
    pub gain: CamillaFloat,
}

impl Mixer {
    /// Creates a Mixer from a config struct
    pub fn from_config(name: String, config: config::Mixer) -> Self {
        let ch_in = config.channels.input();
        let ch_out = config.channels.output();
        let mut mapping = vec![Vec::<MixerSource>::new(); ch_out];
        for cfg_mapping in config.mapping {
            if !cfg_mapping.is_mute() {
                let dest = cfg_mapping.dest;
                for cfg_src in cfg_mapping.sources {
                    if !cfg_src.is_mute() {
                        let gain_value = cfg_src.gain();
                        let inverted = cfg_src.is_inverted();
                        let linear = cfg_src.scale() == config::GainScale::Linear;
                        let gain =
                            gain_from_value(gain_value, linear, inverted, false).to_camilla_float();
                        let src = MixerSource {
                            channel: cfg_src.channel,
                            gain,
                        };
                        mapping[dest].push(src);
                    }
                }
            }
        }
        Mixer {
            name,
            channels_in: ch_in,
            channels_out: ch_out,
            mapping,
        }
    }

    /// Apply a Mixer to an AudioChunk, yielding a new AudioChunk with a possibly different number of channels.
    pub fn process_chunk(&mut self, input: AudioChunk) -> AudioChunk {
        let mut waveforms = container_from_stash(self.channels_out);
        for out_chan in 0..self.channels_out {
            let mut output = vec_from_stash(input.frames);
            for sources in self.mapping[out_chan].chunks(2) {
                if let [a, b] = sources {
                    let input_a = &input.waveforms[a.channel];
                    let input_b = &input.waveforms[b.channel];
                    if input_a.len() == input_b.len() {
                        if input_a.is_empty() {
                            continue;
                        }
                        let gain_a = a.gain;
                        let gain_b = b.gain;
                        for ((out, x), y) in output
                            .iter_mut()
                            .zip(input_a)
                            .zip(input_b)
                            .take(input.frames)
                        {
                            *out = (*out + gain_a * x) + gain_b * y;
                        }
                        continue;
                    }
                }
                for source in sources {
                    let input_waveform = &input.waveforms[source.channel];
                    if input_waveform.is_empty() {
                        continue;
                    }
                    let gain = source.gain;
                    for (out, sample) in output.iter_mut().zip(input_waveform).take(input.frames) {
                        *out += gain * sample;
                    }
                }
            }
            waveforms.push(output);
        }

        let new_chunk = AudioChunk::from(&input, waveforms);
        recycle_chunk(input);
        new_chunk
    }
}

#[cfg(test)]
mod tests {
    use crate::audiochunk::AudioChunk;
    use crate::config::finite;
    use crate::config::{Mixer, MixerChannels, MixerMapping, MixerSource};
    use crate::mixer;
    use crate::mixer::used_input_channels;
    use std::num::NonZeroUsize;

    #[test]
    fn mix_channels_when_a_source_disappears_and_returns() {
        let conf = yaml_serde::from_str(
            "channels: {in: 3, out: 4}
mapping:
  - dest: 0
    sources:
      - {channel: 0, gain: 0.5, scale: linear}
      - {channel: 1, inverted: true}
      - {channel: 2, gain: 0.25, scale: linear}
  - dest: 1
    sources:
      - {channel: 2, gain: 0.5, scale: linear}
      - {channel: 0, gain: 0.5, scale: linear}
  - dest: 2
    sources: [{channel: 1, gain: 0.5, scale: linear}]
",
        )
        .unwrap();
        let mut mix = mixer::Mixer::from_config("mix".to_owned(), conf);
        for (frames, missing) in [(64, false), (65, true), (64, false)] {
            let input = AudioChunk::new(
                vec![
                    vec![0.5; frames],
                    vec![-0.25; frames],
                    if missing {
                        Vec::new()
                    } else {
                        vec![1.0; frames]
                    },
                ],
                1.0,
                -1.0,
                frames,
                frames - 1,
            );
            let timestamp = input.timestamp;
            let output = mix.process_chunk(input);
            assert_eq!((output.frames, output.valid_frames), (frames, frames - 1));
            assert_eq!(output.timestamp, timestamp);
            assert_eq!(
                output.waveforms,
                vec![
                    vec![if missing { 0.5 } else { 0.75 }; frames],
                    vec![if missing { 0.25 } else { 0.75 }; frames],
                    vec![-0.125; frames],
                    vec![0.0; frames],
                ]
            );
            crate::utils::stash::recycle_chunk(output);
        }
    }

    #[test]
    fn check_all_used() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(false),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(false),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let used = used_input_channels(&conf);
        assert_eq!(used, vec![true, true]);
    }

    #[test]
    fn check_not_mapped() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(false),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(false),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let used = used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_mute_source() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(true),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(true),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(false),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(false),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let used = used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_mute_mapping() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(-3.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(true),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(true),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let used = used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_make_mixer() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 0,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(false),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(false),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let mix = mixer::Mixer::from_config("dummy".to_string(), conf);
        assert_eq!(mix.channels_in, 2);
        assert_eq!(mix.channels_out, 4);

        let exp_src0 = mixer::MixerSource {
            channel: 0,
            gain: 1.0,
        };
        let exp_src1 = mixer::MixerSource {
            channel: 1,
            gain: 1.0,
        };
        let exp_src2 = mixer::MixerSource {
            channel: 0,
            gain: 1.0,
        };
        let exp_src3 = mixer::MixerSource {
            channel: 1,
            gain: 1.0,
        };

        let exp_map = vec![
            vec![exp_src0],
            vec![exp_src1],
            vec![exp_src2],
            vec![exp_src3],
        ];

        assert_eq!(mix.mapping, exp_map);
    }

    #[test]
    fn check_make_mixer_muted() {
        let chans = MixerChannels {
            r#in: NonZeroUsize::new(2).unwrap(),
            out: NonZeroUsize::new(4).unwrap(),
        };
        let src0 = MixerSource {
            channel: 0,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: Some(finite!(0.0)),
            inverted: Some(false),
            mute: Some(false),
            scale: None,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: Some(true),
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: Some(false),
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: Some(true),
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: Some(false),
        };
        let conf = Mixer {
            description: None,
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
            labels: None,
        };
        let mix = mixer::Mixer::from_config("dummy".to_string(), conf);
        assert_eq!(mix.channels_in, 2);
        assert_eq!(mix.channels_out, 4);

        //let exp_src0 = mixer::MixerSource {channel: 0, gain: finite!(1.0)};
        let exp_src1 = mixer::MixerSource {
            channel: 1,
            gain: 1.0,
        };
        //let exp_src2 = mixer::MixerSource {channel: 0, gain: 1.0};
        let exp_src3 = mixer::MixerSource {
            channel: 1,
            gain: 1.0,
        };

        let exp_map = vec![vec![], vec![exp_src1], vec![], vec![exp_src3]];

        assert_eq!(mix.mapping, exp_map);
    }
}
