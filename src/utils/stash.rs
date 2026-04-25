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

use crate::PrcFmt;
use crate::audiochunk::AudioChunk;

const MAX_STASH_SIZE: usize = 1024;
const MAX_CONTAINER_STASH_SIZE: usize = 128;

pub static BUFFERSTASH: LazyLock<ArrayQueue<Vec<PrcFmt>>> =
    LazyLock::new(|| ArrayQueue::new(MAX_STASH_SIZE));
pub static CONTAINERSTASH: LazyLock<ArrayQueue<Vec<Vec<PrcFmt>>>> =
    LazyLock::new(|| ArrayQueue::new(MAX_CONTAINER_STASH_SIZE));

fn vec_from_queue(queue: &ArrayQueue<Vec<PrcFmt>>, capacity: usize) -> Vec<PrcFmt> {
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

fn container_from_queue(queue: &ArrayQueue<Vec<Vec<PrcFmt>>>, capacity: usize) -> Vec<Vec<PrcFmt>> {
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

fn recycle_vec_to_queue(queue: &ArrayQueue<Vec<PrcFmt>>, mut vector: Vec<PrcFmt>) {
    trace!("Recycling a vector");

    for elem in vector.iter_mut() {
        *elem = 0.0;
    }

    if queue.push(vector).is_err() {
        trace!("Stash is full, dropping a vector");
    }
}

fn recycle_container_to_queue(
    container_queue: &ArrayQueue<Vec<Vec<PrcFmt>>>,
    vector_queue: &ArrayQueue<Vec<PrcFmt>>,
    mut container: Vec<Vec<PrcFmt>>,
) {
    trace!("Recycling a container of vectors");
    for vector in container.drain(..) {
        recycle_vec_to_queue(vector_queue, vector);
    }
    if container_queue.push(container).is_err() {
        trace!("Stash is full, dropping a container");
    }
}

pub fn vec_from_stash(capacity: usize) -> Vec<PrcFmt> {
    vec_from_queue(&BUFFERSTASH, capacity)
}

pub fn container_from_stash(capacity: usize) -> Vec<Vec<PrcFmt>> {
    container_from_queue(&CONTAINERSTASH, capacity)
}

pub fn recycle_vec(vector: Vec<PrcFmt>) {
    recycle_vec_to_queue(&BUFFERSTASH, vector);
}

pub fn recycle_container(container: Vec<Vec<PrcFmt>>) {
    recycle_container_to_queue(&CONTAINERSTASH, &BUFFERSTASH, container);
}

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
    fn full_queue_drops_recycled_vec() {
        let queue = ArrayQueue::new(1);
        recycle_vec_to_queue(&queue, vec![1.0]);
        recycle_vec_to_queue(&queue, vec![2.0]);

        let reused = vec_from_queue(&queue, 1);
        assert_eq!(reused, vec![0.0]);
        assert!(queue.is_empty());
    }
}
