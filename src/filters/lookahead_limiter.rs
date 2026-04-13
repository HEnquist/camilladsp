use crate::PrcFmt;
use crate::Res;
use crate::config;
use crate::filters::Filter;
use crate::utils::time::time_to_samples_ceil;

/// A lookahead limiter that applies gain reduction with lookahead attack and exponential release.
#[derive(Clone, Debug)]
pub struct LookaheadLimiter {
    pub name: String,
    /// Linear gain limit (amplitude)
    pub limit: PrcFmt,
    /// Attack time in config units (for display)
    pub attack: PrcFmt,
    /// Release time in config units (for display)
    pub release: PrcFmt,
    pub samplerate: usize,
    /// Lookahead length in samples (N)
    lookahead_samples: usize,
    /// Release coefficient alpha_r = exp(-1/(T_r * fs))
    alpha_r: PrcFmt,
    /// Peak gain state carried across buffers (step 2)
    peak_gain: PrcFmt,
    /// Steps counter for linear attack (step 2)
    steps: usize,
    /// Previous gain state for exponential release (step 3)
    g_prev: PrcFmt,
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

        // Compute release coefficient alpha_r = exp(-1 / (T_r * fs))
        // where T_r = release time in seconds.
        let release_time_seconds =
            time_to_samples(config.release, unit, samplerate) / samplerate as PrcFmt;
        let alpha_r = if release_time_seconds > 0.0 {
            (-1.0 / (release_time_seconds * samplerate as PrcFmt)).exp()
        } else {
            // If release time is zero, instant release (alpha_r = 0)
            0.0
        };

        debug!(
            "Creating lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} ms ({} samples), release: {} ms ({} samples), alpha_r: {}",
            name,
            config.limit,
            limit,
            config.attack,
            lookahead_samples,
            config.release,
            release_samples,
            alpha_r
        );

        LookaheadLimiter {
            name: name.to_string(),
            limit,
            attack: config.attack,
            release: config.release,
            samplerate,
            lookahead_samples,
            alpha_r,
            peak_gain: 1.0,
            steps: lookahead_samples + 1,
            g_prev: 1.0,
        }
    }

    /// Process the waveform with lookahead limiting.
    fn apply_lookahead_limit(&mut self, input: &mut [PrcFmt]) {
        let n = input.len();
        if n == 0 {
            return;
        }

        let limit = self.limit;
        let N = self.lookahead_samples;
        let alpha_r = self.alpha_r;

        // step 1: target gain
        let mut g0 = vec![1.0 as PrcFmt; n];
        for i in 0..n {
            let abs_x = input[i].abs();
            if abs_x > limit {
                g0[i] = limit / abs_x;
            }
        }

        // step 2: backward linear attack (carry peak state across buffers)
        let mut g1 = g0.clone();
        let mut peak_gain = self.peak_gain;
        let mut steps = self.steps;

        for i in (0..n).rev() {
            if g0[i] < 1.0 {
                peak_gain = g0[i];
                steps = 0;
            } else {
                steps += 1;
            }
            if steps <= N {
                g1[i] = g1[i].min(1.0 - (1.0 - peak_gain) * steps as PrcFmt / N as PrcFmt);
            }
        }

        // store state for next buffer
        self.peak_gain = peak_gain;
        self.steps = steps;

        // step 3: forward exponential release (carry g_prev across buffers)
        let mut g2 = g1.clone();
        let mut prev = self.g_prev;

        for i in 0..n {
            if g1[i] > prev {
                g2[i] = g1[i].min(1.0 - (1.0 - prev) * alpha_r);
            }
            prev = g2[i];
        }

        self.g_prev = prev;

        // apply gain
        for i in 0..n {
            input[i] *= g2[i];
        }
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

            // recompute alpha_r
            let release_time_seconds =
                time_to_samples(config.release, unit, self.samplerate) / self.samplerate as PrcFmt;
            let alpha_r = if release_time_seconds > 0.0 {
                (-1.0 / (release_time_seconds * self.samplerate as PrcFmt)).exp()
            } else {
                0.0
            };

            self.limit = limit;
            self.attack = config.attack;
            self.release = config.release;
            self.lookahead_samples = lookahead_samples;
            self.alpha_r = alpha_r;
            // Note: we keep peak_gain, steps, g_prev unchanged across parameter updates.

            debug!(
                "Updated lookahead limiter '{}', limit dB: {}, linear: {}, attack/lookahead: {} units ({} samples), release: {} units ({} samples), alpha_r: {}",
                self.name,
                config.limit,
                limit,
                config.attack,
                lookahead_samples,
                config.release,
                release_samples,
                alpha_r
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
        // Input above limit should be limited to limit (1.0)
        let mut limiter = create_test_limiter(0.0, 10.0, 100.0, 48000);
        let mut input = vec![2.0, 2.0, 2.0];
        // Expect gain reduction to 1.0
        limiter.apply_lookahead_limit(&mut input);
        assert!(input[0].abs() <= 1.0);
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
