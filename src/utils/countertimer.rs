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

use crate::NewValue;
use crate::PrcFmt;
use crate::ProcessingState;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Estimates the current device buffer level by extrapolating from the last known value.
pub struct DeviceBufferEstimator {
    update_time: Instant,
    frames: usize,
    sample_rate: f32,
}

impl DeviceBufferEstimator {
    /// Create a new estimator for a device running at `sample_rate` Hz.
    pub fn new(sample_rate: usize) -> Self {
        DeviceBufferEstimator {
            update_time: Instant::now(),
            frames: 0,
            sample_rate: sample_rate as f32,
        }
    }

    pub fn add(&mut self, frames: usize) {
        self.update_time = Instant::now();
        self.frames = frames;
    }

    pub fn estimate(&self) -> usize {
        let now = Instant::now();
        let time_passed = now.duration_since(self.update_time).as_secs_f32();
        let frames_consumed = (self.sample_rate * time_passed) as usize;
        if frames_consumed >= self.frames {
            return 0;
        }
        self.frames - frames_consumed
    }
}

/// A counter for watching if the signal has been silent
/// for longer than a given limit.
pub struct SilenceCounter {
    silence_threshold: PrcFmt,
    silence_limit_nbr: usize,
    silent_nbr: usize,
}

impl SilenceCounter {
    /// Create a counter that triggers pause after `silence_timeout` seconds below `silence_threshold_db`.
    pub fn new(
        silence_threshold_db: PrcFmt,
        silence_timeout: PrcFmt,
        samplerate: usize,
        chunksize: usize,
    ) -> SilenceCounter {
        let silence_threshold = PrcFmt::coerce(10.0).powf(silence_threshold_db / 20.0);
        let silence_limit_nbr =
            (silence_timeout * samplerate as PrcFmt / chunksize as PrcFmt).round() as usize;
        SilenceCounter {
            silence_threshold,
            silence_limit_nbr,
            silent_nbr: 0,
        }
    }

    pub fn update(&mut self, value_range: PrcFmt) -> ProcessingState {
        let mut state = ProcessingState::Running;
        if self.silence_limit_nbr > 0 {
            if value_range > self.silence_threshold {
                if self.silent_nbr > self.silence_limit_nbr {
                    debug!("Resuming processing");
                }
                self.silent_nbr = 0;
            } else {
                if self.silent_nbr == self.silence_limit_nbr {
                    debug!("Pausing processing");
                }
                if self.silent_nbr >= self.silence_limit_nbr {
                    trace!("Pausing processing");
                    state = ProcessingState::Paused;
                }
                self.silent_nbr += 1;
            }
        }
        state
    }
}

/// A simple stopwatch for measuring time.
pub struct Stopwatch {
    start_time: Instant,
    pub value: Duration,
}

impl Stopwatch {
    /// Create a new stopwatch, started at the current instant.
    pub fn new() -> Stopwatch {
        let start_time = Instant::now();
        let value = Duration::new(0, 0);
        Stopwatch { start_time, value }
    }

    pub fn restart(&mut self) {
        self.start_time = Instant::now();
        self.value = Duration::new(0, 0);
    }

    pub fn store_and_restart(&mut self) {
        let now = Instant::now();
        self.value = now.duration_since(self.start_time);
        self.start_time = now;
    }

    pub fn stored_millis(&self) -> u64 {
        self.value.as_millis() as u64
    }

    pub fn current_duration(&self) -> Duration {
        Instant::now().duration_since(self.start_time)
    }

    pub fn larger_than_millis(&self, millis: u64) -> bool {
        let value = Instant::now().duration_since(self.start_time);
        value.as_millis() as u64 >= millis
    }
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate the average of a series of numbers.
pub struct Averager {
    sum: f64,
    nbr_values: usize,
}

impl Averager {
    /// Create a new, empty averager.
    pub fn new() -> Averager {
        Averager {
            sum: 0.0,
            nbr_values: 0,
        }
    }

    pub fn restart(&mut self) {
        trace!("Restarting averager");
        self.sum = 0.0;
        self.nbr_values = 0;
    }

    pub fn add_value(&mut self, value: f64) {
        self.sum += value;
        self.nbr_values += 1;
        trace!("Averager: added value {}, nb. {}", value, self.nbr_values);
    }

