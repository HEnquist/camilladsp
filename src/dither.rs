use crate::filters::Filter;
use config;
use fifoqueue::FifoQueue;

use PrcFmt;
use Res;

#[derive(Clone, Debug)]
pub struct Dither {
    pub name: String,
    pub bits: usize,
}


impl Dither {
    /// A simple filter providing gain in dB, and can also invert the signal.
    pub fn new(name: String, gain_db: PrcFmt, inverted: bool) -> Self {
        let mut gain: PrcFmt = 10.0;
        gain = gain.powf(gain_db / 20.0);
        if inverted {
            gain = -gain;
        }
        Gain { name, gain }
    }

    pub fn from_config(name: String, conf: config::GainParameters) -> Self {
        let gain = conf.gain;
        let inverted = conf.inverted;
        Gain::new(name, gain, inverted)
    }
}

impl Filter for Dither {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for item in waveform.iter_mut() {
            *item *= self.gain;
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Gain { parameters: conf } = conf {
            let gain_db = conf.gain;
            let inverted = conf.inverted;
            let mut gain: PrcFmt = 10.0;
            gain = gain.powf(gain_db / 20.0);
            if inverted {
                gain = -gain;
            }
            self.gain = gain;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}