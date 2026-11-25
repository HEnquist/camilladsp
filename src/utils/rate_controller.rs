// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::utils::countertimer::{BufferEstimate, Stopwatch};
use crate::{PlaybackStatus, ProcessingParameters};

/// The capture speed asked for by rate adjust, for each kind of actuator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateAdjust {
    /// For an asynchronous resampler, from the [`PIRateController`].
    pub resampler: f64,
    /// For a device clock, from the [`DeadbeatRateController`].
    pub clock: f64,
}

impl RateAdjust {
    /// The same speed for both kinds of actuator.
    pub fn both(speed: f64) -> Self {
        RateAdjust {
            resampler: speed,
            clock: speed,
        }
    }
}

/// The largest adjustment the controller asks for, as a fraction of the rate.
const MAX_ADJUST: f64 = 0.005;

/// PI controller that adjusts the resampling ratio to keep the capture buffer at a target level.
pub struct PIRateController {
    target_level: f64,
    interval: f64,
    k_p: f64,
    k_i: f64,
    frames_per_interval: f64,
    accumulated: f64,
    ramp_steps: usize,
    ramp_trigger_limit: f64,
    ramp_start: f64,
    ramp_step: usize,
}

impl PIRateController {
    /// Create a new controller with default gains
    pub fn new_with_default_gains(fs: usize, interval: f64, target_level: usize) -> Self {
        let k_p = 0.2;
        let k_i = 0.004;
        let ramp_steps = 20;
        let ramp_trigger_limit = 0.33;
        Self::new(
            fs,
            interval,
            target_level,
            k_p,
            k_i,
            ramp_steps,
            ramp_trigger_limit,
        )
    }

    /// Create a controller with explicit proportional (`k_p`) and integral (`k_i`) gains.
    pub fn new(
        fs: usize,
        interval: f64,
        target_level: usize,
        k_p: f64,
        k_i: f64,
        ramp_steps: usize,
        ramp_trigger_limit: f64,
    ) -> Self {
        let frames_per_interval = interval * fs as f64;
        Self {
            target_level: target_level as f64,
            interval,
            k_p,
            k_i,
            frames_per_interval,
            accumulated: 0.0,
            ramp_steps,
            ramp_trigger_limit,
            ramp_start: target_level as f64,
            ramp_step: 0,
        }
    }

    /// Calculate the control output for the next measured value
    pub fn next(&mut self, level: f64) -> f64 {
        if self.ramp_step >= self.ramp_steps
            && ((self.target_level - level) / self.target_level).abs() > self.ramp_trigger_limit
        {
            self.ramp_start = level;
            self.ramp_step = 0;
            debug!(
                "Rate controller, buffer level is {}, starting to adjust back towards target of {}",
                level, self.target_level
            );
        }
        if self.ramp_step == 0 {
            self.ramp_start = level;
        }
        let current_target = if self.ramp_step < self.ramp_steps {
            self.ramp_step += 1;
            let tgt = self.ramp_start
                + (self.target_level - self.ramp_start)
                    * (1.0
                        - ((self.ramp_steps as f64 - self.ramp_step as f64)
                            / self.ramp_steps as f64)
                            .powi(4));
            debug!(
                "Rate controller, ramp step {}/{}, current target {}",
                self.ramp_step, self.ramp_steps, tgt
            );
            tgt
        } else {
            self.target_level
        };
        let err = level - current_target;
        let rel_err = err / self.frames_per_interval;
        let proportional = self.k_p * rel_err;
        let step = rel_err * self.interval;
        // Anti-windup: while the output is held at the limit, only integrate an error
        // that brings it back. Otherwise the integral keeps growing past the clamp, and
        // once the error turns it takes as long again to unwind, all the while asking
        // for the wrong speed. The buffer then runs over or dry, and the levels measured
        // around the recovery can keep pushing the integral further out.
        let unclamped = proportional + self.k_i * (self.accumulated + step);
        if unclamped.abs() <= MAX_ADJUST || unclamped.signum() != step.signum() {
            self.accumulated += step;
        }
        let integral = self.k_i * self.accumulated;
        let mut output = proportional + integral;
        trace!("Rate controller, error: {err}, output: {output}, P: {proportional}, I: {integral}");
        output = output.clamp(-MAX_ADJUST, MAX_ADJUST);
        1.0 - output
    }
}

