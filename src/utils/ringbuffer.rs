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

use ringbuf::traits::*;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many times [`RingBufferFeeder::push`] waits for room before it drops a chunk.
const PUSH_RETRIES: usize = 16;

/// The shortest time after a restart that [`BacklogTrimmer`] watches the level.
const BACKLOG_WINDOW: Duration = Duration::from_millis(500);

/// The shortest time after a restart that [`BacklogTrimmer`] watches the level, in chunks.
const BACKLOG_WINDOW_CHUNKS: f64 = 4.0;

/// Pushes whole playback chunks into a device ring buffer.
///
/// When the ring buffer is full it waits for the device to drain it, half a chunk duration at a
/// time. That wait is what paces a source that is not rate limited, such as the signal generator.
/// Without it the data would arrive far faster than the device can play it, and most of it would
/// be dropped. If there is still no room after `PUSH_RETRIES` waits the device is not draining,
/// and the chunk is dropped whole. That is warned about once per episode, and each dropped chunk
/// is logged at trace.
pub struct RingBufferFeeder {
    retry_sleep: Duration,
    ring_full: bool,
}

impl RingBufferFeeder {
    pub fn new(chunksize: usize, samplerate: usize) -> Self {
        RingBufferFeeder {
            retry_sleep: Duration::from_secs_f64(chunksize as f64 / samplerate as f64 / 2.0),
            ring_full: false,
        }
    }

    /// Push all of `data`, or none of it. Returns `false` if the chunk was dropped.
    pub fn push(&mut self, producer: &mut impl Producer<Item = u8>, data: &[u8]) -> bool {
        for _ in 0..PUSH_RETRIES {
            if producer.vacant_len() >= data.len() {
                break;
            }
            std::thread::sleep(self.retry_sleep);
        }
        if producer.vacant_len() >= data.len() {
            producer.push_slice(data);
            self.ring_full = false;
            true
        } else {
            if !self.ring_full {
                warn!("Playback ring buffer is full, dropping chunks");
                self.ring_full = true;
            }
            trace!(
                "Playback ring buffer is full, dropped chunk of {} bytes",
                data.len()
            );
            false
        }
    }
}

/// Drops the backlog that arrives right after the playback device was restarted.
///
/// When no data reaches the playback device for a while, the device runs low or underruns,
/// and it is restarted at the target level. The data that was held up somewhere before the
/// device, for example in the capture device, then arrives all at once on top of that.
/// The level ends up above the target by about the length of the gap, and rate adjust
/// takes a long time to bring it down, since it only changes the rate by a fraction of a percent.
/// The backlog can not be counted when the device is restarted, since it is not visible yet.
///
/// Instead, for a short time after each restart, chunks are dropped while the level is more
/// than one chunk above the target. How big the backlog is depends on where in a chunk period
/// the gap ends, so it is often less than two chunks even after a long gap.
/// A burst of a chunk or two from the capture side can then also lose a chunk.
/// That drops audio, so it is only done right after a restart, when the audio already has a gap.
pub struct BacklogTrimmer {
    target_level: usize,
    chunksize: usize,
    window: Duration,
    restarted_at: Option<Instant>,
    dropped_frames: usize,
}

impl BacklogTrimmer {
    pub fn new(target_level: usize, chunksize: usize, samplerate: usize) -> Self {
        let window =
            Duration::from_secs_f64(BACKLOG_WINDOW_CHUNKS * chunksize as f64 / samplerate as f64)
                .max(BACKLOG_WINDOW);
        BacklogTrimmer {
            target_level,
            chunksize,
            window,
            restarted_at: None,
            dropped_frames: 0,
        }
    }

    /// The playback device was just restarted, start watching the level.
    pub fn restarted(&mut self) {
        self.restarted_at = Some(Instant::now());
    }

    /// Check the `level` measured as a chunk arrives, `None` if it is not known.
    /// Returns `true` if the chunk should be dropped.
    pub fn drop_chunk(&mut self, level: Option<usize>) -> bool {
        let Some(restarted_at) = self.restarted_at else {
            return false;
        };
        if restarted_at.elapsed() > self.window {
            self.restarted_at = None;
            self.finish();
            return false;
        }
        let Some(level) = level else {
            return false;
        };
        if level > self.target_level + self.chunksize {
            self.dropped_frames += self.chunksize;
            return true;
        }
        self.finish();
        false
    }

    fn finish(&mut self) {
        if self.dropped_frames > 0 {
            warn!(
                "Dropped {} frames that arrived late after a playback restart",
                self.dropped_frames
            );
        }
        self.dropped_frames = 0;
    }
}

