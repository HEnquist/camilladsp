use std::time::{Duration, Instant};

pub struct Timer {
    start_time: Instant,
    pub time: Duration,
}

pub struct Stopwatch {
    start_time: Instant,
    pub value: Duration,
}

impl Stopwatch {
    pub fn new() -> Stopwatch {
        let start_time = Instant::now();
        let value = Duration::new(0, 0);
        Stopwatch {
            start_time,
            value
        }
    }

    pub fn clear(&mut self) {
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
}

impl Timer {
    pub fn new(time_ms: u64) -> Timer {
        let start_time = Instant::now();
        let time = Duration::from_millis(time_ms);
        Timer {
            start_time,
            time,
        }
    }

    pub fn restart(&mut self) {
        self.start_time = Instant::now();
    }

    pub fn is_finished(&self) -> bool {
        let now = Instant::now();
        now.duration_since(self.start_time) >= self.time
    }
}

#[cfg(test)]
mod tests {
    use timer::{Stopwatch, Timer};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn timer() {
        let mut t = Timer::new(10);
        assert_eq!(t.is_finished(), false);
        thread::sleep(Duration::from_millis(5));
        assert_eq!(t.is_finished(), false);
        thread::sleep(Duration::from_millis(5));
        assert_eq!(t.is_finished(), true);
        t.restart();
        assert_eq!(t.is_finished(), false);
    }

    #[test]
    fn stopwatch() {
        let mut t = Stopwatch::new();
        assert_eq!(t.get_stored_millis(), 0);
        thread::sleep(Duration::from_millis(10));
        assert_eq!(t.get_stored_millis(), 0);
        t.store_and_restart();
        assert!(t.get_stored_millis() > 9);
        assert!(t.get_stored_millis() < 11);
        t.store_and_restart();
        assert_eq!(t.get_stored_millis(), 0);
    }
}

