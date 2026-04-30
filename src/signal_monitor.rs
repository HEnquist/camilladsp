use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Selects which side of the processing chain (capture or playback) a signal-level query applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalLevelSide {
    /// Playback (output) side.
    Playback,
    /// Capture (input) side.
    Capture,
}

#[derive(Debug)]
struct EventCounter {
    generation: AtomicU64,
    wait_lock: Mutex<()>,
    wait_cv: Condvar,
}

impl EventCounter {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            wait_lock: Mutex::new(()),
            wait_cv: Condvar::new(),
        }
    }

    fn mark_updated(&self) {
        let _guard = self.wait_lock.lock().unwrap();
        self.generation.fetch_add(1, Ordering::Release);
        self.wait_cv.notify_all();
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn wait_for_change(&self, last_seen: u64, timeout: Duration) -> u64 {
        let mut guard = self.wait_lock.lock().unwrap();
        loop {
            let current = self.generation();
            if current != last_seen {
                return current;
            }

            let (next_guard, wait_result) = self.wait_cv.wait_timeout(guard, timeout).unwrap();
            guard = next_guard;
            if wait_result.timed_out() {
                return self.generation();
            }
        }
    }
}

#[derive(Debug)]
struct SignalMonitor {
    playback: EventCounter,
    capture: EventCounter,
    state: EventCounter,
}

static SIGNAL_MONITOR: LazyLock<SignalMonitor> = LazyLock::new(|| SignalMonitor {
    playback: EventCounter::new(),
    capture: EventCounter::new(),
    state: EventCounter::new(),
});

/// Notify waiters that new playback signal levels are available.
pub fn mark_playback_updated() {
    SIGNAL_MONITOR.playback.mark_updated();
}

/// Notify waiters that new capture signal levels are available.
pub fn mark_capture_updated() {
    SIGNAL_MONITOR.capture.mark_updated();
}

/// Notify waiters that the processing state has changed.
pub fn mark_state_updated() {
    SIGNAL_MONITOR.state.mark_updated();
}

/// Return the current event generation counter for the given side.
pub fn generation(side: SignalLevelSide) -> u64 {
    match side {
        SignalLevelSide::Playback => SIGNAL_MONITOR.playback.generation(),
        SignalLevelSide::Capture => SIGNAL_MONITOR.capture.generation(),
    }
}

/// Return the current state-change generation counter.
pub fn state_generation() -> u64 {
    SIGNAL_MONITOR.state.generation()
}

/// Block until the state generation advances past `last_seen` or `timeout` elapses; returns the new generation.
pub fn wait_for_state_change(last_seen: u64, timeout: Duration) -> u64 {
    SIGNAL_MONITOR.state.wait_for_change(last_seen, timeout)
}

#[cfg(test)]
mod tests {
    use super::{mark_state_updated, state_generation, wait_for_state_change};
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn state_waiters_are_woken_by_generation_change() {
        let start_generation = state_generation();
        let barrier = Arc::new(Barrier::new(2));
        let waiter_barrier = barrier.clone();

        let waiter = thread::spawn(move || {
            waiter_barrier.wait();
            wait_for_state_change(start_generation, Duration::from_secs(1))
        });

        barrier.wait();
        mark_state_updated();

        let wake_generation = waiter.join().unwrap();
        assert!(wake_generation > start_generation);
    }
}
