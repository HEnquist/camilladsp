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

use parking_lot::RwLock;

use crate::PlaybackStatus;
use crate::utils::countertimer::{Averager, BufferEstimate, Stopwatch};

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

/// The playback side of rate adjust.
///
/// Fed the playback buffer level once per chunk. At the end of every adjust period it publishes
/// the average level as the playback buffer level, and returns the capture speed the
/// [`PIRateController`] asks for. When rate adjust is disabled it does nothing, and no buffer
/// level is published.
///
/// After an interruption, such as a pause or a device that stopped taking data, the first level
/// is left out, and a new adjust period starts there. The levels collected before the
/// interruption are old by then, and the first one after it is measured while the buffer
/// is still filling up.
pub struct RateAdjustReporter {
    controller: PIRateController,
    averager: Averager,
    timer: Stopwatch,
    period_millis: u64,
    enabled: bool,
    interrupted: bool,
}

impl RateAdjustReporter {
    pub fn new(samplerate: usize, adjust_period: f32, target_level: usize, enabled: bool) -> Self {
        RateAdjustReporter {
            controller: PIRateController::new_with_default_gains(
                samplerate,
                adjust_period as f64,
                target_level,
            ),
            averager: Averager::new(),
            timer: Stopwatch::new(),
            period_millis: (1000.0 * adjust_period) as u64,
            enabled: enabled && adjust_period > 0.0,
            interrupted: false,
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
    ) -> Option<f64> {
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
        self.averager.restart();
    }

    /// Add a buffer level, in frames. Returns the new capture speed once per adjust period.
    pub fn update(
        &mut self,
        buffer_level: f64,
        playback_status: &RwLock<PlaybackStatus>,
    ) -> Option<f64> {
        if !self.enabled {
            return None;
        }
        if self.interrupted {
            self.interrupted = false;
            self.restart();
            return None;
        }
        self.averager.add_value(buffer_level);
        if !self.timer.larger_than_millis(self.period_millis) {
            return None;
        }
        let average_level = self.averager.average()?;
        let speed = self.controller.next(average_level);
        self.timer.restart();
        self.averager.restart();
        debug!(
            "Playback buffer level {average_level:.1}, set capture rate to {:.4}%.",
            100.0 * speed
        );
        if let Some(mut status) = playback_status.try_write() {
            status.buffer_level = average_level as usize;
        } else {
            xtrace!("Playback status blocked, skip buffer level update.");
        }
        Some(speed)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ADJUST, PIRateController};

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
        let mut c = controller();
        // Far too full for a long time: held at the limit.
        for _ in 0..1000 {
            c.next(13000.0);
        }
        assert!(at_limit(c.next(13000.0)));
        // Now too empty: the first reading comes off the limit, and the controller
        // asks for the other side once the integral has come down: about 11 periods
        // here, where an integral wound up over the 1000 periods would take 1000.
        let first = c.next(7000.0);
        assert!(!at_limit(first));
        let mut periods = 1;
        let mut speed = first;
        while speed < 1.0 {
            speed = c.next(7000.0);
            periods += 1;
            assert!(
                periods <= 12,
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
