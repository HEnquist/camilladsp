use audiodevice::AudioChunk;
use config;
use PrcFmt;
use Res;

#[derive(Clone, Debug)]
pub struct Compressor {
    pub name: String,
    pub channels: usize,
    pub monitor_channels: Vec<usize>,
    pub process_channels: Vec<usize>,
    pub attack: PrcFmt,
    pub release: PrcFmt,
    pub threshold: PrcFmt,
    pub factor: PrcFmt,
    pub makeup_gain: PrcFmt,
    pub soft_clip: bool,
    pub clip_limit: PrcFmt,
    pub samplerate: usize,
    pub scratch: Vec<PrcFmt>,
    pub prev_loudness: PrcFmt,
}

impl Compressor {
    /// Creates a Compressor from a config struct
    pub fn from_config(
        name: String,
        config: config::Compressor,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let channels = config.channels;
        let srate = samplerate as PrcFmt;
        let mut monitor_channels = config.monitor_channels.clone();
        if monitor_channels.is_empty() {
            for n in 0..channels {
                monitor_channels.push(n);
            }
        }
        let mut process_channels = config.process_channels.clone();
        if process_channels.is_empty() {
            for n in 0..channels {
                process_channels.push(n);
            }
        }
        let attack = (-1.0 / srate / config.attack).exp();
        let release = (-1.0 / srate / config.release).exp();
        let clip_limit = (10.0 as PrcFmt).powf(config.clip_limit / 20.0);

        let scratch = vec![0.0; chunksize];

        debug!("Creating compressor '{}', channels: {}, monitor_channels: {:?}, process_channels: {:?}, attack: {}, release: {}, threshold: {}, factor: {}, makeup_gain: {}, soft_clip: {}, clip_limit: {}", 
                name, channels, process_channels, monitor_channels, attack, release, config.threshold, config.factor, config.makeup_gain, config.soft_clip, clip_limit);

        Compressor {
            name,
            channels,
            monitor_channels,
            process_channels,
            attack,
            release,
            threshold: config.threshold,
            factor: config.factor,
            makeup_gain: config.makeup_gain,
            soft_clip: config.soft_clip,
            clip_limit,
            samplerate,
            scratch,
            prev_loudness: -100.0,
        }
    }

    pub fn update_parameters(&mut self, config: config::Compressor) {
        let channels = config.channels;
        let srate = self.samplerate as PrcFmt;
        let mut monitor_channels = config.monitor_channels.clone();
        if monitor_channels.is_empty() {
            for n in 0..channels {
                monitor_channels.push(n);
            }
        }
        let mut process_channels = config.process_channels.clone();
        if process_channels.is_empty() {
            for n in 0..channels {
                process_channels.push(n);
            }
        }
        let attack = (-1.0 / srate / config.attack).exp();
        let release = (-1.0 / srate / config.release).exp();
        let clip_limit = (10.0 as PrcFmt).powf(config.clip_limit / 20.0);

        self.monitor_channels = monitor_channels;
        self.process_channels = process_channels;
        self.attack = attack;
        self.release = release;
        self.threshold = config.threshold;
        self.factor = config.factor;
        self.makeup_gain = config.makeup_gain;
        self.soft_clip = config.soft_clip;
        self.clip_limit = clip_limit;
        debug!("Updated compressor '{}', monitor_channels: {:?}, process_channels: {:?}, attack: {}, release: {}, threshold: {}, factor: {}, makeup_gain: {}, soft_clip: {}, clip_limit: {}", 
                self.name, self.process_channels, self.monitor_channels, attack, release, config.threshold, config.factor, config.makeup_gain, config.soft_clip, clip_limit);
    }

    /// Sum all chanels that are included in loudness monitoring, store result in self.scratch
    fn sum_monitor_channels(&mut self, input: &AudioChunk) {
        let ch = self.monitor_channels[0];
        self.scratch.copy_from_slice(&input.waveforms[ch]);
        for ch in self.monitor_channels.iter().skip(1) {
            for (acc, val) in self.scratch.iter_mut().zip(input.waveforms[*ch].iter()) {
                *acc += *val;
            }
        }
    }

    /// Estimate loundness, store result in self.scratch
    fn estimate_loudness(&mut self) {
        for val in self.scratch.iter_mut() {
            // convert to dB
            *val = 20.0 * (val.abs() + 1.0e-9).log10();
            if *val >= self.prev_loudness {
                *val = self.attack * self.prev_loudness + (1.0 - self.attack) * *val;
            } else {
                *val = self.release * self.prev_loudness + (1.0 - self.release) * *val;
            }
            self.prev_loudness = *val;
        }
    }

    /// Calculate linear gain, store result in self.scratch
    fn calculate_linear_gain(&mut self) {
        for val in self.scratch.iter_mut() {
            if *val > self.threshold {
                *val = -(*val - self.threshold) * (self.factor - 1.0) / self.factor;
            } else {
                *val = 0.0;
            }
            *val += self.makeup_gain;
            *val = (10.0 as PrcFmt).powf(*val / 20.0);
        }
    }

    fn apply_gain(&self, input: &mut [PrcFmt]) {
        for (val, gain) in input.iter_mut().zip(self.scratch.iter()) {
            *val *= gain;
        }
    }

    fn apply_soft_clip(&self, input: &mut [PrcFmt]) {
        for val in input.iter_mut() {
            let mut scaled = *val / self.clip_limit;
            if scaled > 1.5 {
                scaled = 1.5;
            } else if scaled < -1.5 {
                scaled = -1.5;
            }
            scaled -= 0.148148148148148148 * scaled.powi(3);
            *val = scaled * self.clip_limit;
        }
    }

    /// Apply a Compressor to an AudioChunk, modifying it in-place.
    pub fn process_chunk(&mut self, input: &mut AudioChunk) {
        self.sum_monitor_channels(input);
        self.estimate_loudness();
        self.calculate_linear_gain();
        for ch in self.process_channels.iter() {
            self.apply_gain(&mut input.waveforms[*ch]);
            if self.soft_clip {
                self.apply_soft_clip(&mut input.waveforms[*ch]);
            }
        }
    }
}

/// Validate the compressor config, to give a helpful message intead of a panic.
pub fn validate_compressor(config: &config::Compressor) -> Res<()> {
    let channels = config.channels;
    if config.attack <= 0.0 {
        let msg = "Attack value must be larger than zero.";
        return Err(config::ConfigError::new(&msg).into());
    }
    if config.release <= 0.0 {
        let msg = "Release value must be larger than zero.";
        return Err(config::ConfigError::new(&msg).into());
    }
    for ch in config.monitor_channels.iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid monitor channel: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    for ch in config.process_channels.iter() {
        if *ch >= channels {
            let msg = format!(
                "Invalid channel to process: {}, max is: {}.",
                *ch,
                channels - 1
            );
            return Err(config::ConfigError::new(&msg).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use compressor;
    use config::Compressor;
}
