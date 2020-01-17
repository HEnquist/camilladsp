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
    pub tempbuf: Vec<PrcFmt>,
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

    pub fn from_config(conf: config::GainParameters) -> Self {
        let gain = conf.gain;
        let inverted = conf.inverted;
        Gain::new(gain, inverted)
    }
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

impl Delay {
    /// Creates a delay filter with delay in samples
    pub fn new(delay: usize, datalength: usize) -> Self {
        let mut buffer = vec![0.0; delay+datalength];
        let mut tempbuf = vec![0.0; datalength];
        Delay {
            delay: delay,
            buffer: buffer,
            tempbuf: tempbuf,
        }
    }


    pub fn from_config(samplerate: usize, datalength: usize, conf: config::DelayParameters) -> Self {
        let delay_samples = (conf.delay/1000.0 * (samplerate as PrcFmt)) as usize;
        Delay::new(delay_samples, datalength)
    }
}

impl Filter for Delay {
    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for n in 0..waveform.len() {
            self.tempbuf[n] = waveform[n];
            waveform[n] = self.buffer[n];
        }
        for n in 0..self.delay {
            self.buffer[n] = self.buffer[n+waveform.len()];
        }
        for n in 0..waveform.len() {
            self.buffer[n+self.delay] = self.tempbuf[n];
        }

        //let out = input.iter().map(|s| self.process_single(*s)).collect::<Vec<PrcFmt>>();
        Ok(())
    }
}
