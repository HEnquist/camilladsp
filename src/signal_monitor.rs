use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalLevelSide {
    Playback,
    Capture,
}

#[derive(Debug)]
struct SignalMonitor {
    playback_generation: AtomicU64,
    capture_generation: AtomicU64,
}

static SIGNAL_MONITOR: LazyLock<SignalMonitor> = LazyLock::new(|| SignalMonitor {
    playback_generation: AtomicU64::new(0),
    capture_generation: AtomicU64::new(0),
});

pub fn mark_playback_updated() {
    SIGNAL_MONITOR
        .playback_generation
        .fetch_add(1, Ordering::Release);
}

pub fn mark_capture_updated() {
    SIGNAL_MONITOR
        .capture_generation
        .fetch_add(1, Ordering::Release);
}

pub fn generation(side: SignalLevelSide) -> u64 {
    match side {
        SignalLevelSide::Playback => SIGNAL_MONITOR.playback_generation.load(Ordering::Acquire),
        SignalLevelSide::Capture => SIGNAL_MONITOR.capture_generation.load(Ordering::Acquire),
    }
}