/// Copy available bytes from a ring buffer consumer into `out_slice`,
/// then zero-fill any remaining tail.
///
/// This ensures the output buffer is always fully initialized, which is
/// critical for playback callbacks where an incomplete or stale buffer
/// would cause audio glitches.
///
/// Returns `(available_bytes, bytes_from_rb)` where:
/// - `available_bytes` is the number of bytes that were in the ring buffer
///   before the call (may exceed `out_slice.len()`).
/// - `bytes_from_rb` is the number of bytes actually copied into `out_slice`
///   (capped at `out_slice.len()`).
pub fn fill_playback_output_from_ringbuffer(
    consumer: &mut impl Consumer<Item = u8>,
    out_slice: &mut [u8],
) -> (usize, usize) {
    let max_bytes = out_slice.len();
    let available_bytes = consumer.occupied_len();
    let bytes_from_rb = available_bytes.min(max_bytes);

    if bytes_from_rb > 0 {
        consumer.pop_slice(&mut out_slice[..bytes_from_rb]);
    }
    if bytes_from_rb < max_bytes {
        out_slice[bytes_from_rb..max_bytes].fill(0);
    }

    (available_bytes, bytes_from_rb)
}

/// Move up to `max_bytes` bytes from a ring buffer consumer to the back of `queue`.
///
/// The bytes are copied as whole slices, instead of one at a time,
/// to keep the work in real-time playback callbacks low.
///
/// Returns the number of bytes moved.
pub fn append_from_ringbuffer(
    consumer: &mut impl Consumer<Item = u8>,
    queue: &mut VecDeque<u8>,
    max_bytes: usize,
) -> usize {
    let (first, second) = consumer.as_slices();
    let from_first = first.len().min(max_bytes);
    let from_second = second.len().min(max_bytes - from_first);
    queue.extend(&first[..from_first]);
    queue.extend(&second[..from_second]);
    consumer.skip(from_first + from_second)
}

#[cfg(test)]
mod tests {
    use super::{
        BacklogTrimmer, RingBufferFeeder, append_from_ringbuffer,
        fill_playback_output_from_ringbuffer,
    };
    use ringbuf::{HeapRb, traits::*};
    use std::collections::VecDeque;
    use std::time::Duration;

    #[test]
    fn full_underrun_outputs_silence() {
        let ring = HeapRb::<u8>::new(16);
        let (_producer, mut consumer) = ring.split();

        let mut out = vec![0xAA; 8];
        let (available_bytes, bytes_from_rb) =
            fill_playback_output_from_ringbuffer(&mut consumer, &mut out);

        assert_eq!(available_bytes, 0);
        assert_eq!(bytes_from_rb, 0);
        assert_eq!(out, vec![0; 8]);
    }