    pub fn average(&self) -> Option<f64> {
        if self.nbr_values > 0 {
            Some(self.sum / (self.nbr_values as f64))
        } else {
            None
        }
    }
}

impl Default for Averager {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate the average number of added counts per second.
pub struct TimeAverage {
    sum: usize,
    timer: Stopwatch,
}

impl TimeAverage {
    /// Create a new time-average counter, started at the current instant.
    pub fn new() -> TimeAverage {
        TimeAverage {
            sum: 0,
            timer: Stopwatch::new(),
        }
    }

    pub fn restart(&mut self) {
        self.sum = 0;
        self.timer.restart();
    }

    pub fn add_value(&mut self, value: usize) {
        self.sum += value;
    }

    pub fn average(&self) -> f64 {
        let seconds = self.timer.current_duration().as_secs_f64();
        if seconds == 0.0 {
            return 0.0;
        }
        self.sum as f64 / seconds
    }

    pub fn larger_than_millis(&self, millis: u64) -> bool {
        self.timer.larger_than_millis(millis)
    }
}

impl Default for TimeAverage {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a value stays within a given range.
pub struct ValueWatcher {
    min_value: f32,
    max_value: f32,
    count_limit: usize,
    count: usize,
}

impl ValueWatcher {
    /// Create a watcher that fires after `count_limit` consecutive values outside `target ± max_rel_diff`.
    pub fn new(target_value: f32, max_rel_diff: f32, count_limit: usize) -> ValueWatcher {
        let min_value = target_value / (1.0 + max_rel_diff);
        let max_value = target_value * (1.0 + max_rel_diff);
        ValueWatcher {
            min_value,
            max_value,
            count_limit,
            count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }

    pub fn check_value(&mut self, value: f32) -> bool {
        if value < self.min_value || value > self.max_value {
            self.count += 1;
        } else {
            self.count = 0;
        }
        self.count > self.count_limit
    }
}

/// A timestamped snapshot of per-channel values.
#[derive(Clone, Debug)]
pub struct HistoryRecord {
    /// Instant at which this record was captured.
    pub time: Instant,
    /// Per-channel signal values for this record.
    pub values: Vec<f32>,
}

/// Rolling history of timestamped per-channel values, with time-windowed average and peak queries.
#[derive(Clone, Debug)]
pub struct ValueHistory {
    buffer: VecDeque<HistoryRecord>,
    peak: Vec<f32>,
    nbr_values: usize,
    history_length: usize,
    valid_records: usize,
}

impl ValueHistory {
    /// Create a history buffer holding `history_length` records, each with `nbr_values` channels.
    pub fn new(history_length: usize, nbr_values: usize) -> Self {
        let mut history = Self {
            buffer: VecDeque::<HistoryRecord>::with_capacity(history_length),
            peak: vec![0.0; nbr_values],
            nbr_values,
            history_length,
            valid_records: 0,
        };
        history.reset_storage(nbr_values);
        history
    }

    fn reset_storage(&mut self, nbr_values: usize) {
        self.nbr_values = nbr_values;
        self.valid_records = 0;
        self.peak.resize(nbr_values, 0.0);
        self.reset_global_max();
        self.buffer = VecDeque::<HistoryRecord>::with_capacity(self.history_length);
        let now = Instant::now();
        for _ in 0..self.history_length {
            self.buffer.push_back(HistoryRecord {
                time: now,
                values: vec![0.0; nbr_values],
            });
        }
    }

    // Add a record
    pub fn add_record(&mut self, values: &[f32]) {
        if self.history_length == 0 {
            return;
        }
        if values.len() != self.nbr_values {
            debug!(
                "Number of values changed. New {}, prev {}. Clearing history.",
                values.len(),
                self.nbr_values
            );
            self.reset_storage(values.len());
        }
        let mut record = self
            .buffer
            .pop_back()
            .expect("ValueHistory storage must be preallocated");
        record.time = Instant::now();
        for ((slot, max), value) in record
            .values
            .iter_mut()
            .zip(self.peak.iter_mut())
            .zip(values.iter())
        {
            *slot = *value;
            if *value > *max {
                *max = *value;
            }
        }
        self.buffer.push_front(record);
        self.valid_records = (self.valid_records + 1).min(self.history_length);
    }

