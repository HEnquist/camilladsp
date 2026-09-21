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

use std::thread;
use std::time::{Duration, Instant};

/// Paces a dummy device so it moves audio at roughly the rate a real device would.
///
/// The frame position is derived from the clock rather than by counting sleeps:
///
/// ```text
/// frames_due() = (now - start) * rate
/// backlog()    = frames_moved - frames_due()
/// ```
///
/// Sleep granularity, which is about 15 ms on Windows without `timeBeginPeriod`,
/// then only adds jitter that the virtual buffer absorbs. It cannot make the
/// position itself drift away from real time, which is what counting sleeps would do.
pub struct Pacer {
    start: Instant,
    rate: f64,
    frames_moved: u64,
}

impl Pacer {
    pub fn new(samplerate: usize) -> Self {
        Pacer {
            start: Instant::now(),
            rate: samplerate as f64,
            frames_moved: 0,
        }
    }

    /// The number of frames the clock says should have passed through the device by now.
    fn frames_due(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * self.rate
    }

    /// How many frames the device has moved ahead of the clock, which is the fill
    /// level of its virtual buffer. A negative value means the device is running late.
    pub fn backlog(&self) -> f64 {
        self.frames_moved as f64 - self.frames_due()
    }

    /// Count `frames` as moved through the device.
    pub fn advance(&mut self, frames: usize) {
        self.frames_moved += frames as u64;
    }

    /// Change the rate the device runs at, keeping the current backlog.
    ///
    /// The position is derived from the clock, so the anchor has to move with the rate.
    /// Without that, a drift or rate adjust change would make the virtual buffer jump.
    pub fn set_rate(&mut self, rate: f64) {
        let backlog = self.backlog();
        self.rate = rate;
        let frames_due = self.frames_moved as f64 - backlog;
        self.start = Instant::now() - Duration::from_secs_f64(frames_due / rate);
    }

    /// Sleep until the virtual buffer holds no more than `limit` frames.
    pub fn wait_for_backlog_below(&self, limit: f64) {
        let excess = self.backlog() - limit;
        if excess > 0.0 {
            thread::sleep(Duration::from_secs_f64(excess / self.rate));
        }
    }

    /// Drop any accumulated deficit, so the device carries on from the current time.
    pub fn resync(&mut self) {
        self.start = Instant::now() - Duration::from_secs_f64(self.frames_moved as f64 / self.rate);
    }

    /// Drop the accumulated deficit if the device has fallen more than `limit` frames
    /// behind the clock, returning whether it did.
    ///
    /// A real device cannot buffer an unbounded deficit, it overruns and loses the audio.
    /// Resyncing here does the same thing, and keeps one scheduling hiccup on a busy CI
    /// runner from being followed by a long burst of full speed catch-up.
    pub fn resync_if_behind(&mut self, limit: f64) -> bool {
        if self.backlog() >= -limit {
            return false;
        }
        self.resync();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Pacer;

    #[test]
    fn backlog_follows_moved_frames() {
        let mut pacer = Pacer::new(44100);
        // Nothing moved yet, so the device is already behind by whatever time has passed.
        assert!(pacer.backlog() <= 0.0);
        pacer.advance(44100);
        // One second of frames moved in far less than a second of wall time.
        assert!(pacer.backlog() > 40000.0);
    }

    #[test]
    fn waiting_lets_the_clock_catch_up() {
        let mut pacer = Pacer::new(44100);
        pacer.advance(4410);
        pacer.wait_for_backlog_below(0.0);
        assert!(pacer.backlog() <= 0.0);
    }

    #[test]
    fn changing_the_rate_keeps_the_backlog() {
        let mut pacer = Pacer::new(44100);
        pacer.advance(4410);
        let before = pacer.backlog();
        pacer.set_rate(48000.0);
        assert!((pacer.backlog() - before).abs() < 10.0);
        // The new rate is what the position now moves at.
        pacer.advance(48000);
        assert!((pacer.backlog() - before - 48000.0).abs() < 10.0);
    }

    #[test]
    fn resync_clears_a_large_deficit() {
        let mut pacer = Pacer::new(44100);
        // Nothing is moved, so the device falls behind by 20 ms worth of frames.
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Far enough inside a one second limit that nothing happens.
        assert!(!pacer.resync_if_behind(44100.0));
        // Past a 100 frame limit, so the deficit is dropped.
        assert!(pacer.resync_if_behind(100.0));
        assert!(pacer.backlog().abs() < 100.0);
    }
}