/// How the deadbeat controller reads the levels of a period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeadbeatEstimator {
    /// The slope and end level of a least squares line.
    Fit,
    /// The period averages. The drift comes from the difference between two consecutive
    /// averages, and the error is the average itself.
    Average,
}

/// The previous period, as the average based estimator needs it.
#[derive(Debug, Clone, Copy)]
struct PreviousPeriod {
    average: f64,
    /// Time from the centre of the period to its last level, when the speed changed.
    centre_to_end: f64,
    /// The speed during the period.
    applied: f64,
}

/// Deadbeat controller that steers a device clock to keep the buffer at a target level.
///
/// The buffer level integrates the speed mismatch with a gain of exactly `fs` frames per second,
/// so the controller can estimate the speed where the level stays constant from how fast the
/// level moves, and then pick the speed that removes the level error over the next `periods`
/// periods.
pub struct DeadbeatRateController {
    fs: f64,
    interval: f64,
    target_level: f64,
    periods: f64,
    alpha: f64,
    max_step: f64,
    estimator: DeadbeatEstimator,
    applied: f64,
    /// The speed where the level stays constant, `None` until the first period is measured.
    true_speed: Option<f64>,
    previous: Option<PreviousPeriod>,
}

impl DeadbeatRateController {
    /// Create a controller that removes the level error over `periods` periods, averages the
    /// speed estimate with weight `alpha`, and limits the correction to `max_step`.
    pub fn new(
        fs: usize,
        interval: f64,
        target_level: usize,
        periods: f64,
        alpha: f64,
        max_step: f64,
        estimator: DeadbeatEstimator,
    ) -> Self {
        Self {
            fs: fs as f64,
            interval,
            target_level: target_level as f64,
            periods,
            alpha,
            max_step,
            estimator,
            applied: 1.0,
            true_speed: None,
            previous: None,
        }
    }

    /// The levels before an interruption can't be compared with the ones after it.
    /// The drift estimate is kept, since the clocks didn't change.
    fn forget_levels(&mut self) {
        self.previous = None;
    }

    /// Calculate the next speed from the levels of a period.
    /// Returns `None` if there are too few levels, and the speed should stay as it is.
    fn next_period(&mut self, fit: &LevelFit) -> Option<f64> {
        match self.estimator {
            DeadbeatEstimator::Fit => {
                let (end_level, slope) = fit.end_level_and_slope()?;
                debug!("Playback buffer level fit: {end_level:.1} at the end, slope {slope:.2}/s.");
                Some(self.next(end_level, slope))
            }
            DeadbeatEstimator::Average => {
                let average = fit.average()?;
                let centre = fit.mean_t()?;
                Some(self.next_from_average(average, centre, fit.last_t))
            }
        }
    }

    /// Calculate the next speed from the average level of a period.
    /// `centre` is the mean time of the levels, and `end` the time of the last one,
    /// both from the start of the period, which is when the previous speed was applied.
    ///
    /// With the level rising at `(u - d)·fs` in each period, the difference between two
    /// averages is `(u_prev - d)·fs·a + (u - d)·fs·b`, where `a` is the time from the previous
    /// centre to the speed change and `b` from there to this centre. Solved for `d`, that gives
    /// the speed that holds the level constant, without the half period lag of the averages.
    /// The first period after a start or an interruption has nothing to compare with, and
    /// uses the drift estimate from before, or the speed the clock runs at.
    pub fn next_from_average(&mut self, average: f64, centre: f64, end: f64) -> f64 {
        let drift = match self.previous {
            Some(previous) if previous.centre_to_end + centre > 0.0 => {
                let span = previous.centre_to_end + centre;
                let applied_mean =
                    (previous.applied * previous.centre_to_end + self.applied * centre) / span;
                let estimate = applied_mean - (average - previous.average) / (self.fs * span);
                Some(self.smooth(estimate))
            }
            _ => self.true_speed,
        };
        self.previous = Some(PreviousPeriod {
            average,
            centre_to_end: end - centre,
            applied: self.applied,
        });
        let drift = drift.unwrap_or(self.applied);
        let err = average - self.target_level;
        let output = self.output(drift, err);
        trace!("Deadbeat controller, average error: {err}, true speed: {drift}, output: {output}");
        output
    }

