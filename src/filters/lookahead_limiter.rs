use crate::PrcFmt;
use crate::Res;
use crate::config;
use crate::filters::Filter;
use crate::utils::time::time_to_samples_ceil;

/// A lookahead limiter that does nothing (caveman max).
#[derive(Clone, Debug)]
pub struct LookaheadLimiter {
    pub name: String,
    pub limit: PrcFmt,
    pub attack: PrcFmt,
    pub release: PrcFmt,
    pub samplerate: usize,
}

impl LookaheadLimiter {
    /// Creates a LookaheadLimiter from a config struct
    pub fn from_config(
        name: &str,
        config: config::LookaheadLimiterParameters,
        samplerate: usize,
    ) -> Self {
        let limit = (10.0 as PrcFmt).powf(config.limit / 20.0);
        let unit = config.unit();
        let lookahead_samples = time_to_samples_ceil(config.attack, unit, samplerate);
        let release_samples = time_to_samples_ceil(config.release, unit, samplerate);

        debug!(
            "Creating lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} ms ({} samples), release: {} ms ({} samples)",
            name,
            config.limit,
            limit,
            config.attack,
            lookahead_samples,
            config.release,
            release_samples
        );

        LookaheadLimiter {
            name: name.to_string(),
            limit,
            attack: config.attack,
            release: config.release,
            samplerate,
        }
    }

    /// Process the waveform with lookahead limiting (no‑op).
    fn apply_lookahead_limit(&mut self, _input: &mut [PrcFmt]) {
        // do nothing, input stays as is.
    }
}

impl Filter for LookaheadLimiter {
    fn name(&self) -> &str {
        &self.name
    }

    /// Apply the lookahead limiter to a waveform, modifying it in-place.
    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        self.apply_lookahead_limit(waveform);
        Ok(())
    }

    fn update_parameters(&mut self, config: config::Filter) {
        if let config::Filter::LookaheadLimiter {
            parameters: config, ..
        } = config
        {
            let limit = (10.0 as PrcFmt).powf(config.limit / 20.0);
            let unit = config.unit();
            let lookahead_samples = time_to_samples_ceil(config.attack, unit, self.samplerate);
            let release_samples = time_to_samples_ceil(config.release, unit, self.samplerate);

            self.limit = limit;
            self.attack = config.attack;
            self.release = config.release;

            debug!(
                "Updated lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} units ({} samples), release: {} units ({} samples)",
                self.name,
                config.limit,
                limit,
                config.attack,
                lookahead_samples,
                config.release,
                release_samples
            );
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate the lookahead limiter config
pub fn validate_config(config: &config::LookaheadLimiterParameters) -> Res<()> {
    if config.attack <= 0.0 {
        let msg = "Attack time must be positive.";
        return Err(config::ConfigError::new(msg).into());
    }
    if config.release < 0.0 {
        let msg = "Release time must be non-negative.";
        return Err(config::ConfigError::new(msg).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a test limiter
    fn create_test_limiter(
        limit_db: PrcFmt,
        attack_ms: PrcFmt,
        release_ms: PrcFmt,
        samplerate: usize,
    ) -> LookaheadLimiter {
        let config = config::LookaheadLimiterParameters {
            limit: limit_db,
            attack: attack_ms,
            release: release_ms,
            unit: None,
        };
        LookaheadLimiter::from_config("test", config, samplerate)
    }

    #[test]
    fn test_no_limiting_below_threshold() {
        let mut limiter = create_test_limiter(0.0, 10.0, 100.0, 48000);
        let mut input = vec![0.5, 0.5, 0.5];
        let expected = vec![0.5, 0.5, 0.5];

        limiter.apply_lookahead_limit(&mut input);
        assert_eq!(input, expected);
    }

    #[test]
    fn test_hard_clip_above_threshold() {
        // Input unchanged even above limit
        let mut limiter = create_test_limiter(0.0, 10.0, 100.0, 48000);
        let mut input = vec![2.0, 2.0, 2.0];
        let expected = vec![2.0, 2.0, 2.0];

        limiter.apply_lookahead_limit(&mut input);
        assert_eq!(input, expected);
    }

    #[test]
    fn test_limit_conversion_db_to_linear() {
        // dB to linear conversion still works
        let limiter = create_test_limiter(-6.0, 10.0, 100.0, 48000);
        assert!((limiter.limit - 0.5).abs() < 0.01);

        let limiter = create_test_limiter(6.0, 10.0, 100.0, 48000);
        assert!((limiter.limit - 2.0).abs() < 0.01);
    }
}
