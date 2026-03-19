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

#[cfg(test)]
mod tests {
    use super::fill_playback_output_from_ringbuffer;
    use ringbuf::{HeapRb, traits::*};

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
}
