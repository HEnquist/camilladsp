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

use parking_lot::RwLock;

use crate::PrcFmt;
use crate::audiochunk::AudioChunk;

const MAX_STASH_SIZE: usize = 1024;
const MAX_CONTAINER_STASH_SIZE: usize = 128;

lazy_static! {
    pub static ref BUFFERSTASH: RwLock<Vec<Vec<PrcFmt>>> =
        RwLock::new(Vec::with_capacity(MAX_STASH_SIZE));
    pub static ref CONTAINERSTASH: RwLock<Vec<Vec<Vec<PrcFmt>>>> =
        RwLock::new(Vec::with_capacity(MAX_CONTAINER_STASH_SIZE));
}

pub fn vec_from_stash(capacity: usize) -> Vec<PrcFmt> {
    let vec_option = {
        let mut stash = BUFFERSTASH.write();
        trace!(
            "Try to get a vector from the stash, nbr available: {}",
            stash.len()
        );
        stash.pop()
    };
    if let Some(mut vector) = vec_option {
        if capacity != vector.len() {
            if capacity > vector.capacity() {
                debug!(
                    "The stashed vector has insufficient capacity, allocating more space {} -> {}",
                    vector.capacity(),
                    capacity
                );
            }
            vector.resize(capacity, 0.0);
        }
        vector
    } else {
        debug!("Stash is empty, allocating a new vector");
        vec![0.0; capacity]
    }
}

pub fn container_from_stash(capacity: usize) -> Vec<Vec<PrcFmt>> {
    let vec_option = {
        let mut stash = CONTAINERSTASH.write();
        trace!(
            "Try to get a vector container from the stash, nbr available: {}",
            stash.len()
        );
        stash.pop()
    };
    if let Some(mut vector) = vec_option {
        if capacity > vector.capacity() {
            debug!(
                "The stashed container vector has insufficient capacity, allocating more space {} -> {}",
                vector.capacity(),
                capacity
            );
            vector.reserve_exact(capacity - vector.capacity());
        }
        vector
    } else {
        debug!("Stash is empty, allocating a new container vector");
        Vec::with_capacity(capacity)
    }
}

pub fn recycle_vec(mut vector: Vec<PrcFmt>) {
    trace!("Recycling a vector");
    {
        let stash = BUFFERSTASH.read();
        if stash.len() >= MAX_STASH_SIZE {
            trace!("Stash is full, dropping a vector");
            return;
        }
    }

    for elem in vector.iter_mut() {
        *elem = 0.0;
    }

    let mut stash = BUFFERSTASH.write();
    if stash.len() >= MAX_STASH_SIZE {
        trace!("Stash is full, dropping a vector");
        return;
    }
    stash.push(vector);
}

pub fn recycle_container(mut container: Vec<Vec<PrcFmt>>) {
    trace!("Recycling a container of vectors");
    for vector in container.drain(..) {
        recycle_vec(vector);
    }
    let mut stash = CONTAINERSTASH.write();
    if stash.len() >= MAX_CONTAINER_STASH_SIZE {
        trace!("Stash is full, dropping a container");
        return;
    }
    stash.push(container);
}

pub fn recycle_chunk(chunk: AudioChunk) {
    recycle_container(chunk.waveforms);
}
