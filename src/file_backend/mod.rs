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

#[cfg(all(target_os = "linux", feature = "bluez-backend"))]
pub mod bluez;
pub mod device;
#[cfg(not(target_os = "linux"))]
pub mod filereader;
#[cfg(target_os = "linux")]
pub mod filereader_nonblock;
