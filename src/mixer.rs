type PrcFmt = f64;
mod config;
use std::error;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

/// Holder of the biquad coefficients, utilizes normalized form
#[derive(Clone, Copy, Debug)]
pub struct Mixer {
    pub channels_in: usize,
    pub channels_out: usize,
    pub mapping: Vec<Vec<MixerSource>>
}

struct MixerSource {
    channel: usize,
    gain: PrcFmt,
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
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn from_config(config: config::Mixer) -> Self {
        let ch_in = config.channels.in;
        let ch_out = config.channels.out;
        Mixer {
            s1: 0.0,
            s2: 0.0,
            coeffs: coefficients,
        }
    }

    fn process_chunk(&mut self, input: AudioChunk) -> AudioChunk {
        let mut waveforms = Vec::PrcFmt::with_capacity(self.channels_out);
        for out_chan in 0..self.channels_out {
            waveforms.push(vec![0.0; input.frames]);
            for source in 0..self.mapping[out_chan].len() {
                source_chan = self.mapping[out_chan][source].channel;
                gain = self.mapping[out_chan][source].gain;
                for n in 0..input.frames {
                    waveforms[out_chan][n] = waveforms[out_chan][n] + gain*input.waveforms[source_chan];
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
        }
    }
}
