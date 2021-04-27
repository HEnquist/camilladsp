use std::time::{Duration, Instant};
use NewValue;
use PrcFmt;
use ProcessingState;

pub struct Averager {
    sum: f64,
    nbr_values: usize,
}

pub struct Stopwatch {
    start_time: Instant,
    pub value: Duration,
}

pub struct TimeAverage {
    sum: usize,
    timer: Stopwatch,
}

pub struct SilenceCounter {
    silence_threshold: PrcFmt,
    silence_limit_nbr: usize,
    silent_nbr: usize,
}

impl SilenceCounter {
    pub fn new(
        silence_threshold_db: PrcFmt,
        silence_timeout: PrcFmt,
        samplerate: usize,
        chunksize: usize,
    ) -> SilenceCounter {
        let silence_threshold = PrcFmt::new(10.0).powf(silence_threshold_db / 20.0);
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

impl Stopwatch {
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

    pub fn get_stored_millis(&self) -> u64 {
        self.value.as_millis() as u64
    }

    pub fn get_current_duration(&self) -> Duration {
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

impl Averager {
    pub fn new() -> Averager {
        Averager {
            sum: 0.0,
            nbr_values: 0,
        }
    }

    pub fn restart(&mut self) {
        self.sum = 0.0;
        self.nbr_values = 0;
    }

    pub fn add_value(&mut self, value: f64) {
        self.sum += value;
        self.nbr_values += 1;
    }

    pub fn get_average(&self) -> Option<f64> {
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

impl TimeAverage {
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

    pub fn get_average(&self) -> f64 {
        let seconds = self.timer.get_current_duration().as_secs_f64();
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

#[cfg(test)]
mod tests {
    use countertimer::{Averager, SilenceCounter, Stopwatch, TimeAverage};
    use std::time::Instant;
    use ProcessingState;

    fn spinsleep(time: u128) {
        let start = Instant::now();
        while Instant::now().duration_since(start).as_millis() <= time {}
    }

    #[test]
    fn stopwatch_as_timer() {
        let mut t = Stopwatch::new();
        assert_eq!(t.larger_than_millis(8), false);
        spinsleep(5);
        assert_eq!(t.larger_than_millis(8), false);
        spinsleep(5);
        assert_eq!(t.larger_than_millis(8), true);
        t.restart();
        assert_eq!(t.larger_than_millis(8), false);
    }

    #[test]
    fn stopwatch() {
        let mut t = Stopwatch::new();
        assert_eq!(t.get_stored_millis(), 0);
        spinsleep(100);
        assert_eq!(t.get_stored_millis(), 0);
        t.store_and_restart();
        assert!(t.get_stored_millis() > 80);
        assert!(t.get_stored_millis() < 120);
        t.store_and_restart();
        assert_eq!(t.get_stored_millis(), 0);
    }

    #[test]
    fn averager() {
        let mut a = Averager::new();
        assert_eq!(a.get_average(), None);
        a.add_value(1.0);
        a.add_value(2.0);
        a.add_value(6.0);
        assert_eq!(a.get_average(), Some(3.0));
        a.restart();
        assert_eq!(a.get_average(), None);
    }

    #[test]
    fn timeaverage() {
        let mut a = TimeAverage::new();
        spinsleep(10);
        assert_eq!(a.get_average(), 0.0);
        a.add_value(125);
        spinsleep(10);
        a.add_value(125);
        spinsleep(10);
        a.add_value(125);
        spinsleep(10);
        a.add_value(125);
        spinsleep(10);
        assert!(a.get_average() > 7000.0);
        assert!(a.get_average() < 13000.0);
        a.restart();
        spinsleep(10);
        assert_eq!(a.get_average(), 0.0);
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
}
