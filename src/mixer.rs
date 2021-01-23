use audiodevice::AudioChunk;
use config;
use PrcFmt;
use Res;

#[derive(Clone)]
pub struct Mixer {
    pub name: String,
    pub channels_in: usize,
    pub channels_out: usize,
    pub mapping: Vec<Vec<MixerSource>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
}

impl Mixer {
    /// Creates a Mixer from a config struct
    pub fn from_config(name: String, config: config::Mixer) -> Self {
        let ch_in = config.channels.r#in;
        let ch_out = config.channels.out;
        let mut mapping = vec![Vec::<MixerSource>::new(); ch_out];
        for cfg_mapping in config.mapping {
            if !cfg_mapping.mute {
                let dest = cfg_mapping.dest;
                for cfg_src in cfg_mapping.sources {
                    if !cfg_src.mute {
                        let mut gain: PrcFmt = 10.0;
                        gain = gain.powf(cfg_src.gain / 20.0);
                        if cfg_src.inverted {
                            gain = -gain;
                        }
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

    pub fn update_parameters(&mut self, config: config::Mixer) {
        let ch_in = config.channels.r#in;
        let ch_out = config.channels.out;
        let mut mapping = vec![Vec::<MixerSource>::new(); ch_out];
        for cfg_mapping in config.mapping {
            let dest = cfg_mapping.dest;
            for cfg_src in cfg_mapping.sources {
                let mut gain: PrcFmt = 10.0;
                gain = gain.powf(cfg_src.gain / 20.0);
                if cfg_src.inverted {
                    gain = -gain;
                }
                let src = MixerSource {
                    channel: cfg_src.channel,
                    gain,
                };
                mapping[dest].push(src);
            }
        }
        self.channels_in = ch_in;
        self.channels_out = ch_out;
        self.mapping = mapping;
    }

    /// Apply a Mixer to an AudioChunk, yielding a new AudioChunk with a possibly different number of channels.
    pub fn process_chunk(&mut self, input: &AudioChunk) -> AudioChunk {
        let mut waveforms = Vec::<Vec<PrcFmt>>::with_capacity(self.channels_out);
        for out_chan in 0..self.channels_out {
            waveforms.push(vec![0.0; input.frames]);
            for source in 0..self.mapping[out_chan].len() {
                let source_chan = self.mapping[out_chan][source].channel;
                if !input.waveforms[source_chan].is_empty() {
                    let gain = self.mapping[out_chan][source].gain;
                    for n in 0..input.frames {
                        waveforms[out_chan][n] += gain * input.waveforms[source_chan][n];
                    }
                }
            }
        }

        AudioChunk::from(input, waveforms)
    }
}

/// Validate the mixer config, to give a helpful message intead of a panic.
pub fn validate_mixer(mixer_config: &config::Mixer) -> Res<()> {
    let chan_in = mixer_config.channels.r#in;
    let chan_out = mixer_config.channels.out;
    for mapping in mixer_config.mapping.iter() {
        if mapping.dest >= chan_out {
            let msg = format!(
                "Invalid destination channel {}, max is {}.",
                mapping.dest,
                chan_out - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
        for source in mapping.sources.iter() {
            if source.channel >= chan_in {
                let msg = format!(
                    "Invalid source channel {}, max is {}.",
                    source.channel,
                    chan_in - 1
                );
                return Err(config::ConfigError::new(&msg).into());
            }
        }
    }
    Ok(())
}

/// Get a vector showing which input channels are used
pub fn get_used_input_channels(mixer_config: &config::Mixer) -> Vec<bool> {
    let chan_in = mixer_config.channels.r#in;
    let mut used_channels = vec![false; chan_in];
    for mapping in mixer_config.mapping.iter() {
        if !mapping.mute {
            for source in mapping.sources.iter() {
                if !source.mute {
                    used_channels[source.channel] = true;
                }
            }
        }
    }
    used_channels
}

#[cfg(test)]
mod tests {
    use config::{Mixer, MixerChannels, MixerMapping, MixerSource};
    use mixer;
    use mixer::get_used_input_channels;

    #[test]
    fn check_all_used() {
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: false,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: false,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
        };
        let used = get_used_input_channels(&conf);
        assert_eq!(used, vec![true, true]);
    }

    #[test]
    fn check_not_mapped() {
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: false,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: false,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
        };
        let used = get_used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_mute_source() {
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: true,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: true,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: false,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: false,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
        };
        let used = get_used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_mute_mapping() {
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: -3.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: true,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: true,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
        };
        let used = get_used_input_channels(&conf);
        assert_eq!(used, vec![false, true]);
    }

    #[test]
    fn check_make_mixer() {
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 0,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: false,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: false,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
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
        let chans = MixerChannels { r#in: 2, out: 4 };
        let src0 = MixerSource {
            channel: 0,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src1 = MixerSource {
            channel: 1,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src2 = MixerSource {
            channel: 0,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let src3 = MixerSource {
            channel: 1,
            gain: 0.0,
            inverted: false,
            mute: false,
        };
        let map0 = MixerMapping {
            dest: 0,
            sources: vec![src0],
            mute: true,
        };
        let map1 = MixerMapping {
            dest: 1,
            sources: vec![src1],
            mute: false,
        };
        let map2 = MixerMapping {
            dest: 2,
            sources: vec![src2],
            mute: true,
        };
        let map3 = MixerMapping {
            dest: 3,
            sources: vec![src3],
            mute: false,
        };
        let conf = Mixer {
            channels: chans,
            mapping: vec![map0, map1, map2, map3],
        };
        let mix = mixer::Mixer::from_config("dummy".to_string(), conf);
        assert_eq!(mix.channels_in, 2);
        assert_eq!(mix.channels_out, 4);

        //let exp_src0 = mixer::MixerSource {channel: 0, gain: 1.0};
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