    // Add a record but square the numbers (used for RMS history)
    pub fn add_record_squared(&mut self, values: &[f32]) {
        if self.history_length == 0 {
            return;
        }
        if values.len() != self.nbr_values {
            debug!(
                "Number of values changed. New {}, prev {}. Clearing history.",
                values.len(),
                self.nbr_values
            );
            self.reset_storage(values.len());
        }
        let mut record = self
            .buffer
            .pop_back()
            .expect("ValueHistory storage must be preallocated");
        record.time = Instant::now();
        for ((slot, max), value) in record
            .values
            .iter_mut()
            .zip(self.peak.iter_mut())
            .zip(values.iter())
        {
            let squared = *value * *value;
            *slot = squared;
            if squared > *max {
                *max = squared;
            }
        }
        self.buffer.push_front(record);
        self.valid_records = (self.valid_records + 1).min(self.history_length);
    }

    // Get the average since the given Instance
    pub fn average_since(&self, time: Instant) -> Option<HistoryRecord> {
        let mut scratch = vec![0.0; self.nbr_values];
        let mut nbr_summed = 0;
        for record in self.buffer.iter().take(self.valid_records) {
            if record.time <= time {
                break;
            }
            record
                .values
                .iter()
                .zip(scratch.iter_mut())
                .for_each(|(val, acc)| *acc += *val);
            nbr_summed += 1;
        }
        if nbr_summed == 0 {
            return None;
        }
        let last = self.last().unwrap();
        scratch.iter_mut().for_each(|val| *val /= nbr_summed as f32);
        Some(HistoryRecord {
            values: scratch,
            time: last.time,
        })
    }

    // Get the max since the given Instance
    pub fn max_since(&self, time: Instant) -> Option<HistoryRecord> {
        let mut scratch = vec![0.0; self.nbr_values];
        let mut valid = false;
        for record in self.buffer.iter().take(self.valid_records) {
            if record.time <= time {
                break;
            }
            record
                .values
                .iter()
                .zip(scratch.iter_mut())
                .for_each(|(val, max)| {
                    if *val > *max {
                        *max = *val;
                    }
                });
            valid = true;
        }
        if valid {
            let last = self.last().unwrap();
            return Some(HistoryRecord {
                values: scratch,
                time: last.time,
            });
        }
        None
    }

    // Get the max since the start
    pub fn global_max(&self) -> Vec<f32> {
        self.peak.clone()
    }

    // Reset the global max
    pub fn reset_global_max(&mut self) {
        self.peak.iter_mut().for_each(|val| *val = 0.0);
    }

    // Clear the history and global peak
    pub fn clear_history(&mut self) {
        self.valid_records = 0;
        self.reset_global_max();
    }

    // Get the square root of the average since the given Instance.
    // Used for RMS history.
    // Assumes that every record is the (squared) RMS value for an equally long interval.
    pub fn average_sqrt_since(&self, time: Instant) -> Option<HistoryRecord> {
        let mut result = self.average_since(time);
        if let Some(ref mut record) = result {
            record.values.iter_mut().for_each(|val| *val = val.sqrt());
        };
        result
    }

    pub fn last(&self) -> Option<HistoryRecord> {
        if self.valid_records == 0 {
            None
        } else {
            self.buffer.front().cloned()
        }
    }

