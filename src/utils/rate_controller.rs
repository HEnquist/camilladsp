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

/// Deadbeat controller that steers a device clock to keep the buffer at a target level.
///
/// The buffer level integrates the speed mismatch with a gain of exactly `fs` frames per second,
/// so the controller can estimate the speed where the level stays constant from the slope of the
/// level, and then pick the speed that removes the level error over the next `periods` periods.
pub struct DeadbeatRateController {
    fs: f64,
    interval: f64,
    target_level: f64,
    periods: f64,
    alpha: f64,
    max_step: f64,
    applied: f64,
    /// The speed where the level stays constant, `None` until the first period is measured.
    true_speed: Option<f64>,
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
    ) -> Self {
        Self {
            fs: fs as f64,
            interval,
            target_level: target_level as f64,
            periods,
            alpha,
            max_step,
            applied: 1.0,
            true_speed: None,
        }
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
        let true_speed = match self.true_speed {
            Some(previous) => previous + self.alpha * (estimate - previous),
            None => estimate,
        };
        self.true_speed = Some(true_speed);
        let err = level - self.target_level;
        let correction =
            (err / (self.fs * self.interval * self.periods)).clamp(-self.max_step, self.max_step);
        let output = (true_speed - correction).clamp(1.0 - MAX_ADJUST, 1.0 + MAX_ADJUST);
        trace!(
            "Deadbeat controller, error: {err}, slope: {slope}, true speed: {true_speed}, output: {output}"
        );
        self.applied = output;
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

/// Spike: `CAMILLA_DEADBEAT=N,alpha,max_step` sets the deadbeat parameters, and
/// `CAMILLA_DEADBEAT=off` steers a clock with the PI controller instead, for comparison.
fn deadbeat_from_env(
    fs: usize,
    interval: f64,
    target_level: usize,
) -> Option<DeadbeatRateController> {
    let (mut periods, mut alpha, mut max_step) = (2.0, 0.5, 0.001);
    if let Ok(value) = std::env::var("CAMILLA_DEADBEAT") {
        if value.trim() == "off" {
            info!("Rate adjust steers a clock with the PI controller.");
            return None;
        }
        let parts: Vec<Option<f64>> = value.split(',').map(|p| p.trim().parse().ok()).collect();
        match parts[..] {
            [Some(n), Some(a), Some(m)] => (periods, alpha, max_step) = (n, a, m),
            _ => warn!("Ignoring CAMILLA_DEADBEAT={value}, expected N,alpha,max_step or off."),
        }
    }
    info!(
        "Rate adjust steers a clock with the deadbeat controller, N={periods}, alpha={alpha}, max_step={max_step}."
    );
    Some(DeadbeatRateController::new(
        fs,
        interval,
        target_level,
        periods,
        alpha,
        max_step,
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
        let clock = match (&mut self.deadbeat, self.fit.end_level_and_slope()) {
            (Some(deadbeat), Some((end_level, slope))) => {
                debug!("Playback buffer level fit: {end_level:.1} at the end, slope {slope:.2}/s.");
                deadbeat.next(end_level, slope)
            }
            (Some(deadbeat), None) => deadbeat.applied,
            (None, _) => resampler,
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
        let mut controller = DeadbeatRateController::new(48000, interval, 2000, 1.0, 1.0, 0.01);
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
            let mut controller = DeadbeatRateController::new(48000, interval, 2000, 1.0, 0.5, 0.01);
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