    /// Average a new estimate of the true speed into the old one.
    fn smooth(&mut self, estimate: f64) -> f64 {
        let true_speed = match self.true_speed {
            Some(previous) => previous + self.alpha * (estimate - previous),
            None => estimate,
        };
        self.true_speed = Some(true_speed);
        true_speed
    }

    /// The speed that removes `err` over `periods` periods, starting from `true_speed`.
    fn output(&mut self, true_speed: f64, err: f64) -> f64 {
        let correction =
            (err / (self.fs * self.interval * self.periods)).clamp(-self.max_step, self.max_step);
        let output = (true_speed - correction).clamp(1.0 - MAX_ADJUST, 1.0 + MAX_ADJUST);
        self.applied = output;
        output
    }

    /// Start from the speed the clock already runs at. The drift estimate is the applied speed
    /// minus the measured slope, so a clock left at another speed by an earlier session
    /// would otherwise make the first estimate wrong by that much, and the first step go the
    /// wrong way.
    pub fn seed(&mut self, speed: f64) {
        self.applied = speed;
    }

    /// Calculate the next speed from the fitted level at the end of the period,
    /// and the fitted slope in frames per second.
    ///
    /// The first estimate is taken as it is, since there is nothing to average it with.
    /// `alpha` only smooths the ones after it.
    pub fn next(&mut self, level: f64, slope: f64) -> f64 {
        let estimate = self.applied - slope / self.fs;
        let true_speed = self.smooth(estimate);
        let err = level - self.target_level;
        let output = self.output(true_speed, err);
        trace!(
            "Deadbeat controller, error: {err}, slope: {slope}, true speed: {true_speed}, output: {output}"
        );
        output
    }
}

/// Least squares fit of a straight line to the buffer levels of one adjust period.
#[derive(Default)]
struct LevelFit {
    n: usize,
    sum_t: f64,
    sum_tt: f64,
    sum_l: f64,
    sum_tl: f64,
    last_t: f64,
}

impl LevelFit {
    fn add(&mut self, t: f64, level: f64) {
        self.n += 1;
        self.sum_t += t;
        self.sum_tt += t * t;
        self.sum_l += level;
        self.sum_tl += t * level;
        self.last_t = t;
    }

    fn average(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum_l / self.n as f64)
    }

    /// The mean time of the levels.
    fn mean_t(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum_t / self.n as f64)
    }

    /// The fitted level at the time of the last value, and the slope per second.
    fn end_level_and_slope(&self) -> Option<(f64, f64)> {
        let n = self.n as f64;
        let denom = n * self.sum_tt - self.sum_t * self.sum_t;
        if self.n < 2 || denom <= 0.0 {
            return None;
        }
        let slope = (n * self.sum_tl - self.sum_t * self.sum_l) / denom;
        let mean_t = self.sum_t / n;
        let mean_l = self.sum_l / n;
        Some((mean_l + slope * (self.last_t - mean_t), slope))
    }
}

