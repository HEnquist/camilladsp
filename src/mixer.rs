use audiodevice::AudioChunk;
use config;
use PrcFmt;

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
    pub fn from_config(name:String, config: config::Mixer) -> Self {
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

        AudioChunk {
            frames: input.frames,
            channels: self.channels_out,
            maxval: 0.0,
            minval: 0.0,
            waveforms,
        }
    }
}
