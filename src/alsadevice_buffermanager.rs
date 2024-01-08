extern crate alsa;
extern crate nix;
use alsa::pcm::{Frames, HwParams, SwParams};
use std::fmt::Debug;
use std::thread;
use std::time::Duration;

use crate::config;
use crate::Res;

pub trait DeviceBufferManager {
    // intended for internal use
    fn data(&self) -> &DeviceBufferData;
    fn data_mut(&mut self) -> &mut DeviceBufferData;

    fn apply_start_threshold(&mut self, swp: &SwParams) -> Res<()>;

    // Calculate a power-of-two buffer size that is large enough to accommodate any changes due to resampling,
    // and at least 4 times the minimum period size to avoid random broken pipes.
    fn calculate_buffer_size(&self, min_period: Frames) -> Frames {
        let data = self.data();
        let mut frames_needed = 3.0 * data.chunksize as f32 / data.resampling_ratio;
        if frames_needed < 4.0 * min_period as f32 {
            frames_needed = 4.0 * min_period as f32;
            debug!(
                "Minimum period is {} frames, buffer size is minimum {} frames",
                min_period, frames_needed
            );
        }
        2.0f32.powi(frames_needed.log2().ceil() as i32) as Frames
    }

    // Calculate an alternative buffer size that is 3 multiplied by a power-of-two,
    // and at least 4 times the minimum period size to avoid random broken pipes.
    // This is for some devices that cannot work with the default setting,
    // and when set_buffer_size_near() does not return a working alternative near the requested one.
    // Caused by driver bugs?
    fn calculate_buffer_size_alt(&self, min_period: Frames) -> Frames {
        let data = self.data();
        let mut frames_needed = 3.0 * data.chunksize as f32 / data.resampling_ratio;
        if frames_needed < 4.0 * min_period as f32 {
            frames_needed = 4.0 * min_period as f32;
            debug!(
                "Minimum period is {} frames, alternate buffer size is minimum {} frames",
                min_period, frames_needed
            );
        }
        3 * 2.0f32.powi((frames_needed / 3.0).log2().ceil() as i32) as Frames
    }

    // Calculate a buffer size and apply it to a hwp container. Only for use when opening a device.
    fn apply_buffer_size(&mut self, hwp: &HwParams) -> Res<()> {
        let min_period = hwp.get_period_size_min().unwrap_or(0);
        let buffer_frames = self.calculate_buffer_size(min_period);
        let alt_buffer_frames = self.calculate_buffer_size_alt(min_period);
        let data = self.data_mut();
        debug!("Setting buffer size to {} frames", buffer_frames);
        match hwp.set_buffer_size_near(buffer_frames) {
            Ok(frames) => {
                data.bufsize = frames;
            }
            Err(_) => {
                debug!(
                    "Device did not accept a buffer size of {} frames, trying again with {}",
                    buffer_frames, alt_buffer_frames
                );
                data.bufsize = hwp.set_buffer_size_near(alt_buffer_frames)?;
            }
        }
        debug!("Device is using a buffer size of {} frames", data.bufsize);
        Ok(())
    }

    // Calculate a period size and apply it to a hwp container. Only for use when opening a device, after setting buffer size.
    fn apply_period_size(&mut self, hwp: &HwParams) -> Res<()> {
        let data = self.data_mut();
        let period_frames = data.bufsize / 8;
        debug!("Setting period size to {} frames", period_frames);
        match hwp.set_period_size_near(period_frames, alsa::ValueOr::Nearest) {
            Ok(frames) => {
                data.period = frames;
            }
            Err(_) => {
                let alt_period_frames =
                    3 * 2.0f32.powi((period_frames as f32 / 2.0).log2().ceil() as i32) as Frames;
                debug!(
                    "Device did not accept a period size of {} frames, trying again with {}",
                    period_frames, alt_period_frames
                );
                data.period =
                    hwp.set_period_size_near(alt_period_frames, alsa::ValueOr::Nearest)?;
            }
        }
        Ok(())
    }

    // Update avail_min so set target for snd_pcm_wait.
    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()> {
        let data = self.data_mut();
        // maximum timing safety - headroom for one io_size only
        if data.io_size < data.period {
            warn!(
                "Trying to set avail_min to {}, must be larger than or equal to period of {}",
                data.io_size, data.period
            );
        } else if data.io_size > data.bufsize {
            let msg = format!("Trying to set avail_min to {}, must be smaller than or equal to device buffer size of {}",
                data.io_size, data.bufsize);
            error!("{}", msg);
            return Err(config::ConfigError::new(&msg).into());
        }
        data.avail_min = data.io_size;
        swp.set_avail_min(data.avail_min)?;
        Ok(())
    }

    fn update_io_size(&mut self, swp: &SwParams, io_size: Frames) -> Res<()> {
        let data = self.data_mut();
        data.io_size = io_size;
        // must update avail_min
        self.apply_avail_min(swp)?;
        // must update threshold
        self.apply_start_threshold(swp)?;
        Ok(())
    }

    fn frames_to_stall(&mut self) -> Frames {
        let data = self.data_mut();
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

impl DeviceBufferData {
    pub fn buffersize(&self) -> Frames {
        self.bufsize
    }
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
}
