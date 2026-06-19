use alsa::pcm::{Frames, HwParams, SwParams};

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
                "Minimum period is {min_period} frames, buffer size is minimum {frames_needed} frames"
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
                "Minimum period is {min_period} frames, alternate buffer size is minimum {frames_needed} frames"
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
        debug!("Setting buffer size to {buffer_frames} frames");
        match hwp.set_buffer_size_near(buffer_frames) {
            Ok(frames) => {
                data.bufsize = frames;
            }
            Err(_) => {
                debug!(
                    "Device did not accept a buffer size of {buffer_frames} frames, trying again with {alt_buffer_frames}"
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
        debug!("Setting period size to {period_frames} frames");
        match hwp.set_period_size_near(period_frames, alsa::ValueOr::Nearest) {
            Ok(frames) => {
                data.period = frames;
            }
            Err(_) => {
                let alt_period_frames =
                    3 * 2.0f32.powi((period_frames as f32 / 2.0).log2().ceil() as i32) as Frames;
                debug!(
                    "Device did not accept a period size of {period_frames} frames, trying again with {alt_period_frames}"
                );
                data.period =
                    hwp.set_period_size_near(alt_period_frames, alsa::ValueOr::Nearest)?;
            }
        }
        debug!("Device is using a period size of {} frames", data.period);
        Ok(())
    }

    /// Update avail_min so set target for snd_pcm_wait.
    /// Use a fixed period-sized avail_min in threaded mode so device I/O cadence is independent of chunk handoff size.
    fn apply_avail_min(&mut self, swp: &SwParams) -> Res<()>;

    fn update_io_size(&mut self, swp: &SwParams, io_size: Frames) -> Res<()> {
        let data = self.data_mut();
        data.io_size = io_size;
        // must update avail_min
        self.apply_avail_min(swp)?;
        // must update threshold
        self.apply_start_threshold(swp)?;
        Ok(())
    }

    fn frames_to_stall(&self) -> Frames {
        let data = self.data();
        // +1 to make sure the device really stalls
        data.bufsize - data.avail_min + 1
    }

    fn current_delay(&self, avail: Frames) -> Frames;
}

#[derive(Debug)]
pub struct DeviceBufferData {
    pub(crate) bufsize: Frames,
    pub(crate) period: Frames,
    pub(crate) threshold: Frames,
    pub(crate) avail_min: Frames,
    pub(crate) io_size: Frames, /* size of read/write block */
    pub(crate) chunksize: Frames,
    pub(crate) resampling_ratio: f32,
}

impl DeviceBufferData {
    pub fn buffersize(&self) -> Frames {
        self.bufsize
    }

    pub fn periodsize(&self) -> Frames {
        self.period
    }
}
