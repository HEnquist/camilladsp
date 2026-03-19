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

// A simple PI controller for rate adjustments
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
        self.accumulated += rel_err * self.interval;
        let proportional = self.k_p * rel_err;
        let integral = self.k_i * self.accumulated;
        let mut output = proportional + integral;
        trace!("Rate controller, error: {err}, output: {output}, P: {proportional}, I: {integral}");
        output = output.clamp(-0.005, 0.005);
        1.0 - output
    }
}
