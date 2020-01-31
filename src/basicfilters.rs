use crate::filters::Filter;
use config;

use PrcFmt;
use Res;

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
    /// A simple filter providing gain in dB, and can also invert the signal.
    pub fn new(gain_db: PrcFmt, inverted: bool) -> Self {
        let mut gain: PrcFmt = 10.0;
        gain = gain.powf(gain_db/20.0);
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
        Ok(())
    }
}

impl Delay {
    /// Creates a delay filter with delay in samples
    /// Will be improved as it gets slow for long delays
    pub fn new(delay: usize, datalength: usize) -> Self {
        let buffer = vec![0.0; delay+datalength];
        let tempbuf = vec![0.0; datalength];
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
        Ok(())
    }
}