/// Spike: `CAMILLA_DEADBEAT=[avg|fit,]N,alpha,max_step` sets the deadbeat estimator and
/// parameters, and `CAMILLA_DEADBEAT=off` steers a clock with the PI controller instead, for
/// comparison. Without a prefix the estimator is `avg`.
fn deadbeat_from_env(
    fs: usize,
    interval: f64,
    target_level: usize,
) -> Option<DeadbeatRateController> {
    let (mut estimator, mut periods, mut alpha, mut max_step) =
        (DeadbeatEstimator::Average, 3.0, 0.5, 0.001);
    if let Ok(value) = std::env::var("CAMILLA_DEADBEAT") {
        if value.trim() == "off" {
            info!("Rate adjust steers a clock with the PI controller.");
            return None;
        }
        let mut fields: Vec<&str> = value.split(',').map(|p| p.trim()).collect();
        let parsed_estimator = match fields.first() {
            Some(&"avg") => Some(DeadbeatEstimator::Average),
            Some(&"fit") => Some(DeadbeatEstimator::Fit),
            _ => None,
        };
        if parsed_estimator.is_some() {
            fields.remove(0);
        }
        let numbers: Vec<Option<f64>> = fields.iter().map(|p| p.parse().ok()).collect();
        match numbers[..] {
            [Some(n), Some(a), Some(m)] => {
                (periods, alpha, max_step) = (n, a, m);
                estimator = parsed_estimator.unwrap_or(estimator);
            }
            _ => warn!(
                "Ignoring CAMILLA_DEADBEAT={value}, expected [avg|fit,]N,alpha,max_step or off."
            ),
        }
    }
    info!(
        "Rate adjust steers a clock with the deadbeat controller, {estimator:?}, N={periods}, alpha={alpha}, max_step={max_step}."
    );
    Some(DeadbeatRateController::new(
        fs,
        interval,
        target_level,
        periods,
        alpha,
        max_step,
        estimator,
    ))
}

/// The playback side of rate adjust.
///
/// Fed the playback buffer level once per chunk. At the end of every adjust period it publishes
/// the average level as the playback buffer level, and returns the capture speeds the
/// [`PIRateController`] and the [`DeadbeatRateController`] ask for. When rate adjust is disabled
/// it does nothing, and no buffer level is published.
///
/// After an interruption, such as a pause or a device that stopped taking data, the first level
/// is left out, and a new adjust period starts there. The levels collected before the
/// interruption are old by then, and the first one after it is measured while the buffer
/// is still filling up.
pub struct RateAdjustReporter {
    controller: PIRateController,
    deadbeat: Option<DeadbeatRateController>,
    /// Where to read the capture clock speed from, until the deadbeat has been seeded.
    capture_clock: Option<Arc<ProcessingParameters>>,
    fit: LevelFit,
    timer: Stopwatch,
    period_millis: u64,
    enabled: bool,
    interrupted: bool,
}

impl RateAdjustReporter {
    /// The capture clock speed is read from `processing_params` at the end of the first
    /// adjust period, when the capture device has long since recorded it.
    pub fn new(
        samplerate: usize,
        adjust_period: f32,
        target_level: usize,
        enabled: bool,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        RateAdjustReporter {
            controller: PIRateController::new_with_default_gains(
                samplerate,
                adjust_period as f64,
                target_level,
            ),
            deadbeat: deadbeat_from_env(samplerate, adjust_period as f64, target_level),
            capture_clock: Some(processing_params),
            fit: LevelFit::default(),
            timer: Stopwatch::new(),
            period_millis: (1000.0 * adjust_period) as u64,
            enabled: enabled && adjust_period > 0.0,
            interrupted: false,
        }
    }

    /// The steered clock is on the playback device, and ran at `speed` when it was opened.
    pub fn playback_clock_speed(&mut self, speed: f64) {
        self.capture_clock = None;
        self.seed_clock(speed);
    }

    fn seed_clock(&mut self, speed: f64) {
        if let Some(deadbeat) = &mut self.deadbeat {
            debug!(
                "Rate adjust starts from a clock speed of {:.4}%.",
                100.0 * speed
            );
            deadbeat.seed(speed);
        }
    }

    /// The stream was interrupted, start a new adjust period at the next level.
    pub fn interrupt(&mut self) {
        self.interrupted = true;
    }