    pub fn last_sqrt(&self) -> Option<HistoryRecord> {
        let mut result = self.last();
        if let Some(ref mut record) = result {
            record.values.iter_mut().for_each(|val| *val = val.sqrt())
        };
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::ProcessingState;
    use crate::utils::countertimer::{
        Averager, HistoryRecord, SilenceCounter, Stopwatch, TimeAverage, ValueHistory, ValueWatcher,
    };
    use std::time::{Duration, Instant};

    fn instant_in_past(duration: Duration) -> Instant {
        Instant::now().checked_sub(duration).unwrap()
    }

    fn set_last_record_time(hist: &mut ValueHistory, time: Instant) {
        hist.buffer.front_mut().unwrap().time = time;
    }

    fn add_record_at(hist: &mut ValueHistory, time: Instant, values: Vec<f32>) {
        hist.add_record(&values);
        // Override the timestamp so time-based queries can be tested deterministically.
        set_last_record_time(hist, time);
    }

    fn add_record_squared_at(hist: &mut ValueHistory, time: Instant, values: Vec<f32>) {
        hist.add_record_squared(&values);
        // Override the timestamp so time-based RMS queries do not depend on sleeping.
        set_last_record_time(hist, time);
    }

    fn last_record(hist: &ValueHistory) -> HistoryRecord {
        hist.last().unwrap()
    }

    #[test]
    fn stopwatch_as_timer() {
        let mut t = Stopwatch::new();
        // Set an artificial elapsed time to avoid relying on scheduler timing.
        t.start_time = instant_in_past(Duration::from_millis(10));
        assert!(!t.larger_than_millis(1000));
        // Move the start time further back to simulate a long-running timer deterministically.
        t.start_time = instant_in_past(Duration::from_secs(2));
        assert!(t.larger_than_millis(1000));
        t.restart();
        assert!(!t.larger_than_millis(1000));
    }

    #[test]
    fn stopwatch() {
        let mut t = Stopwatch::new();
        assert_eq!(t.stored_millis(), 0);
        // Inject a stored value directly so this assertion does not depend on real time passing.
        t.value = Duration::from_millis(321);
        assert_eq!(t.stored_millis(), 321);
        // Backdate the start time so store_and_restart observes a known elapsed interval.
        t.start_time = instant_in_past(Duration::from_secs(2));
        t.store_and_restart();
        assert!(t.stored_millis() >= 1000);
        t.restart();
        assert_eq!(t.stored_millis(), 0);
    }

    #[test]
    fn averager() {
        let mut a = Averager::new();
        assert_eq!(a.average(), None);
        a.add_value(1.0);
        a.add_value(2.0);
        a.add_value(6.0);
        assert_eq!(a.average(), Some(3.0));
        a.restart();
        assert_eq!(a.average(), None);
    }

    #[test]
    fn timeaverage() {
        let mut a = TimeAverage::new();
        assert_eq!(a.average(), 0.0);
        a.add_value(1_000_000);
        a.add_value(1_000_000);
        a.add_value(1_000_000);
        a.add_value(1_000_000);
        // Adjust the nested stopwatch directly so the rate calculation uses a deterministic interval.
        a.timer.start_time = instant_in_past(Duration::from_secs(2));
        assert!(a.average() > 1_500_000.0);
        assert!(a.average() < 2_500_000.0);
        a.restart();
        assert_eq!(a.average(), 0.0);
    }

    #[test]
    fn silencecounter() {
        let mut counter = SilenceCounter::new(-40.0, 3.0, 48000, 1024);
        let limit_nbr = (3.0f64 * 48000.0 / 1024.0).round() as usize;
        assert_eq!(counter.silence_limit_nbr, limit_nbr);
        assert_eq!(counter.silence_threshold, 0.01);
        for _ in 0..(2 * limit_nbr) {
            let state = counter.update(0.1);
            assert_eq!(state, ProcessingState::Running);
        }
        for _ in 0..(limit_nbr) {
            let state = counter.update(0.001);
            assert_eq!(state, ProcessingState::Running);
        }
        for _ in 0..(2 * limit_nbr) {
            let state = counter.update(0.001);
            assert_eq!(state, ProcessingState::Paused);
        }
        for _ in 0..(2 * limit_nbr) {
            let state = counter.update(0.1);
            assert_eq!(state, ProcessingState::Running);
        }
    }

    #[test]
    fn silencecounter_largechunksize() {
        let mut counter = SilenceCounter::new(-40.0, 1.0, 48000, 23000);
        let limit_nbr = 2;
        assert_eq!(counter.silence_limit_nbr, limit_nbr);
        assert_eq!(counter.silence_threshold, 0.01);
        for _ in 0..5 {
            let state = counter.update(0.1);
            assert_eq!(state, ProcessingState::Running);
        }
        for _ in 0..2 {
            let state = counter.update(0.001);
            assert_eq!(state, ProcessingState::Running);
        }
        for _ in 0..5 {
            let state = counter.update(0.001);
            assert_eq!(state, ProcessingState::Paused);
        }
        for _ in 0..5 {
            let state = counter.update(0.1);
            assert_eq!(state, ProcessingState::Running);
        }
    }

    #[test]
    fn test_valuewatcher() {
        let limit_nbr = 3;
        let mut watcher = ValueWatcher::new(48000.0, 0.05, limit_nbr);
        for n in 0..10 {
            let val = 48000.0 * (1.0 + 0.004 * n as f32);
            assert!(!watcher.check_value(val));
            let val = 48000.0 * (1.0 - 0.004 * n as f32);
            assert!(!watcher.check_value(val));
        }
        for _ in 0..limit_nbr {
            assert!(!watcher.check_value(44100.0));
        }
        for _ in 0..5 {
            assert!(watcher.check_value(44100.0));
        }
        assert!(!watcher.check_value(48000.0));
        for _ in 0..limit_nbr {
            assert!(!watcher.check_value(88200.0));
        }
        for _ in 0..5 {
            assert!(watcher.check_value(88200.0));
        }
    }

    #[test]
    fn test_valuehistory() {
        let mut hist = ValueHistory::new(6, 2);
        let now = Instant::now();
        let start1 = now.checked_sub(Duration::from_secs(6)).unwrap();
        let start2 = now.checked_sub(Duration::from_secs(2)).unwrap();
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(5)).unwrap(),
            vec![1.0, 2.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(4)).unwrap(),
            vec![2.0, 3.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(3)).unwrap(),
            vec![3.0, 4.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(1500)).unwrap(),
            vec![5.0, 8.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(1)).unwrap(),
            vec![6.0, 9.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(500)).unwrap(),
            vec![7.0, 10.0],
        );
        // This must include all values.
        assert_eq!(
            format!("{:?}", vec![4.0, 6.0]),
            format!("{:?}", hist.average_since(start1).unwrap().values)
        );
        // This must only include the last three.
        assert_eq!(
            format!("{:?}", vec![6.0, 9.0]),
            format!("{:?}", hist.average_since(start2).unwrap().values)
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(400)).unwrap(),
            vec![5.0, 8.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(300)).unwrap(),
            vec![6.0, 9.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(200)).unwrap(),
            vec![7.0, 10.0],
        );
        // This must include the last 6 since the history length is set to 6.
        assert_eq!(
            format!("{:?}", vec![6.0, 9.0]),
            format!("{:?}", hist.average_since(start1).unwrap().values)
        );

