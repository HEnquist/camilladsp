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
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::utils::countertimer::{BufferEstimate, Stopwatch};
use crate::{PlaybackStatus, ProcessingParameters};

/// A playback buffer level, read when a chunk is about to be written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelReading {
    /// Frames in the device buffer and in any buffer between it and the playback thread.
    pub device: f64,
    /// Frames in the chunks queued in front of the playback thread, not counting the chunk
    /// at hand.
    pub queued: f64,
    /// How long before the level was read the capture device captured the last frame of the
    /// chunk at hand, from its [`timestamp`](crate::audiochunk::AudioChunk::timestamp).
    pub latency: Duration,
}

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

/// The deadbeat controller ignores the periods that start within this many seconds of the start.
const IGNORE_AT_START: f64 = 1.0;

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

/// The number of periods the deadbeat controller removes a level error over.
const DEADBEAT_PERIODS: f64 = 3.0;
/// The weight of a new drift estimate in the smoothed one.
const DEADBEAT_ALPHA: f64 = 0.5;
/// The largest correction the deadbeat controller makes on top of the drift.
const DEADBEAT_MAX_STEP: f64 = 0.001;

/// The previous period, as the drift estimate needs it.
/// Times are from the start of the period, and so is the integral of the speed.
#[derive(Debug, Clone, Copy)]
struct PreviousPeriod {
    average: f64,
    /// The mean time of the levels.
    centre: f64,
    /// The mean of the speed integral at the levels.
    mean_integral: f64,
    /// The time of the last level, where the next period starts.
    end: f64,
    /// The speed integral at the last level.
    end_integral: f64,
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
    applied: f64,
    /// The speed sent before `applied`, which the levels show until they catch up.
    previous_applied: f64,
    /// The speed where the level stays constant, `None` until the first period is measured.
    true_speed: Option<f64>,
    previous: Option<PreviousPeriod>,
    /// The time from the start to the start of the current period, in seconds.
    elapsed: f64,
}

