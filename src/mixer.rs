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

#[derive(Clone)]
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
                let gain = self.mapping[out_chan][source].gain;
                for n in 0..input.frames {
                    waveforms[out_chan][n] += gain * input.waveforms[source_chan][n];
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
        if  mapping.dest >= chan_out {
            let msg = format!(
                "Invalid destination channel {}, max is {}.",
                mapping.dest, chan_out-1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
        for source in mapping.sources.iter() {
            if  source.channel >= chan_in {
                let msg = format!(
                    "Invalid source channel {}, max is {}.",
                    source.channel, chan_in-1
                );
                return Err(config::ConfigError::new(&msg).into());
            }
        }
    }
    Ok(())
}
