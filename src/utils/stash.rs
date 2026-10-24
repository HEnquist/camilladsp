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

use crossbeam_queue::ArrayQueue;
use std::sync::LazyLock;

use crate::CamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config;
use crate::utils::resampling::max_capture_frames;

const MAX_STASH_SIZE: usize = 1024;
const MAX_CONTAINER_STASH_SIZE: usize = 128;

/// Global stash of reusable `Vec<CamillaFloat>` audio waveform buffers, avoiding repeated allocations.
pub static BUFFERSTASH: LazyLock<ArrayQueue<Vec<CamillaFloat>>> =
    LazyLock::new(|| ArrayQueue::new(MAX_STASH_SIZE));
/// Global stash of reusable `Vec<Vec<CamillaFloat>>` channel-container buffers.
pub static CONTAINERSTASH: LazyLock<ArrayQueue<Vec<Vec<CamillaFloat>>>> =
    LazyLock::new(|| ArrayQueue::new(MAX_CONTAINER_STASH_SIZE));

fn vec_from_queue(queue: &ArrayQueue<Vec<CamillaFloat>>, capacity: usize) -> Vec<CamillaFloat> {
    trace!(
        "Try to get a vector from the stash, nbr available: {}",
        queue.len()
    );
    if let Some(mut vector) = queue.pop() {
        if capacity != vector.len() {
            if capacity > vector.capacity() {
                trace!(
                    "The stashed vector has insufficient capacity, allocating more space {} -> {}",
                    vector.capacity(),
                    capacity
                );
            }
            vector.resize(capacity, 0.0);
        }
        vector
    } else {
        trace!("Stash is empty, allocating a new vector");
        vec![0.0; capacity]
    }
}

fn container_from_queue(
    queue: &ArrayQueue<Vec<Vec<CamillaFloat>>>,
    capacity: usize,
) -> Vec<Vec<CamillaFloat>> {
    trace!(
        "Try to get a vector container from the stash, nbr available: {}",
        queue.len()
    );
    if let Some(mut vector) = queue.pop() {
        if capacity > vector.capacity() {
            trace!(
                "The stashed container vector has insufficient capacity, allocating more space {} -> {}",
                vector.capacity(),
                capacity
            );
            vector.reserve_exact(capacity - vector.capacity());
        }
        vector
    } else {
        trace!("Stash is empty, allocating a new container vector");
        Vec::with_capacity(capacity)
    }
}

fn recycle_vec_to_queue(queue: &ArrayQueue<Vec<CamillaFloat>>, mut vector: Vec<CamillaFloat>) {
    trace!("Recycling a vector");

    for elem in vector.iter_mut() {
        *elem = 0.0;
    }

    if queue.push(vector).is_err() {
        trace!("Stash is full, dropping a vector");
    }
}

fn recycle_container_to_queue(
    container_queue: &ArrayQueue<Vec<Vec<CamillaFloat>>>,
    vector_queue: &ArrayQueue<Vec<CamillaFloat>>,
    mut container: Vec<Vec<CamillaFloat>>,
) {
    trace!("Recycling a container of vectors");
    for vector in container.drain(..) {
        recycle_vec_to_queue(vector_queue, vector);
    }
    if container_queue.push(container).is_err() {
        trace!("Stash is full, dropping a container");
    }
}

/// A vector of `frames` zeros, with every element written. `vec![0.0; n]`
/// gets zeroed memory from the allocator, which the OS may only map when it is
/// first written, and that write would then happen on an audio thread.
fn touched_vec(frames: usize) -> Vec<CamillaFloat> {
    let mut vector = Vec::with_capacity(frames);
    vector.resize(frames, 0.0);
    vector
}

fn prefill_queues(
    container_queue: &ArrayQueue<Vec<Vec<CamillaFloat>>>,
    vector_queue: &ArrayQueue<Vec<CamillaFloat>>,
    frames: usize,
    channels: usize,
    chunks: usize,
) {
    // Grow what is already stashed. Each one is taken out and put back once,
    // so the audio threads can keep borrowing and returning meanwhile.
    for _ in 0..vector_queue.len() {
        if let Some(mut vector) = vector_queue.pop() {
            if vector.capacity() < frames {
                vector.resize(frames, 0.0);
            }
            let _ = vector_queue.push(vector);
        }
    }
    for _ in 0..container_queue.len() {
        if let Some(mut container) = container_queue.pop() {
            container.reserve_exact(channels);
            let _ = container_queue.push(container);
        }
    }

    let vectors = (chunks * channels).min(vector_queue.capacity());
    while vector_queue.len() < vectors {
        if vector_queue.push(touched_vec(frames)).is_err() {
            break;
        }
    }
    let containers = chunks.min(container_queue.capacity());
    while container_queue.len() < containers {
        if container_queue.push(Vec::with_capacity(channels)).is_err() {
            break;
        }
    }
}

