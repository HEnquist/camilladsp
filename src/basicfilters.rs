// Based on https://github.com/korken89/biquad-rs
// coeffs: https://arachnoid.com/BiQuadDesigner/index.html

//mod filters;

use crate::filters::Filter;
use config;

// Sample format
//type SmpFmt = i16;
type PrcFmt = f64;

use std::error;
pub type Res<T> = Result<T, Box<dyn error::Error>>;

#[derive(Copy, Clone, Debug)]
pub struct Gain {
    pub gain: PrcFmt,
}

pub struct Delay {
    pub delay: usize,
    pub buffer: Vec<PrcFmt>,
}


impl Gain {
    /// Creates a Direct Form 2 Transposed biquad from a set of filter coefficients
    pub fn new(gain_dB: PrcFmt, inverted: bool) -> Self {
        let mut gain: PrcFmt = 10.0;
        gain = gain.powf(gain_dB/20.0);
        if inverted {
            gain = -gain;
        }
        Gain {
            gain: gain,
        }
    }

    pub fn from_config()
}

impl Filter for Gain {
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for n in 0..waveform.len() {
            waveform[n] = self.gain*waveform[n];
        }
        //let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<PrcFmt>>();
        Ok(())
    }
}
