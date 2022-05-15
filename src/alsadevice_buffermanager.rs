extern crate alsa;
extern crate nix;
use alsa::pcm::{Frames, HwParams, SwParams};
use std::fmt::Debug;
use std::thread;
use std::time::Duration;

use crate::Res;

pub trait DeviceBufferManager {
    // intended for internal use
    fn get_data(&mut self) -> &mut DeviceBufferData;
    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()>;

    fn apply_buffer_size(&mut self, hwp: &HwParams) -> Res<()> {
        let data = self.get_data();
        data.bufsize = hwp.set_buffer_size_near(data.io_size * 2)?;
        Ok(())
    }
    fn apply_period_size(&mut self, hwp: &HwParams) -> Res<()> {
        let data = self.get_data();
        data.period = hwp.set_period_size_near(data.io_size / 4, alsa::ValueOr::Nearest)?;
        Ok(())
    }

    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()> {
        let data = self.get_data();
        // maximum timing safety - headroom for one io_size only
        data.avail_min = data.io_size;
        swp.set_avail_min(data.io_size)?;
        Ok(())
    }

    fn update_io_size(&mut self, swp: &SwParams, io_size: Frames) -> Res<()> {
        let data = self.get_data();
        data.io_size = io_size;
        // must update avail_min
        swp.set_avail_min(io_size)?;
        data.avail_min = io_size;
        // must update threshold
        self.apply_start_threshold(swp)?;
        Ok(())
    }

    fn get_frames_to_stall(&mut self) -> Frames {
        let data = self.get_data();
        // +1 to make sure the device really stalls
        data.bufsize - data.avail_min + 1
    }
}

#[derive(Debug)]
pub struct DeviceBufferData {
    bufsize: Frames,
    period: Frames,
    threshold: Frames,
    avail_min: Frames,
    io_size: Frames, /* size of read/write block */
}

#[derive(Debug)]
pub struct CaptureBufferManager {
    pub data: DeviceBufferData,
}

impl CaptureBufferManager {
    pub fn new(init_io_size: Frames) -> Self {
        CaptureBufferManager {
            data: DeviceBufferData {
                bufsize: 0,
                period: 0,
                threshold: 0,
                avail_min: 0,
                io_size: init_io_size,
            },
        }
    }
}

impl DeviceBufferManager for CaptureBufferManager {
    fn get_data(&mut self) -> &mut DeviceBufferData {
        &mut self.data
    }

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()> {
        // immediate start after pcmdev.prepare
        let threshold = 0;
        swp.set_start_threshold(threshold)?;
        self.data.threshold = threshold;
        Ok(())
    }
}

#[derive(Debug)]
pub struct PlaybackBufferManager {
    pub data: DeviceBufferData,
    target_level: Frames,
}

impl PlaybackBufferManager {
    pub fn new(init_io_size: Frames, target_level: Frames) -> Self {
        PlaybackBufferManager {
            data: DeviceBufferData {
                bufsize: 0,
                period: 0,
                threshold: 0,
                avail_min: 0,
                io_size: init_io_size,
            },
            target_level,
        }
    }

    pub fn sleep_for_target_delay(&mut self, millis_per_frame: f32) {
        let sleep_millis = (self.target_level as f32 * millis_per_frame) as u64;
        trace!(
            "Sleeping for {} frames = {} ms",
            self.target_level,
            sleep_millis
        );
        thread::sleep(Duration::from_millis(sleep_millis));
    }
}

impl DeviceBufferManager for PlaybackBufferManager {
    fn get_data(&mut self) -> &mut DeviceBufferData {
        &mut self.data
    }

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()> {
        // start on first write of any size
        let threshold = 1;
        swp.set_start_threshold(threshold)?;
        self.data.threshold = threshold;
        Ok(())
    }
}
