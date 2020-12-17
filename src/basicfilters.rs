use std::sync::{Arc, RwLock};

use crate::filters::Filter;
use config;
use fifoqueue::FifoQueue;

use PrcFmt;
use ProcessingStatus;
use Res;

#[derive(Clone, Debug)]
pub struct Gain {
    pub name: String,
    pub gain: PrcFmt,
}

pub struct Delay {
    pub name: String,
    samplerate: usize,
    pub queue: FifoQueue<PrcFmt>,
}

pub struct Volume {
    pub name: String,
    ramptime_in_chunks: usize,
    current_volume: PrcFmt,
    target_volume: f32,
    ramp_start: PrcFmt,
    ramp_step: usize,
    samplerate: usize,
    chunksize: usize,
    processing_status: Arc<RwLock<ProcessingStatus>>,
}

impl Volume {
    pub fn new(
        name: String,
        ramp_time_ms: f32,
        current_volume: f32,
        chunksize: usize,
        samplerate: usize,
        processing_status: Arc<RwLock<ProcessingStatus>>,
    ) -> Self {
        let ramptime_in_chunks =
            (ramp_time_ms / (1000.0 * chunksize as f32 / samplerate as f32)).round() as usize;
        Volume {
            name,
            ramptime_in_chunks,
            current_volume: current_volume as PrcFmt,
            ramp_start: current_volume as PrcFmt,
            target_volume: current_volume as f32,
            ramp_step: 0,
            samplerate,
            chunksize,
            processing_status,
        }
    }

    pub fn from_config(
        name: String,
        conf: config::VolumeParameters,
        chunksize: usize,
        samplerate: usize,
        processing_status: Arc<RwLock<ProcessingStatus>>,
    ) -> Self {
        let current_volume = processing_status.read().unwrap().volume;
        Volume::new(
            name,
            conf.ramp_time,
            current_volume,
            chunksize,
            samplerate,
            processing_status,
        )
    }

    fn make_ramp(&self) -> Vec<PrcFmt> {
        let ramprange =
            (self.target_volume as PrcFmt - self.ramp_start) / self.ramptime_in_chunks as PrcFmt;
        let stepsize = ramprange / self.chunksize as PrcFmt;
        (0..self.chunksize)
            .map(|val| {
                (10.0 as PrcFmt).powf(
                    (self.ramp_start
                        + ramprange * (self.ramp_step as PrcFmt - 1.0)
                        + val as PrcFmt * stepsize)
                        / 20.0,
                )
            })
            .collect()
    }
}

impl Filter for Volume {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        let shared_vol = self.processing_status.read().unwrap().volume;

        // Volume setting changed
        if (shared_vol - self.target_volume).abs() > 0.001 {
            if self.ramptime_in_chunks > 0 {
                trace!("starting ramp {} -> {}", self.current_volume, shared_vol);
                self.ramp_start = self.current_volume;
                self.target_volume = shared_vol;
                self.ramp_step = 1;
            } else {
                self.current_volume = shared_vol as PrcFmt;
                self.target_volume = shared_vol;
                self.ramp_step = 0;
            }
        }
        // Not in a ramp
        if self.ramp_step == 0 {
            //debug!("constant gain {}", self.current_volume);
            let mut gain: PrcFmt = 10.0;
            gain = gain.powf(self.current_volume as PrcFmt / 20.0);
            for item in waveform.iter_mut() {
                *item *= gain;
            }
        }
        // Ramping
        else if self.ramp_step <= self.ramptime_in_chunks {
            trace!("ramp step {}", self.ramp_step);
            let ramp = self.make_ramp();
            self.ramp_step += 1;
            if self.ramp_step > self.ramptime_in_chunks {
                // Last step of ramp
                self.ramp_step = 0;
            }
            for (item, stepgain) in waveform.iter_mut().zip(ramp.iter()) {
                *item *= *stepgain;
            }
            self.current_volume = 20.0 * ramp.last().unwrap().log10();
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Volume { parameters: conf } = conf {
            self.ramptime_in_chunks = (conf.ramp_time
                / (1000.0 * self.chunksize as f32 / self.samplerate as f32))
                .round() as usize;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

impl Gain {
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

impl Filter for Gain {
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

impl Delay {
    /// Creates a delay filter with delay in samples
    /// Will be improved as it gets slow for long delays
    pub fn new(name: String, samplerate: usize, delay: usize) -> Self {
        let mut queue = FifoQueue::filled_with(delay + 1, 0.0);
        let _elem = queue.pop();
        Delay {
            name,
            samplerate,
            queue,
        }
    }

    pub fn from_config(name: String, samplerate: usize, conf: config::DelayParameters) -> Self {
        let delay_samples = match conf.unit {
            config::TimeUnit::Milliseconds => {
                (conf.delay / 1000.0 * (samplerate as PrcFmt)) as usize
            }
            config::TimeUnit::Samples => conf.delay as usize,
        };
        Delay::new(name, samplerate, delay_samples)
    }
}

impl Filter for Delay {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn process_waveform(&mut self, waveform: &mut Vec<PrcFmt>) -> Res<()> {
        for item in waveform.iter_mut() {
            self.queue.push(*item)?;
            *item = self.queue.pop().unwrap();
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Delay { parameters: conf } = conf {
            let delay_samples = (conf.delay / 1000.0 * (self.samplerate as PrcFmt)) as usize;
            let mut queue = FifoQueue::filled_with(delay_samples + 1, 0.0);
            let _elem = queue.pop();
            self.queue = queue;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

#[cfg(test)]
mod tests {
    use basicfilters::{Delay, Gain};
    use filters::Filter;

    #[test]
    fn gain_invert() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_inv = vec![0.5, 0.0, -0.5];
        let mut gain = Gain::new("test".to_string(), 0.0, true);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_inv);
    }

    #[test]
    fn gain_ampl() {
        let mut waveform = vec![-0.5, 0.0, 0.5];
        let waveform_ampl = vec![-5.0, 0.0, 5.0];
        let mut gain = Gain::new("test".to_string(), 20.0, false);
        gain.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_ampl);
    }

    #[test]
    fn delay_small() {
        let mut waveform = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let waveform_delayed = vec![0.0, 0.0, 0.0, 0.0, -0.5, 1.0, 0.0, 0.0];
        let mut delay = Delay::new("test".to_string(), 44100, 3);
        delay.process_waveform(&mut waveform).unwrap();
        assert_eq!(waveform, waveform_delayed);
    }

    #[test]
    fn delay_large() {
        let mut waveform1 = vec![0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut waveform2 = vec![0.0; 8];
        let waveform_delayed = vec![0.0, 0.0, -0.5, 1.0, 0.0, 0.0, 0.0, 0.0];
        let mut delay = Delay::new("test".to_string(), 44100, 9);
        delay.process_waveform(&mut waveform1).unwrap();
        delay.process_waveform(&mut waveform2).unwrap();
        assert_eq!(waveform1, vec![0.0; 8]);
        assert_eq!(waveform2, waveform_delayed);
    }
}
