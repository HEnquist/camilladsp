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

use alsa::pcm::{Frames, SwParams};
use std::fmt::Debug;
use std::thread;
use std::time::Duration;

use crate::Res;
use crate::alsa_backend::device_buffer_manager::{DeviceBufferData, DeviceBufferManager};
use crate::config;

fn synchronous_apply_avail_min(swp: &SwParams<'_>, data: &mut DeviceBufferData) -> Res<()> {
    // maximum timing safety - headroom for one io_size only
    if data.io_size > data.bufsize {
        let msg = format!(
            "Trying to set avail_min to {}, must be smaller than or equal to device buffer size of {}",
            data.io_size, data.bufsize
        );
        error!("{msg}");
        return Err(config::ConfigError::new(&msg).into());
    }
    data.avail_min = data.io_size;
    swp.set_avail_min(data.avail_min)?;
    Ok(())
}

#[derive(Debug)]
pub struct CaptureBufferManager {
    pub data: DeviceBufferData,
}

impl CaptureBufferManager {
    pub fn new(chunksize: Frames, resampling_ratio: f32) -> Self {
        let init_io_size = (chunksize as f32 / resampling_ratio) as Frames;
        CaptureBufferManager {
            data: DeviceBufferData {
                bufsize: 0,
                period: 0,
                threshold: 0,
                avail_min: 0,
                io_size: init_io_size,
                resampling_ratio,
                chunksize,
            },
        }
    }
}

impl DeviceBufferManager for CaptureBufferManager {
    fn data(&self) -> &DeviceBufferData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut DeviceBufferData {
        &mut self.data
    }

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()> {
        // immediate start after pcmdev.prepare
        let threshold = 0;
        swp.set_start_threshold(threshold)?;
        self.data.threshold = threshold;
        Ok(())
    }

    fn current_delay(&self, avail: Frames) -> Frames {
        avail
    }

    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()> {
        let data = self.data_mut();
        synchronous_apply_avail_min(swp, data)
    }
}

#[derive(Debug)]
pub struct PlaybackBufferManager {
    pub data: DeviceBufferData,
    target_level: Frames,
}

impl PlaybackBufferManager {
    pub fn new(chunksize: Frames, target_level: Frames) -> Self {
        PlaybackBufferManager {
            data: DeviceBufferData {
                bufsize: 0,
                period: 0,
                threshold: 0,
                avail_min: 0,
                io_size: chunksize,
                resampling_ratio: 1.0,
                chunksize,
            },
            target_level,
        }
    }

    pub fn sleep_for_target_delay(&self, millis_per_frame: f32) {
        let sleep_millis = (self.target_level as f32 * millis_per_frame) as u64;
        trace!(
            "Sleeping for {} frames = {} ms",
            self.target_level, sleep_millis
        );
        thread::sleep(Duration::from_millis(sleep_millis));
    }
}

impl DeviceBufferManager for PlaybackBufferManager {
    fn data(&self) -> &DeviceBufferData {
        &self.data
    }

    fn data_mut(&mut self) -> &mut DeviceBufferData {
        &mut self.data
    }

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()> {
        // start on first write of any size
        let threshold = 1;
        swp.set_start_threshold(threshold)?;
        self.data.threshold = threshold;
        Ok(())
    }

    fn current_delay(&self, avail: Frames) -> Frames {
        self.data.bufsize - avail
    }

    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()> {
        let data = self.data_mut();
        synchronous_apply_avail_min(swp, data)
    }
}
