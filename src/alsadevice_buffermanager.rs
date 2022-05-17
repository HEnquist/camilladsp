extern crate alsa;
extern crate nix;
use alsa::pcm::{Frames, HwParams, SwParams};
use std::fmt::Debug;
use std::thread;
use std::time::Duration;

use crate::Res;

pub trait DeviceBufferManager {
    // intended for internal use
    fn get_data(&self) -> &DeviceBufferData;
    fn get_data_mut(&mut self) -> &mut DeviceBufferData;

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()>;

    // Calculate a power-of-two buffer size that is large enough to accommodate any changes due to resampling.
    fn calculate_buffer_size(&self) -> Frames {
        let data = self.get_data();
        2.0f32.powf(
            (1.2 * data.chunksize as f32 / data.resampling_ratio)
                .log2()
                .ceil(),
        ) as Frames
    }

    // Calculate a buffer size and apply it to a hwp container. Only for use when opening a device.
    fn apply_buffer_size(&mut self, hwp: &HwParams) -> Res<()> {
        let buffer_frames = self.calculate_buffer_size();
        let data = self.get_data_mut();
        data.bufsize = hwp.set_buffer_size_near(buffer_frames)?;
        if data.bufsize < buffer_frames {
            warn!(
                "Unable to set the desired device buffer size, requested {}, got {}",
                buffer_frames, data.bufsize
            );
        }
        Ok(())
    }

    // Calculate a period size and apply it to a hwp container. Only for use when opening a device, after setting buffer size.
    fn apply_period_size(&mut self, hwp: &HwParams) -> Res<()> {
        let data = self.get_data_mut();
        data.period = hwp.set_period_size_near(data.bufsize / 8, alsa::ValueOr::Nearest)?;
        Ok(())
    }

    // Update avail_min so set target for snd_pcm_wait.
    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()> {
        let data = self.get_data_mut();
        // maximum timing safety - headroom for one io_size only
        if data.io_size < data.period {
            warn!(
                "Trying to set avail_min to {}, must be larger than or equal to period of {}",
                data.io_size, data.period
            );
        } else if data.io_size > data.bufsize {
            warn!(
                "Trying to set avail_min to {}, must be smaller than or equal to device buffer size of {}",
                data.io_size, data.bufsize
            );
        }
        data.avail_min = data.io_size;
        swp.set_avail_min(data.avail_min)?;
        Ok(())
    }

    fn update_io_size(&mut self, swp: &SwParams, io_size: Frames) -> Res<()> {
        let data = self.get_data_mut();
        data.io_size = io_size;
        // must update avail_min
        self.apply_avail_min(swp)?;
        // must update threshold
        self.apply_start_threshold(swp)?;
        Ok(())
    }

    fn get_frames_to_stall(&mut self) -> Frames {
        let data = self.get_data_mut();
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
    chunksize: Frames,
    resampling_ratio: f32,
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
    fn get_data(&self) -> &DeviceBufferData {
        &self.data
    }

    fn get_data_mut(&mut self) -> &mut DeviceBufferData {
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
    fn get_data(&self) -> &DeviceBufferData {
        &self.data
    }

    fn get_data_mut(&mut self) -> &mut DeviceBufferData {
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