    #[test]
    fn partial_underrun_zero_pads_tail() {
        let ring = HeapRb::<u8>::new(16);
        let (mut producer, mut consumer) = ring.split();

        let pushed = producer.push_slice(&[1, 2, 3]);
        assert_eq!(pushed, 3);

        let mut out = vec![0xAA; 8];
        let (available_bytes, bytes_from_rb) =
            fill_playback_output_from_ringbuffer(&mut consumer, &mut out);

        assert_eq!(available_bytes, 3);
        assert_eq!(bytes_from_rb, 3);
        assert_eq!(out, vec![1, 2, 3, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn excess_data_writes_full_buffer_and_leaves_remainder() {
        let ring = HeapRb::<u8>::new(32);
        let (mut producer, mut consumer) = ring.split();

        let pushed = producer.push_slice(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert_eq!(pushed, 10);

        let mut out = vec![0xAA; 8];
        let (available_bytes, bytes_from_rb) =
            fill_playback_output_from_ringbuffer(&mut consumer, &mut out);

        assert_eq!(available_bytes, 10);
        assert_eq!(bytes_from_rb, 8);
        assert_eq!(out, vec![10, 11, 12, 13, 14, 15, 16, 17]);

        // Two bytes should remain unconsumed
        assert_eq!(consumer.occupied_len(), 2);
    }

    #[test]
    fn exact_fit_no_zero_padding() {
        let ring = HeapRb::<u8>::new(16);
        let (mut producer, mut consumer) = ring.split();

        let pushed = producer.push_slice(&[5, 6, 7, 8]);
        assert_eq!(pushed, 4);

        let mut out = vec![0xAA; 4];
        let (available_bytes, bytes_from_rb) =
            fill_playback_output_from_ringbuffer(&mut consumer, &mut out);

        assert_eq!(available_bytes, 4);
        assert_eq!(bytes_from_rb, 4);
        assert_eq!(out, vec![5, 6, 7, 8]);
    }

    #[test]
    fn empty_output_slice_returns_zeros() {
        let ring = HeapRb::<u8>::new(16);
        let (mut producer, mut consumer) = ring.split();

        producer.push_slice(&[1, 2, 3]);

        let mut out: Vec<u8> = vec![];
        let (available_bytes, bytes_from_rb) =
            fill_playback_output_from_ringbuffer(&mut consumer, &mut out);

        assert_eq!(available_bytes, 3);
        assert_eq!(bytes_from_rb, 0);
        assert!(out.is_empty());
        // Nothing was consumed
        assert_eq!(consumer.occupied_len(), 3);
    }

    #[test]
    fn append_from_wrapped_ringbuffer() {
        // Advance the read position so that the content wraps around the end of the buffer.
        let mut ring = HeapRb::<u8>::new(8);
        ring.push_slice(&[0; 6]);
        ring.skip(6);
        ring.push_slice(&[1, 2, 3, 4, 5]);
        let (_producer, mut consumer) = ring.split();
        let (first, second) = consumer.as_slices();
        assert!(!first.is_empty() && !second.is_empty());

        let mut queue = VecDeque::from(vec![9]);
        let moved = append_from_ringbuffer(&mut consumer, &mut queue, 4);
        assert_eq!(moved, 4);
        assert_eq!(queue, vec![9, 1, 2, 3, 4]);
        assert_eq!(consumer.occupied_len(), 1);

        let moved = append_from_ringbuffer(&mut consumer, &mut queue, 10);
        assert_eq!(moved, 1);
        assert_eq!(queue, vec![9, 1, 2, 3, 4, 5]);
        assert_eq!(consumer.occupied_len(), 0);
    }

    // A huge sample rate makes the retry sleep zero, so a full ring fails at once.
    const NO_SLEEP_RATE: usize = 10_000_000;

    #[test]
    fn feeder_pushes_a_chunk_that_fits() {
        let ring = HeapRb::<u8>::new(8);
        let (mut producer, consumer) = ring.split();
        let mut feeder = RingBufferFeeder::new(1, NO_SLEEP_RATE);
        assert!(feeder.push(&mut producer, &[1, 2, 3, 4, 5, 6]));
        assert_eq!(consumer.occupied_len(), 6);
    }

    #[test]
    fn feeder_drops_a_chunk_that_does_not_fit_whole() {
        let ring = HeapRb::<u8>::new(8);
        let (mut producer, consumer) = ring.split();
        let mut feeder = RingBufferFeeder::new(1, NO_SLEEP_RATE);
        assert!(feeder.push(&mut producer, &[1, 2, 3, 4, 5, 6]));
        assert!(!feeder.push(&mut producer, &[7, 8, 9, 10]));
        assert_eq!(consumer.occupied_len(), 6);
    }

    #[test]
    fn feeder_waits_for_the_consumer_to_make_room() {
        let ring = HeapRb::<u8>::new(8);
        let (mut producer, mut consumer) = ring.split();
        // 9600 frames at 48 kHz sleeps 100 ms per retry, 1.6 s in total. The wide margin over
        // the 20 ms drain keeps the test from failing when a loaded runner schedules it late.
        let mut feeder = RingBufferFeeder::new(9600, 48000);
        assert!(feeder.push(&mut producer, &[0; 8]));
        let drain = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            consumer.skip(8);
        });
        assert!(feeder.push(&mut producer, &[1; 8]));
        drain.join().unwrap();
    }

    // Target 2000 frames, chunks of 1000. Chunks are dropped while the level is above 3000.
    fn trimmer() -> BacklogTrimmer {
        BacklogTrimmer::new(2000, 1000, 48000)
    }

    #[test]
    fn trimmer_keeps_everything_without_a_restart() {
        let mut trimmer = trimmer();
        assert!(!trimmer.drop_chunk(Some(10000)));
    }

    #[test]
    fn trimmer_drops_a_backlog_down_to_a_chunk_above_the_target() {
        let mut trimmer = trimmer();
        trimmer.restarted();
        let dropped: Vec<bool> = [2000, 6500, 5500, 4500, 3500, 2500, 2900]
            .into_iter()
            .map(|level| trimmer.drop_chunk(Some(level)))
            .collect();
        assert_eq!(dropped, [false, true, true, true, true, false, false]);
    }

    #[test]
    fn trimmer_drops_a_backlog_of_less_than_two_chunks() {
        let mut trimmer = trimmer();
        trimmer.restarted();
        let dropped: Vec<bool> = [2000, 3700, 2700, 2500]
            .into_iter()
            .map(|level| trimmer.drop_chunk(Some(level)))
            .collect();
        assert_eq!(dropped, [false, true, false, false]);
    }

    #[test]
    fn trimmer_keeps_a_chunk_with_an_unknown_level() {
        let mut trimmer = trimmer();
        trimmer.restarted();
        assert!(!trimmer.drop_chunk(None));
        assert!(trimmer.drop_chunk(Some(6000)));
    }

    #[test]
    fn trimmer_stops_after_the_window() {
        let mut trimmer = trimmer();
        trimmer.window = Duration::ZERO;
        trimmer.restarted();
        std::thread::sleep(Duration::from_millis(1));
        assert!(!trimmer.drop_chunk(Some(10000)));
    }
}