impl DeadbeatRateController {
    /// Create a controller with the default settings.
    pub fn new_with_defaults(fs: usize, interval: f64, target_level: usize) -> Self {
        Self::new(
            fs,
            interval,
            target_level,
            DEADBEAT_PERIODS,
            DEADBEAT_ALPHA,
            DEADBEAT_MAX_STEP,
        )
    }

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
            previous_applied: 1.0,
            true_speed: None,
            previous: None,
            elapsed: 0.0,
        }
    }

    /// The levels before an interruption can't be compared with the ones after it.
    /// The drift estimate is kept, since the clocks didn't change.
    fn forget_levels(&mut self) {
        self.previous = None;
    }

    /// Calculate the next speed from the levels of a period.
    /// Returns `None` if there are too few levels, and the speed should stay as it is.
    ///
    /// The periods that start within [`IGNORE_AT_START`] of the start are ignored, and the
    /// clock keeps the speed it started at. Playback is still priming the buffer then, and the
    /// clock may not yet run at the speed it was set to: BlackHole holds the level flat for
    /// about 0.6 s, and then it jumps by tens of frames. Used for the drift, that is off by
    /// hundreds of ppm, and the first steps overshoot.
    fn next_period(&mut self, fit: &LevelFit) -> Option<f64> {
        let period_start = self.elapsed;
        self.elapsed += fit.last_t;
        if period_start < IGNORE_AT_START {
            debug!("Deadbeat controller ignores the period starting {period_start:.2} s in.");
            self.previous_applied = self.applied;
            return Some(self.applied);
        }
        self.next_from_average(fit)
    }

    /// Calculate the next speed from the average level of a period.
    /// The times in `fit` are from the start of the period, which is when the previous speed
    /// was sent, and its speed integral is of the speed in effect at each level,
    /// see [`effective_speed`](Self::effective_speed).
    ///
    /// The level is `L0 + fs·(U(t) - d·t)`, where `U` is the integral of the speed in effect
    /// and `d` the speed that holds the level constant. Averaged over the levels of a period,
    /// that is `L0 + fs·(Ū - d·t̄)`, so the difference between two period averages gives
    /// `d = (ΔŪ - ΔA/fs) / Δt̄`, with everything counted from the start of the previous
    /// period. This holds however the speed changed within the periods, so neither the half
    /// period lag of the averages nor a speed change that shows partway into a period
    /// throws it off.
    ///
    /// The first period after a start has nothing to compare with. It ran at one speed, so
    /// the slope of a least squares line through its levels gives the drift directly.
    /// After an interruption the drift estimate from before is used.
    fn next_from_average(&mut self, fit: &LevelFit) -> Option<f64> {
        let average = fit.average()?;
        let centre = fit.mean_t()?;
        let mean_integral = fit.mean_speed_integral()?;
        let drift = match self.previous {
            Some(previous) if previous.end - previous.centre + centre > 0.0 => {
                let span = previous.end - previous.centre + centre;
                let integral = previous.end_integral - previous.mean_integral + mean_integral;
                let estimate = (integral - (average - previous.average) / self.fs) / span;
                Some(self.smooth(estimate))
            }
            _ => match self.true_speed {
                Some(true_speed) => Some(true_speed),
                None => fit.end_level_and_slope().map(|(_, slope)| {
                    debug!(
                        "Deadbeat controller takes its first drift from a slope of {slope:.2}/s."
                    );
                    self.smooth(self.applied - slope / self.fs)
                }),
            },
        };
        self.previous = Some(PreviousPeriod {
            average,
            centre,
            mean_integral,
            end: fit.last_t,
            end_integral: fit.speed_integral,
        });
        let err = average - self.target_level;
        let drift = drift.unwrap_or(self.applied);
        let output = self.output(drift, err);
        trace!("Deadbeat controller, average error: {err}, true speed: {drift}, output: {output}");
        Some(output)
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
        self.previous_applied = self.applied;
        self.applied = output;
        output
    }

    /// The speed that shows in a level measured `t` seconds after the last speed was sent,
    /// when filtering delays the levels by `lag` seconds. Assumes that the lag is shorter
    /// than a period.
    fn effective_speed(&self, t: f64, lag: f64) -> f64 {
        if t >= lag {
            self.applied
        } else {
            self.previous_applied
        }
    }

    /// Start from the speed the clock already runs at. The drift estimate is the applied speed
    /// minus the measured slope, so a clock left at another speed by an earlier session
    /// would otherwise make the first estimate wrong by that much, and the first step go the
    /// wrong way.
    pub fn seed(&mut self, speed: f64) {
        self.applied = speed;
        self.previous_applied = speed;
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
    /// The integral of the speed in effect, from the start of the period to the last level.
    speed_integral: f64,
    sum_speed_integral: f64,
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

    /// Add a level, with the speed that was in effect since the level before it.
    fn add_with_speed(&mut self, t: f64, level: f64, speed: f64) {
        self.speed_integral += speed * (t - self.last_t);
        self.sum_speed_integral += self.speed_integral;
        self.add(t, level);
    }

    /// The mean of the speed integral at the levels.
    fn mean_speed_integral(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum_speed_integral / self.n as f64)
    }

    fn average(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum_l / self.n as f64)
    }

    /// The mean time of the levels.
    fn mean_t(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum_t / self.n as f64)
    }

    /// The mean time between levels, for levels that come at an even pace.
    fn interval(&self) -> Option<f64> {
        let mean_t = self.mean_t()?;
        (self.n > 1).then(|| 2.0 * (self.last_t - mean_t) / (self.n - 1) as f64)
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

/// The window of [`LevelMedian`].
const LEVEL_MEDIAN: usize = 5;

/// Running median of the last few levels.
///
/// A level that is badly off comes alone or in a pair: a chunk that arrives late reads low by
/// how late it is, and one that follows right behind it can read a chunk low as well, before
/// the device side has counted the chunk pushed just before it. The median of five drops both,
/// and the period average and the drift taken from it never see them. The price is a lag of
/// two levels.
#[derive(Default)]
struct LevelMedian {
    values: [f64; LEVEL_MEDIAN],
    len: usize,
    next: usize,
}

impl LevelMedian {
    fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    /// How many levels the median lags behind, for a level that moves steadily.
    fn lag(&self) -> f64 {
        (LEVEL_MEDIAN - 1) as f64 / 2.0
    }

    /// Add a level, and return the median of the window, or of the levels so far
    /// while it fills up.
    fn add(&mut self, level: f64) -> f64 {
        self.values[self.next] = level;
        self.next = (self.next + 1) % LEVEL_MEDIAN;
        self.len = (self.len + 1).min(LEVEL_MEDIAN);
        let mut sorted = self.values;
        let sorted = &mut sorted[..self.len];
        sorted.sort_unstable_by(f64::total_cmp);
        sorted[self.len / 2]
    }
}

/// How many latencies the usual latency starts out as the median of.
/// Few enough that, at common chunk sizes, the reference is in place well within
/// [`IGNORE_AT_START`]. The levels change when it comes in, and a period with that change in
/// it gives a drift that is hundreds of ppm off.
const LATENCY_START_SAMPLES: usize = 16;
/// The time constant of following slow changes of the usual latency, in seconds.
const LATENCY_TIME_CONSTANT: f64 = 60.0;

/// Refers each buffer level to the moment its chunk was captured.
///
/// A level read when a chunk arrives at the playback side depends on when the chunk arrives.
/// A chunk held up anywhere between the capture device and the playback thread, by a late
/// wakeup or a slow processing run, finds less in the playback buffer, although neither
/// device clock has moved. The *latency* of a chunk is the time from when the capture device
/// captured its last frame to when the level is read. Adding back how much longer than
/// usual the latency was gives the level the playback buffer had when the chunk was captured.
/// That level depends only on the two device clocks, which is what rate adjust is there to
/// match.
///
/// Chunks waiting in front of the playback side are left out of the referred level. They
/// waited because the chunk at hand was late, and its latency already counts that.
///
/// The usual latency starts out as the median of the first few, so that one slow chunk does
/// not skew it, and then follows slow changes. That keeps the average level where it was
/// without the reference, so that the target level means the same, also after for example
/// the processing gets heavier.
struct LatencyReference {
    sample_rate: f64,
    start: [f64; LATENCY_START_SAMPLES],
    count: usize,
    usual: Option<f64>,
    last_update: Option<Instant>,
}

impl LatencyReference {
    fn new(sample_rate: usize) -> Self {
        LatencyReference {
            sample_rate: sample_rate as f64,
            start: [0.0; LATENCY_START_SAMPLES],
            count: 0,
            usual: None,
            last_update: None,
        }
    }

    /// Frames to add to the level read with a chunk of this latency, in place of the chunks
    /// queued in front of the playback side. `None` while the usual latency is not known yet.
    fn correction(&mut self, latency: Duration) -> Option<f64> {
        let latency = latency.as_secs_f64();
        let now = Instant::now();
        match self.usual {
            None => {
                self.start[self.count] = latency;
                self.count += 1;
                if self.count == LATENCY_START_SAMPLES {
                    let mut sorted = self.start;
                    sorted.sort_unstable_by(f64::total_cmp);
                    self.usual = Some(sorted[LATENCY_START_SAMPLES / 2]);
                    debug!(
                        "Rate adjust refers levels to a usual latency of {:.2} ms.",
                        1000.0 * sorted[LATENCY_START_SAMPLES / 2]
                    );
                }
            }
            Some(ref mut usual) => {
                if let Some(last) = self.last_update {
                    let step =
                        (now.duration_since(last).as_secs_f64() / LATENCY_TIME_CONSTANT).min(1.0);
                    *usual += step * (latency - *usual);
                }
            }
        }
        self.last_update = Some(now);
        self.usual.map(|usual| (latency - usual) * self.sample_rate)
    }
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
    deadbeat: DeadbeatRateController,
    /// Where to read the capture clock speed from, until the deadbeat has been seeded.
    capture_clock: Option<Arc<ProcessingParameters>>,
    median: LevelMedian,
    /// How far the median lags the levels, in seconds, from the spacing of the levels in the
    /// last period. Zero until a period has been measured.
    median_lag: f64,
    latency: LatencyReference,
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
            deadbeat: DeadbeatRateController::new_with_defaults(
                samplerate,
                adjust_period as f64,
                target_level,
            ),
            capture_clock: Some(processing_params),
            median: LevelMedian::default(),
            median_lag: 0.0,
            latency: LatencyReference::new(samplerate),
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
        debug!(
            "Rate adjust starts from a clock speed of {:.4}%.",
            100.0 * speed
        );
        self.deadbeat.seed(speed);
    }

    /// The stream was interrupted, start a new adjust period at the next level.
    pub fn interrupt(&mut self) {
        self.interrupted = true;
    }

    /// Add the level from a device buffer estimate, with the `queued_frames` waiting in front
    /// of the device and the `latency` of the chunk at hand, see [`LevelReading`].
    /// An estimate that is `None`, because the device side held the lock, is skipped.
    /// A stale or invalid estimate counts as an interruption.
    /// Returns the new capture speed once per adjust period.
    pub fn update_from_estimate(
        &mut self,
        estimate: Option<BufferEstimate>,
        queued_frames: usize,
        latency: Duration,
        playback_status: &RwLock<PlaybackStatus>,
    ) -> Option<RateAdjust> {
        match estimate {
            Some(BufferEstimate::Level(frames)) => self.update(
                LevelReading {
                    device: frames as f64,
                    queued: queued_frames as f64,
                    latency,
                },
                playback_status,
            ),
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

    /// Add a buffer level. Returns the new capture speeds once per adjust period.
    pub fn update(
        &mut self,
        reading: LevelReading,
        playback_status: &RwLock<PlaybackStatus>,
    ) -> Option<RateAdjust> {
        if !self.enabled {
            return None;
        }
        let buffer_level = match self.latency.correction(reading.latency) {
            Some(frames) => reading.device + frames,
            None => reading.device + reading.queued,
        };
        if self.interrupted {
            self.interrupted = false;
            self.restart();
            self.median.clear();
            self.deadbeat.forget_levels();
            return None;
        }
        let t = self.timer.current_duration().as_secs_f64();
        let level = self.median.add(buffer_level);
        let speed = self.deadbeat.effective_speed(t, self.median_lag);
        self.fit.add_with_speed(t, level, speed);
        if !self.timer.larger_than_millis(self.period_millis) {
            return None;
        }
        let average_level = self.fit.average()?;
        if let Some(params) = self.capture_clock.take() {
            self.seed_clock(params.capture_clock_speed());
        }
        let resampler = self.controller.next(average_level);
        let clock = self
            .deadbeat
            .next_period(&self.fit)
            .unwrap_or(self.deadbeat.applied);
        if let Some(interval) = self.fit.interval() {
            self.median_lag = self.median.lag() * interval;
        }
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
    fn level_median_drops_a_late_chunk_and_the_one_behind_it() {
        let mut median = LevelMedian::default();
        // Rising 1 frame per level, with a late chunk and the one right behind it reading low.
        let levels: Vec<f64> = (0..12)
            .map(|i| {
                2000.0 + i as f64
                    - if i == 6 {
                        700.0
                    } else if i == 7 {
                        1024.0
                    } else {
                        0.0
                    }
            })
            .collect();
        let out: Vec<f64> = levels.iter().map(|&l| median.add(l)).collect();
        // Once the window is full, the output is the ramp two levels late. While the bad pair
        // is in the window it lags up to four levels, but it never dips.
        for (i, value) in out.iter().enumerate().skip(4) {
            let ramp = 2000.0 + i as f64;
            if (6..=10).contains(&i) {
                assert!(*value >= ramp - 4.0 && *value <= ramp - 2.0, "out {out:?}");
            } else {
                assert_eq!(*value, ramp - 2.0, "out {out:?}");
            }
        }
        median.clear();
        assert_eq!(median.add(1500.0), 1500.0);
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
        run_periods_delayed(
            controller,
            true_speed,
            start_error,
            periods,
            0.0,
            0.0,
            offset,
        )
    }

    /// As [`run_periods`], with a new speed showing in the levels `delay` seconds after it is
    /// sent, and the controller told that the levels lag by `lag` seconds.
    fn run_periods_delayed(
        controller: &mut DeadbeatRateController,
        true_speed: f64,
        start_error: f64,
        periods: usize,
        delay: f64,
        lag: f64,
        offset: impl Fn(usize, usize) -> f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let fs = 48000.0;
        let dt = 1024.0 / fs;
        // The reporter ends a period at the first level past the period length.
        let per_period = (controller.interval / dt).ceil() as usize + 1;
        let mut level = controller.target_level + start_error;
        let mut previous_speed = controller.applied;
        let mut speed = controller.applied;
        let mut errors = Vec::new();
        let mut speeds = Vec::new();
        for period in 0..periods {
            let mut fit = LevelFit::default();
            for i in 0..per_period {
                // The first level comes one chunk after the start of the period.
                let t = dt * (i + 1) as f64;
                let effective = if t > delay { speed } else { previous_speed };
                level += (effective - true_speed) * fs * dt;
                fit.add_with_speed(
                    t,
                    level + offset(period, i),
                    controller.effective_speed(t, lag),
                );
            }
            previous_speed = speed;
            speed = controller.next_period(&fit).unwrap();
            errors.push(level - controller.target_level);
            speeds.push(speed);
        }
        (errors, speeds)
    }

    #[test]
    fn average_deadbeat_allows_for_a_lag_in_the_levels() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        // Started 300 frames low, so the speed steps by a few hundred ppm at first.
        // The levels lag by 0.3 s, a lot more than the median adds, to make the effect clear.
        let drift_after_start = |compensate: bool| {
            let mut controller = DeadbeatRateController::new(48000, 1.0, 2000, 3.0, 1.0, 0.01);
            controller.seed(true_speed);
            let lag = if compensate { 0.3 } else { 0.0 };
            run_periods_delayed(&mut controller, true_speed, -300.0, 4, 0.3, lag, |_, _| 0.0);
            controller.true_speed.unwrap()
        };
        // The levels move steadily within each period, so the lag is exact to within a
        // level, and with it the drift comes out exact to a few ppm.
        let compensated = drift_after_start(true);
        let plain = drift_after_start(false);
        assert!(
            (compensated - true_speed).abs() < 5e-6,
            "compensated {compensated}"
        );
        assert!((plain - true_speed).abs() > 20e-6, "plain {plain}");
    }

    #[test]
    fn average_deadbeat_settles_without_overshoot() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let mut controller = DeadbeatRateController::new(48000, 2.0, 2000, 3.0, 1.0, 0.01);
        let (errors, speeds) = run_periods(&mut controller, true_speed, 200.0, 20, |_, _| 0.0);
        // The first period is ignored and keeps the clock speed, so the level runs off.
        // From there the error shrinks every period without changing sign.
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
    fn deadbeat_ignores_the_priming_period() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let leftover = 1.001;
        let mut controller = DeadbeatRateController::new(48000, 1.0, 2000, 3.0, 0.5, 0.001);
        controller.seed(leftover);
        // The first period starts on an empty buffer that fills up over the first few levels,
        // as when playback primes it.
        let priming = |period, i| {
            if period == 0 && i < 8 {
                -2000.0 * (1.0 - i as f64 / 8.0)
            } else {
                0.0
            }
        };
        // Started 370 frames low with a leftover speed 900 ppm off, as seen on BlackHole.
        // With a small start error the first two periods at the leftover speed can still
        // overshoot, since there is no drift estimate before the end of the third period.
        let (errors, speeds) = run_periods(&mut controller, true_speed, -370.0, 20, priming);
        assert_eq!(speeds[0], leftover, "speeds {speeds:?}");
        // From the largest error on, it only shrinks, and never changes sign.
        let peak = errors
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        for pair in errors[peak..].windows(2) {
            assert!(
                pair[1].abs() < pair[0].abs() + 1e-6 && pair[1] * pair[0] > 0.0,
                "errors {errors:?}"
            );
        }
        assert!(errors[19].abs() < 2.0, "errors {errors:?}");
    }

    #[test]
    fn deadbeat_ignores_the_first_second() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let leftover = 1.001;
        let mut controller = DeadbeatRateController::new(48000, 0.5, 2000, 3.0, 0.5, 0.001);
        controller.seed(leftover);
        // With half second periods, the first two start within the first second.
        let (_, speeds) = run_periods(&mut controller, true_speed, -100.0, 3, |_, _| 0.0);
        assert_eq!(speeds[..2], [leftover, leftover], "speeds {speeds:?}");
        assert_ne!(speeds[2], leftover, "speeds {speeds:?}");
    }

    #[test]
    fn latency_reference_adds_back_how_late_a_chunk_is() {
        let mut reference = LatencyReference::new(48000);
        let usual = Duration::from_millis(5);
        // The usual latency is the median of the first ones, so one slow chunk among them
        // does not move it.
        for i in 0..LATENCY_START_SAMPLES - 1 {
            let latency = if i == 10 {
                Duration::from_millis(40)
            } else {
                usual
            };
            assert_eq!(reference.correction(latency), None);
        }
        assert_eq!(reference.correction(usual), Some(0.0));
        // A chunk 1 ms late finds 48 frames less in the buffer, which the correction adds
        // back. The usual latency hardly follows a single one.
        let late = reference.correction(Duration::from_millis(6)).unwrap();
        assert!((late - 48.0).abs() < 0.01, "correction {late}");
        let early = reference.correction(Duration::from_millis(4)).unwrap();
        assert!((early + 48.0).abs() < 0.01, "correction {early}");
    }

    #[test]
    fn average_deadbeat_shrugs_off_one_outlier() {
        let true_speed = 1.0 / (1.0 - 100e-6);
        let mut controller = DeadbeatRateController::new_with_defaults(48000, 2.0, 2000);
        controller.seed(true_speed);
        // Settled at the target, then one level a whole chunk off at the end of period 10.
        let outlier = |period, i| if period == 10 && i == 93 { 1024.0 } else { 0.0 };
        let (_, speeds) = run_periods(&mut controller, true_speed, 0.0, 20, outlier);
        let worst = speeds
            .iter()
            .map(|s| (s - true_speed).abs())
            .fold(0.0, f64::max);
        // 1024 frames in 95 levels moves the average by 10.8 frames. That gives 113 ppm on the
        // drift, half of it after smoothing, and 38 ppm of correction. A least squares slope
        // through the same levels sees about 350 ppm.
        assert!(worst < 100e-6, "worst {worst}");
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
