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

//! Test-only capture and playback devices, behind the `dummy-backend` feature.
//!
//! They move audio at a paced rate without touching any hardware, so the whole
//! binary can be driven end to end on a CI runner with no sound card, no kernel
//! module and no driver. They are not documented for users and must never be
//! enabled in a release build.

pub mod device;
pub mod pacer;
