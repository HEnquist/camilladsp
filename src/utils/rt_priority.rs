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

//! Thin wrapper around real-time thread priority promotion.
//!
//! All the actual work is done by `audio_thread_priority`, which picks the right mechanism per
//! platform: the mach time-constraint policy on macOS, MMCSS "Pro Audio" on Windows, and on Linux
//! either rtkit over D-Bus or a direct `pthread_setschedparam(SCHED_FIFO)` call.
//!
//! On Linux the choice is made by the crate's `with_dbus` feature, enabled here through the `rtkit`
//! feature. rtkit is the right choice for unprivileged desktop processes, and it ships as a
//! dependency of PipeWire, so the `pipewire-backend` feature enables it. The plain ALSA-only build
//! targets minimal/headless systems that often have no D-Bus at all, and gets the native path
//! instead. That one needs no D-Bus and no running service, and works wherever the process may
//! request real-time scheduling (running as root, holding `CAP_SYS_NICE`, or with an `RLIMIT_RTPRIO`
//! limit configured, e.g. via `/etc/security/limits.conf`).
//!
//! The only thing added on top of the crate is the `CAMILLADSP_RT_PRIORITY` environment variable,
//! which overrides the priority the native Linux path requests.

pub use audio_thread_priority::{RtPriorityHandle, demote_current_thread_from_real_time};

use audio_thread_priority::AudioThreadPriorityError;

/// Promote the calling thread to real-time priority.
///
/// The buffer size and sample rate are only used by the Linux rtkit path, which derives an
/// `RLIMIT_RTTIME` budget from them. Pass a non-zero sample rate, since the crate rejects zero.
pub fn promote_current_thread_to_real_time(
    audio_buffer_frames: u32,
    audio_samplerate_hz: u32,
) -> Result<RtPriorityHandle, AudioThreadPriorityError> {
    #[cfg(all(target_os = "linux", not(feature = "rtkit")))]
    priority_override::apply();
    audio_thread_priority::promote_current_thread_to_real_time(
        audio_buffer_frames,
        audio_samplerate_hz,
    )
}

/// Applies the `CAMILLADSP_RT_PRIORITY` override to the native Linux path. Not available for the
/// other paths: rtkit picks the priority itself, and macOS and Windows have no priority to set.
#[cfg(all(target_os = "linux", not(feature = "rtkit")))]
mod priority_override {
    use std::sync::Once;

    /// Environment variable used to override the real-time priority. Accepts an integer 1-99. Higher
    /// values preempt more work but must stay below the audio interface's IRQ thread, or they starve
    /// the very threads that deliver audio.
    const RT_PRIORITY_ENV: &str = "CAMILLADSP_RT_PRIORITY";

    static APPLIED: Once = Once::new();

    /// Read the environment variable and hand the priority to `audio_thread_priority`, once per
    /// process. Without an override the crate uses its default priority of 10, the same value the
    /// rtkit path requests.
    pub fn apply() {
        APPLIED.call_once(|| {
            let Ok(value) = std::env::var(RT_PRIORITY_ENV) else {
                return;
            };
            match value.trim().parse::<u8>() {
                Ok(priority) if (1..=99).contains(&priority) => {
                    audio_thread_priority::set_rt_priority(Some(priority))
                }
                _ => warn!(
                    "Ignoring invalid {RT_PRIORITY_ENV}=\"{value}\", \
                     expected an integer 1-99. Using the default priority."
                ),
            }
        });
    }
}
