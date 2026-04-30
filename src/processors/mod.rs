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

/// Dynamic range compressor processor.
pub mod compressor;
/// Noise gate processor.
pub mod noisegate;
/// RACE (Recursive Ambiophonic Crosstalk Elimination) processor.
pub mod race;

/// Trait implemented by all multi-channel audio processors.
pub trait Processor {
    /// Apply the processor to all channels of `chunk` in place.
    fn process_chunk(&mut self, chunk: &mut AudioChunk) -> Res<()>;

    /// Hot-reload processor parameters from a new configuration without rebuilding.
    fn update_parameters(&mut self, config: config::Processor);

    /// Return the processor's name as given in the configuration.
    fn name(&self) -> &str;
}
