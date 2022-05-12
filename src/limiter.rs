use crate::config;
use crate::filters::Filter;
use crate::PrcFmt;
use crate::Res;

const CUBEFACTOR: PrcFmt = 1.0 / 6.75; // = 1 / (2 * 1.5^3)

#[derive(Clone, Debug)]
pub struct Limiter {
    pub name: String,
    pub soft_clip: bool,
    pub clip_limit: PrcFmt,
}

impl Limiter {
    /// Creates a Compressor from a config struct
    pub fn from_config(
        name: String,
        config: config::LimiterParameters,
    ) -> Self {

        let clip_limit = (10.0 as PrcFmt).powf(config.clip_limit / 20.0);

        debug!("Creating limiter '{}', soft_clip: {}, clip_limit dB: {}, linear: {}", 
                name, config.soft_clip, config.clip_limit, clip_limit);

        Limiter {
            name,
            soft_clip: config.soft_clip,
            clip_limit,
        }
    }

    fn apply_soft_clip(&self, input: &mut [PrcFmt]) {
        for val in input.iter_mut() {
            let mut scaled = *val / self.clip_limit;
            scaled = scaled.clamp(-1.5, 1.5);
            scaled -= CUBEFACTOR * scaled.powi(3);
            *val = scaled * self.clip_limit;
        }
    }

    fn apply_hard_clip(&self, input: &mut [PrcFmt]) {
        for val in input.iter_mut() {
            *val = val.clamp(-self.clip_limit, self.clip_limit);
        }
    }
}

impl Filter for Limiter {
    fn name(&self) -> String {
        self.name.clone()
    }

    /// Apply a Compressor to an AudioChunk, modifying it in-place.
    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        if self.soft_clip {
            self.apply_soft_clip(waveform);
        }
        else {
            self.apply_hard_clip(waveform);
        }
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Filter) {
        if let config::Filter::Limiter { parameters: config } = config {
            let clip_limit = (10.0 as PrcFmt).powf(config.clip_limit / 20.0);

            self.soft_clip = config.soft_clip;
            self.clip_limit = clip_limit;
            debug!("Updated limiter '{}', soft_clip: {}, clip_limit dB: {}, linear: {}",
                self.name, config.soft_clip, config.clip_limit, clip_limit);
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the limiter config, always ok for now.
pub fn validate_config(_config: &config::LimiterParameters) -> Res<()> {
    Ok(())
}
