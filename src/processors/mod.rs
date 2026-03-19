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

use crate::Res;
use crate::audiochunk::AudioChunk;
use crate::config;

pub mod compressor;
pub mod noisegate;
pub mod race;

pub trait Processor {
    // Process a chunk containing several channels.
    fn process_chunk(&mut self, chunk: &mut AudioChunk) -> Res<()>;

    fn update_parameters(&mut self, config: config::Processor);

    fn name(&self) -> &str;
}