    /// Add the level from a device buffer estimate, plus the `queued_frames` waiting in front
    /// of the device. An estimate that is `None`, because the device side held the lock,
    /// is skipped. A stale or invalid estimate counts as an interruption.
    /// Returns the new capture speed once per adjust period.
    pub fn update_from_estimate(
        &mut self,
        estimate: Option<BufferEstimate>,
        queued_frames: usize,
        playback_status: &RwLock<PlaybackStatus>,
    ) -> Option<RateAdjust> {
        match estimate {
            Some(BufferEstimate::Level(frames)) => {
                self.update((frames + queued_frames) as f64, playback_status)
            }
            Some(BufferEstimate::Stale) => {
                xtrace!("Buffer estimate is stale, skip buffer level sample.");
                self.interrupt();
                None
            }
            Some(BufferEstimate::Invalid) => {
                xtrace!("Buffer estimate is invalid, skip buffer level sample.");
                self.interrupt();
                None
            }
            None => {
                xtrace!("Buffer estimator busy, skip buffer level sample.");
                None
            }
        }
    }

    /// Discard the levels collected so far and start a new adjust period.
    pub fn restart(&mut self) {
        self.timer.restart();
        self.fit = LevelFit::default();
    }

    /// Add a buffer level, in frames. Returns the new capture speeds once per adjust period.
    pub fn update(
        &mut self,
        buffer_level: f64,
        playback_status: &RwLock<PlaybackStatus>,
    ) -> Option<RateAdjust> {
        if !self.enabled {
            return None;
        }
        if self.interrupted {
            self.interrupted = false;
            self.restart();
            if let Some(deadbeat) = &mut self.deadbeat {
                deadbeat.forget_levels();
            }
            return None;
        }
        self.fit
            .add(self.timer.current_duration().as_secs_f64(), buffer_level);
        if !self.timer.larger_than_millis(self.period_millis) {
            return None;
        }
        let average_level = self.fit.average()?;
        if let Some(params) = self.capture_clock.take() {
            self.seed_clock(params.capture_clock_speed());
        }
        let resampler = self.controller.next(average_level);
        let clock = match &mut self.deadbeat {
            Some(deadbeat) => deadbeat.next_period(&self.fit).unwrap_or(deadbeat.applied),
            None => resampler,
        };
        self.restart();
        debug!(
            "Playback buffer level {average_level:.1}, set capture rate to {:.4}% for a resampler, {:.4}% for a clock.",
            100.0 * resampler,
            100.0 * clock
        );
        if let Some(mut status) = playback_status.try_write() {
            status.buffer_level = average_level as usize;
        } else {
            xtrace!("Playback status blocked, skip buffer level update.");
        }
        Some(RateAdjust { resampler, clock })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_gives_end_level_and_slope() {
        let mut fit = LevelFit::default();
        // Rising 20 frames per second, with the chunk quantization of the queue on top.
        for i in 0..100 {
            let t = 0.02 * i as f64;
            let queued = if i % 4 == 0 || i % 4 == 3 {
                1024.0
            } else {
                0.0
            };
            fit.add(t, 1000.0 + 20.0 * t + queued);
        }
        let (end_level, slope) = fit.end_level_and_slope().unwrap();
        assert!((slope - 20.0).abs() < 1e-6, "slope {slope}");
        assert!(
            (end_level - (1512.0 + 20.0 * 1.98)).abs() < 1e-6,
            "end level {end_level}"
        );
        assert!((fit.average().unwrap() - (1512.0 + 20.0 * 0.99)).abs() < 1e-6);
    }

    #[test]
    fn deadbeat_settles_in_two_periods() {
        let fs = 48000.0;
        let interval = 2.0;
        let target = 2000.0;
        // The capture clock runs 100 ppm slow at speed 1.0.
        let true_speed = 1.0 / (1.0 - 100e-6);
        let mut controller = DeadbeatRateController::new(
            48000,
            interval,
            2000,
            1.0,
            1.0,
            0.01,
            DeadbeatEstimator::Fit,
        );
        let mut level = 2200.0;
        let mut speed = 1.0;
        let mut levels = Vec::new();
        for _ in 0..4 {
            let slope = (speed - true_speed) * fs;
            level += slope * interval;
            speed = controller.next(level, slope);
            levels.push(level);
        }
        // The first period only measures the drift, the second removes the error.
        assert!((levels[0] - target).abs() > 100.0);
        assert!((levels[1] - target).abs() < 1e-6, "levels {levels:?}");
        assert!((levels[3] - target).abs() < 1e-6, "levels {levels:?}");
        assert!((speed - true_speed).abs() < 1e-12);
    }

    #[test]
    fn seeded_deadbeat_starts_from_a_leftover_speed() {
        let fs = 48000.0;
        let interval = 2.0;
        let target = 2000.0;
        let true_speed = 1.0 / (1.0 - 100e-6);
        // An earlier session left the clock 0.1 % fast.
        let leftover = 1.001;
        let simulate = |seed: Option<f64>| {
            // With averaging, which must not hold back the first, exact, estimate.
            let mut controller = DeadbeatRateController::new(
                48000,
                interval,
                2000,
                1.0,
                0.5,
                0.01,
                DeadbeatEstimator::Fit,
            );
            if let Some(speed) = seed {
                controller.seed(speed);
            }
            let mut level = 2200.0;
            let mut speed = leftover;
            let mut speeds = Vec::new();
            let mut levels = Vec::new();
            for _ in 0..4 {
                let slope = (speed - true_speed) * fs;
                level += slope * interval;
                speed = controller.next(level, slope);
                speeds.push(speed);
                levels.push(level);
            }
            (speeds, levels)
        };
        // Seeded, it settles in two periods as if it had started there.
        let (speeds, levels) = simulate(Some(leftover));
        assert!((levels[1] - target).abs() < 1e-6, "levels {levels:?}");
        assert!((speeds[3] - true_speed).abs() < 1e-12);
        // Not seeded, it takes the leftover for drift, and the first step goes the
        // wrong way, below the true speed rather than down towards it.
        let (speeds, _) = simulate(None);
        assert!(speeds[0] < true_speed - 500e-6, "speeds {speeds:?}");
    }

    /// Feeds a controller levels once per chunk, like the reporter does, with a capture clock
    /// that holds the level constant at `true_speed`. `offset(period, i)` is added to the
    /// measured level. Returns the true level error and the speed at the end of each period.
    fn run_periods(
        controller: &mut DeadbeatRateController,
        true_speed: f64,
        start_error: f64,
        periods: usize,
        offset: impl Fn(usize, usize) -> f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let fs = 48000.0;
        let dt = 1024.0 / fs;
        // The reporter ends a period at the first level past the period length.
        let per_period = (controller.interval / dt).ceil() as usize + 1;
        let mut level = controller.target_level + start_error;
        let mut speed = controller.applied;
        let mut errors = Vec::new();
        let mut speeds = Vec::new();
        for period in 0..periods {
            let mut fit = LevelFit::default();
            for i in 0..per_period {
                let t = dt * i as f64;
                // The first level comes one chunk after the start of the period.
                level += (speed - true_speed) * fs * dt;
                fit.add(t + dt, level + offset(period, i));
            }
            speed = controller.next_period(&fit).unwrap();
            errors.push(level - controller.target_level);
            speeds.push(speed);
        }
        (errors, speeds)
    }

    #[test]
    fn average_deadbeat_settles_without_overshoot() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let mut controller = DeadbeatRateController::new(
            48000,
            2.0,
            2000,
            3.0,
            1.0,
            0.01,
            DeadbeatEstimator::Average,
        );
        let (errors, speeds) = run_periods(&mut controller, true_speed, 200.0, 20, |_, _| 0.0);
        // The first period has no drift estimate and takes the clock speed for it, so the level
        // runs off. From there the error shrinks every period without changing sign.
        let peak = errors
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        for pair in errors[peak..].windows(2) {
            assert!(
                pair[1].abs() < pair[0].abs() && pair[1] * pair[0] > 0.0,
                "errors {errors:?}"
            );
        }
        assert!(errors[19].abs() < 1.0, "errors {errors:?}");
        assert!((speeds[19] - true_speed).abs() < 1e-7, "speeds {speeds:?}");
    }

