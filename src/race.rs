// RACE, recursive ambiophonic crosstalk eliminator

use crate::audiodevice::AudioChunk;
use crate::basicfilters::Delay;
use crate::basicfilters::Gain;
use crate::config;
use crate::config::DelayParameters;
use crate::config::GainParameters;
use crate::filters::Filter;
use crate::filters::Processor;
use crate::PrcFmt;
use crate::Res;

//#[derive(Debug)]
pub struct RACE {
    pub name: String,
    pub channels: usize,
    pub channel_a: usize,
    pub channel_b: usize,
    pub feedback_a: PrcFmt,
    pub feedback_b: PrcFmt,
    pub delay_a: Delay,
    pub delay_b: Delay,
    pub gain: Gain,
    pub samplerate: usize,
}

fn delay_config(config: &config::RACEParameters, samplerate: usize) -> DelayParameters {
    // compensate the delay by subtracting one sample period from the delay, clamp at zero
    let sample_period_in_delay_unit = match config.delay_unit() {
        config::TimeUnit::Microseconds => 1000000.0 / samplerate as PrcFmt,
        config::TimeUnit::Milliseconds => 1000.0 / samplerate as PrcFmt,
        config::TimeUnit::Millimetres => 343.0 * 1000.0 / samplerate as PrcFmt,
        config::TimeUnit::Samples => 1.0,
    };
    let compensated_delay = (config.delay - sample_period_in_delay_unit).max(0.0);

    config::DelayParameters {
        delay: compensated_delay,
        unit: config.delay_unit,
        subsample: config.subsample_delay,
    }
}

fn gain_config(config: &config::RACEParameters) -> GainParameters {
    config::GainParameters {
        gain: -config.attenuation,
        scale: Some(config::GainScale::Decibel),
        inverted: Some(true),
        mute: Some(false),
    }
}

impl RACE {
    /// Creates a RACE processor from a config struct
    pub fn from_config(name: &str, config: config::RACEParameters, samplerate: usize) -> Self {
        let name = name.to_string();
        let channels = config.channels;

        debug!("Creating RACE '{}', channels: {}, channel_a: {}, channel_b: {}, delay: {} {:?}, subsample: {}, attenuation: {}",
                name, channels, config.channel_a, config.channel_b, config.delay, config.delay_unit(), config.subsample_delay(), config.attenuation);
        let delayconf = delay_config(&config, samplerate);
        let delay_a = Delay::from_config("Delay A", samplerate, delayconf.clone());
        let delay_b = Delay::from_config("Delay B", samplerate, delayconf);

        let gainconfig = gain_config(&config);
        let gain = Gain::from_config("Gain", gainconfig);

        // sort channel numbers
        let channel_a = config.channel_a.min(config.channel_b);
        let channel_b = config.channel_a.max(config.channel_b);

        RACE {
            name,
            channels,
            samplerate,
            channel_a,
            channel_b,
            delay_a,
            delay_b,
            gain,
            feedback_a: 0.0,
            feedback_b: 0.0,
        }
    }
}

impl Processor for RACE {
    fn name(&self) -> &str {
        &self.name
    }

    /// Apply a RACE processor to an AudioChunk, modifying it in-place.
    fn process_chunk(&mut self, input: &mut AudioChunk) -> Res<()> {
        let (first, second) = input.waveforms.split_at_mut(self.channel_b);
        let channel_a = &mut first[self.channel_a];
        let channel_b = &mut second[0];
        if channel_a.is_empty() || channel_b.is_empty() {
            return Ok(());
        }
        for (value_a, value_b) in channel_a.iter_mut().zip(channel_b.iter_mut()) {
            // todo math
            let added_a = *value_a + self.feedback_b;
            let added_b = *value_b + self.feedback_a;
            self.feedback_a = self.delay_a.process_single(added_a);
            self.feedback_b = self.delay_b.process_single(added_b);
            self.feedback_a = self.gain.process_single(self.feedback_a);
            self.feedback_b = self.gain.process_single(self.feedback_b);
            *value_a = added_a;
            *value_b = added_b;
        }
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Processor) {
        if let config::Processor::RACE {
            parameters: config, ..
        } = config
        {
            self.channels = config.channels;

            let delayparams = delay_config(&config, self.samplerate);
            let delayconf = config::Filter::Delay {
                parameters: delayparams,
                description: None,
            };
            self.delay_a.update_parameters(delayconf.clone());
            self.delay_b.update_parameters(delayconf);

            let gainparams = gain_config(&config);
            let gainconfig = config::Filter::Gain {
                description: None,
                parameters: gainparams,
            };
            self.gain.update_parameters(gainconfig);

            // sort channel numbers
            self.channel_a = config.channel_a.min(config.channel_b);
            self.channel_b = config.channel_a.max(config.channel_b);

            debug!("Updating RACE '{}', channels: {}, channel_a: {}, channel_b: {}, delay: {} {:?}, subsample: {}, attenuation: {}",
                self.name, self.channels, config.channel_a, config.channel_b, config.delay, config.delay_unit(), config.subsample_delay(), config.attenuation);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the RACE processor config, to give a helpful message intead of a panic.
pub fn validate_race(config: &config::RACEParameters) -> Res<()> {
    let channels = config.channels;
    if config.attenuation <= 0.0 {
        let msg = "Attenuation value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.delay <= 0.0 {
        let msg = "Delay value must be larger than zero.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.channel_a == config.channel_b {
        let msg = "Channels a and b must be different";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.channel_a >= channels {
        let msg = format!(
            "Invalid channel a to process: {}, max is: {}.",
            config.channel_a,
            channels - 1
        );
        return Err(config::ConfigError::new(&msg).into());
    }
    if config.channel_b >= channels {
        let msg = format!(
            "Invalid channel b to process: {}, max is: {}.",
            config.channel_b,
            channels - 1
        );
        return Err(config::ConfigError::new(&msg).into());
    }
    Ok(())
}
