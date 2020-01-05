type PrcFmt = f64;
use config;
use std::error;
use audiodevice::AudioChunk;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

/// Holder of the biquad coefficients, utilizes normalized form
#[derive(Clone)]
pub struct Mixer {
    pub channels_in: usize,
    pub channels_out: usize,
    pub mapping: Vec<Vec<MixerSource>>
}

#[derive(Clone)]
pub struct MixerSource {
    pub channel: usize,
    pub gain: PrcFmt,
}


// #[derive(Debug, Deserialize)]
// pub struct MixerChannels {
//     r#in: usize,
//     out: usize,
// }
// 
// #[derive(Debug, Deserialize)]
// pub struct MixerSource {
//     channel: usize,
//     gain: PrcFmt,
//     inverted: bool,
// }
// 
// #[derive(Debug, Deserialize)]
// pub struct MixerMapping {
//     dest: usize,
//     sources: Vec<MixerSource>,
// }
// 
// #[derive(Debug, Deserialize)]
// pub struct Mixer {
//     channels: MixerChannels,
//     mapping: Vec<MixerMapping>,
// }

impl Mixer {
    /// Creates a Mixer from a config struct
    pub fn from_config(config: config::Mixer) -> Self {
        let ch_in = config.channels.r#in;
        let ch_out = config.channels.out;
        let mut mapping = vec![Vec::<MixerSource>::new(); ch_out];
        for cfg_mapping in config.mapping {
            let dest = cfg_mapping.dest;
            for cfg_src in cfg_mapping.sources {
                let mut gain: PrcFmt = 10.0;
                gain = gain.powf(cfg_src.gain/20.0);
                if cfg_src.inverted {
                    gain = -gain;
                }
                let src = MixerSource {
                    channel: cfg_src.channel,
                    gain: gain,
                };
                mapping[dest].push(src);
            }
        }
        Mixer {
            channels_in: ch_in,
            channels_out: ch_out,
            mapping: mapping,
        }
    }

    pub fn process_chunk(&mut self, input: &AudioChunk) -> AudioChunk {
        let mut waveforms = Vec::<Vec<PrcFmt>>::with_capacity(self.channels_out);
        for out_chan in 0..self.channels_out {
            waveforms.push(vec![0.0; input.frames]);
            for source in 0..self.mapping[out_chan].len() {
                let source_chan = self.mapping[out_chan][source].channel;
                let gain = self.mapping[out_chan][source].gain;
                for n in 0..input.frames {
                    waveforms[out_chan][n] = waveforms[out_chan][n] + gain*input.waveforms[source_chan][n];
                }
            }
        }

        //for wave in wfs.iter() {
        //let _res_l = filter_l.process_waveform(&mut chunk.waveforms[0]);
        //filtered_wfs.push(filtered_l);
        //let _res_r = filter_r.process_waveform(&mut chunk.waveforms[1]);
        //filtered_wfs.push(filtered_r);
        //
        let chunk = AudioChunk{
            frames: input.frames,
            channels: self.channels_out,
            waveforms: waveforms,
        //    //waveforms: Waveforms::Float64(vec![buf.clone(), buf]),
        };
        chunk
    }
}