/// Fill the stash so that the audio threads never have to allocate. Stashed
/// vectors are grown to hold `frames`, and more are added until there are
/// enough for `chunks` chunks of `channels` channels.
///
/// Call it from a control thread before the devices start, and again when a
/// config change can widen the pipeline. Without it the stash still reaches
/// the same state by itself, but by allocating on the audio threads during
/// the first chunks, which is the worst moment for it.
pub fn prefill(frames: usize, channels: usize, chunks: usize) {
    debug!("Prefill the stash for {chunks} chunks of {channels} channels and {frames} frames");
    prefill_queues(&CONTAINERSTASH, &BUFFERSTASH, frames, channels, chunks);
}

/// Chunks in use beyond those waiting in the playback queue. The capture queue
/// normally stays close to empty, so allow two there. The rest are being worked
/// on: one being captured, the old and new while resampling, the old and new
/// while mixing, and one being played. Over-estimating only costs memory.
const CHUNKS_OUTSIDE_QUEUE: usize = 6;

/// [`prefill`] with the sizes `conf` needs: vectors long enough for either
/// side of the resampler, and enough of them for the widest part of the
/// pipeline.
pub fn prefill_for_config(conf: &config::Configuration) {
    prefill(
        max_capture_frames(&conf.devices),
        config::max_channels(conf),
        conf.devices.queuelimit() + CHUNKS_OUTSIDE_QUEUE,
    );
}

/// Borrow a zeroed `Vec<CamillaFloat>` of the given length from the stash, allocating if empty.
pub fn vec_from_stash(capacity: usize) -> Vec<CamillaFloat> {
    vec_from_queue(&BUFFERSTASH, capacity)
}

/// Borrow a `Vec<Vec<CamillaFloat>>` container of the given capacity from the stash, allocating if empty.
pub fn container_from_stash(capacity: usize) -> Vec<Vec<CamillaFloat>> {
    container_from_queue(&CONTAINERSTASH, capacity)
}

/// Return a `Vec<CamillaFloat>` to the stash for reuse. The vector is zeroed before stashing.
pub fn recycle_vec(vector: Vec<CamillaFloat>) {
    recycle_vec_to_queue(&BUFFERSTASH, vector);
}

/// Return a channel container and all its inner waveform vectors to the stash.
pub fn recycle_container(container: Vec<Vec<CamillaFloat>>) {
    recycle_container_to_queue(&CONTAINERSTASH, &BUFFERSTASH, container);
}

/// Return all waveform buffers of an [`AudioChunk`] to the stash.
pub fn recycle_chunk(chunk: AudioChunk) {
    recycle_container(chunk.waveforms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recycled_vec_is_zeroed_and_resized_when_reused() {
        let queue = ArrayQueue::new(1);
        recycle_vec_to_queue(&queue, vec![1.0, 2.0, 3.0]);

        let reused = vec_from_queue(&queue, 5);

        assert_eq!(reused, vec![0.0; 5]);
    }

    #[test]
    fn recycled_container_returns_empty_container_with_capacity() {
        let vector_queue = ArrayQueue::new(4);
        let container_queue = ArrayQueue::new(1);
        let container = vec![vec![1.0, 2.0], vec![3.0]];

        recycle_container_to_queue(&container_queue, &vector_queue, container);

        let reused = container_from_queue(&container_queue, 2);
        assert!(reused.is_empty());
        assert!(reused.capacity() >= 2);

        let first = vec_from_queue(&vector_queue, 2);
        let second = vec_from_queue(&vector_queue, 1);
        assert_eq!(first, vec![0.0, 0.0]);
        assert_eq!(second, vec![0.0]);
    }

    #[test]
    fn prefill_grows_stashed_and_adds_missing() {
        let vector_queue = ArrayQueue::new(16);
        let container_queue = ArrayQueue::new(4);
        recycle_vec_to_queue(&vector_queue, vec![1.0; 10]);
        container_queue.push(Vec::with_capacity(1)).unwrap();

        prefill_queues(&container_queue, &vector_queue, 100, 3, 2);

        assert_eq!(vector_queue.len(), 6);
        assert_eq!(container_queue.len(), 2);
        while let Some(vector) = vector_queue.pop() {
            assert!(vector.capacity() >= 100);
            assert!(vector.iter().all(|v| *v == 0.0));
        }
        while let Some(container) = container_queue.pop() {
            assert!(container.capacity() >= 3);
        }
    }

    #[test]
    fn prefill_stops_at_the_queue_size() {
        let vector_queue = ArrayQueue::new(4);
        let container_queue = ArrayQueue::new(1);

        prefill_queues(&container_queue, &vector_queue, 8, 3, 2);

        assert_eq!(vector_queue.len(), 4);
        assert_eq!(container_queue.len(), 1);
    }

    #[test]
    fn full_queue_drops_recycled_vec() {
        let queue = ArrayQueue::new(1);
        recycle_vec_to_queue(&queue, vec![1.0]);
        recycle_vec_to_queue(&queue, vec![2.0]);

        let reused = vec_from_queue(&queue, 1);
        assert_eq!(reused, vec![0.0]);
        assert!(queue.is_empty());
    }
}