        let last = last_record(&hist).time;
        // No new data, should be empty
        assert!(hist.average_since(last).is_none());
    }

    #[test]
    fn test_valuehistory_rms() {
        let mut hist = ValueHistory::new(10, 1);
        let now = Instant::now();
        let start1 = now.checked_sub(Duration::from_secs(2)).unwrap();
        add_record_squared_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(1)).unwrap(),
            vec![7.0],
        );
        add_record_squared_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(500)).unwrap(),
            vec![1.0],
        );
        assert_eq!(
            format!("{:?}", vec![5.0]),
            format!("{:?}", hist.average_sqrt_since(start1).unwrap().values)
        );
    }

    #[test]
    fn test_valuehistory_peak() {
        let mut hist = ValueHistory::new(10, 1);
        let now = Instant::now();
        let start1 = now.checked_sub(Duration::from_secs(2)).unwrap();
        let start2 = now.checked_sub(Duration::from_millis(500)).unwrap();
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(4)).unwrap(),
            vec![8.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(3)).unwrap(),
            vec![9.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(1500)).unwrap(),
            vec![5.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_secs(1)).unwrap(),
            vec![6.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(400)).unwrap(),
            vec![1.0],
        );
        add_record_at(
            &mut hist,
            now.checked_sub(Duration::from_millis(200)).unwrap(),
            vec![2.0],
        );
        // This must include only values added after start1.
        assert_eq!(
            format!("{:?}", vec![6.0]),
            format!("{:?}", hist.max_since(start1).unwrap().values)
        );
        // This must include only values added after start2.
        assert_eq!(
            format!("{:?}", vec![2.0]),
            format!("{:?}", hist.max_since(start2).unwrap().values)
        );
        // This must include all values.
        assert_eq!(
            format!("{:?}", vec![9.0]),
            format!("{:?}", hist.global_max())
        );

        let last = last_record(&hist).time;
        // No new data, should be empty
        assert!(hist.max_since(last).is_none());
    }

    #[test]
    fn test_valuehistory_last() {
        let mut hist = ValueHistory::new(10, 1);
        hist.add_record(&[1.0]);
        hist.add_record(&[2.0]);
        hist.add_record(&[3.0]);
        hist.add_record(&[4.0]);
        assert_eq!(
            format!("{:?}", vec![4.0]),
            format!("{:?}", hist.last().unwrap().values)
        );
        assert_eq!(
            format!("{:?}", vec![2.0]),
            format!("{:?}", hist.last_sqrt().unwrap().values)
        );
    }

    #[test]
    fn test_valuehistory_change_nbr() {
        let mut hist = ValueHistory::new(10, 2);
        hist.add_record(&[1.0, 1.0]);
        hist.add_record(&[2.0, 2.0]);
        assert_eq!(
            format!("{:?}", vec![2.0, 2.0]),
            format!("{:?}", hist.last().unwrap().values)
        );
        hist.add_record(&[3.0]);
        assert_eq!(
            format!("{:?}", vec![3.0]),
            format!("{:?}", hist.last().unwrap().values)
        );
    }
}
