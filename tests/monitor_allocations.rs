//! Per-thread allocation checks on warmed processor calls, not startup/reloads.
use camilladsp::audiochunk::AudioChunk;
use camilladsp::config::{self, MonitorMode};
use camilladsp::processors::Processor;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

fn record() {
    let _ = TRACK.try_with(|tracking| {
        if tracking.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}

// SAFETY: all pointer ownership, layouts and allocation operations are delegated
// unchanged to System. The counters are thread-local, const-initialized Cells.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: caller guarantees GlobalAlloc's layout contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: same forwarding contract as alloc.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        // SAFETY: caller provides the original pointer/layout and new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: caller provides the original pointer and allocation layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn warmed_processor_calls_do_not_allocate() {
    for mode in [MonitorMode::Sum, MonitorMode::Max, MonitorMode::Rms] {
        for kind in ["Compressor", "NoiseGate", "LookaheadLimiter"] {
            let extra = match kind {
                "Compressor" => "threshold: -20\nfactor: 2\n",
                "NoiseGate" => "threshold: -20\nattenuation: 40\n",
                _ => "limit: 0\n",
            };
            let yaml = format!(
                "channels: 4\nmonitor_mode: {mode:?}\nattack: 17\nattack_unit: samples\nrelease: 19\nrelease_unit: samples\n{extra}"
            );
            let mut processor: Box<dyn Processor> = match kind {
                "Compressor" => {
                    let p: config::CompressorParameters = yaml_serde::from_str(&yaml).unwrap();
                    Box::new(camilladsp::processors::compressor::Compressor::from_config(
                        "p", p, 48000, 64,
                    ))
                }
                "NoiseGate" => {
                    let p: config::NoiseGateParameters = yaml_serde::from_str(&yaml).unwrap();
                    Box::new(camilladsp::processors::noisegate::NoiseGate::from_config(
                        "p", p, 48000, 64,
                    ))
                }
                _ => {
                    let p: config::LookaheadLimiterProcessorParameters =
                        yaml_serde::from_str(&yaml).unwrap();
                    Box::new(
                        camilladsp::processors::lookahead_limiter::LookaheadLimiter::from_config(
                            "p", p, 48000, 64,
                        ),
                    )
                }
            };
            let mut input = AudioChunk::new(vec![vec![0.5; 64]; 4], 1.0, -1.0, 64, 64);
            for _ in 0..4 {
                processor.process_chunk(&mut input);
            }
            ALLOCATIONS.with(|count| count.set(0));
            TRACK.with(|tracking| tracking.set(true));
            for _ in 0..32 {
                for waveform in &mut input.waveforms {
                    waveform.fill(0.5);
                }
                processor.process_chunk(&mut input);
            }
            TRACK.with(|tracking| tracking.set(false));
            let count = ALLOCATIONS.with(Cell::get);
            assert_eq!(count, 0, "{kind} {mode:?} allocated {count} times");
        }
    }
}