    #[test]
    fn average_deadbeat_shrugs_off_one_outlier() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let new = |estimator| {
            let mut controller =
                DeadbeatRateController::new(48000, 2.0, 2000, 3.0, 0.5, 0.001, estimator);
            controller.seed(true_speed);
            controller
        };
        // Settled at the target, then one level a whole chunk off at the end of period 10.
        let outlier = |period, i| if period == 10 && i == 93 { 1024.0 } else { 0.0 };
        let worst = |speeds: Vec<f64>| {
            speeds
                .iter()
                .map(|s| (s - true_speed).abs())
                .fold(0.0, f64::max)
        };
        let (_, speeds) = run_periods(
            &mut new(DeadbeatEstimator::Average),
            true_speed,
            0.0,
            20,
            outlier,
        );
        let average = worst(speeds);
        let (_, speeds) = run_periods(
            &mut new(DeadbeatEstimator::Fit),
            true_speed,
            0.0,
            20,
            outlier,
        );
        let fit = worst(speeds);
        // 1024 frames in 95 levels moves the average by 10.8 frames. That gives 113 ppm on the
        // drift, half of it after smoothing, and 38 ppm of correction. The fit sees ~350 ppm.
        assert!(average < 100e-6, "average {average}");
        assert!(fit > 3.0 * average, "fit {fit}, average {average}");
    }

    // Default gains, no ramp, so the error is just level - target.
    fn controller() -> PIRateController {
        PIRateController::new(48000, 10.0, 10000, 0.2, 0.004, 0, 0.33)
    }

    fn at_limit(speed: f64) -> bool {
        ((1.0 - speed).abs() - MAX_ADJUST).abs() < 1e-12
    }

    #[test]
    fn small_errors_are_plain_pi() {
        let mut c = controller();
        let mut accumulated = 0.0;
        for level in [10100.0, 10200.0, 9900.0, 10050.0, 9800.0] {
            let rel_err: f64 = (level - 10000.0) / 480000.0;
            accumulated += rel_err * 10.0;
            let expected = 1.0 - (0.2 * rel_err + 0.004 * accumulated);
            assert!((c.next(level) - expected).abs() < 1e-15);
        }
    }

    #[test]
    fn leaves_the_limit_as_soon_as_the_error_turns() {
        // The defaults: chunksize 1024 as the target, a 10 s adjust period. An error
        // can not be larger than the target on the empty side.
        let mut c = PIRateController::new(48000, 10.0, 1024, 0.2, 0.004, 0, 0.33);
        // Too full for a long time: held at the limit.
        for _ in 0..1000 {
            c.next(1624.0);
        }
        assert!(at_limit(c.next(1624.0)));
        // Now empty: the first reading comes off the limit, and the controller asks
        // for the other side once the integral has come down, about 51 periods at
        // these gains. An integral wound up over the 1000 periods would take about
        // 580, and one wound up for a day several thousand.
        let first = c.next(0.0);
        assert!(!at_limit(first));
        let mut periods = 1;
        let mut speed = first;
        while speed < 1.0 {
            speed = c.next(0.0);
            periods += 1;
            assert!(
                periods <= 60,
                "still asking to slow down after {periods} periods"
            );
        }
    }

    #[test]
    fn the_integral_unwinds_from_saturation() {
        let mut c = controller();
        for _ in 0..1000 {
            c.next(13000.0);
        }
        // Integrating into saturation is stopped, so what was built up is no more
        // than the limit's worth.
        assert!((0.004 * c.accumulated).abs() <= MAX_ADJUST + 1e-12);
    }
}
