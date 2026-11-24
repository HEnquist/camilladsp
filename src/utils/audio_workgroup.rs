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

//! Audio workgroups (macOS 11 and later).
//!
//! CoreAudio runs the IO thread of each device in an *audio workgroup*, an `os_workgroup_t` that
//! tells the scheduler which threads work against the deadline of that device. The capture,
//! processing and playback threads do audio work for the same deadline, but are not part of the
//! workgroup unless they join it. Outside of it the scheduler may move them to efficiency cores,
//! and on Apple silicon that makes the processing several times slower.
//!
//! The CoreAudio backend publishes the workgroup of its device before the startup barrier, and the
//! threads join it after the barrier. The playback device's workgroup wins when both devices are
//! CoreAudio, since playback sets the deadline the whole chain works towards.
//!
//! `os_workgroup_join` and `os_workgroup_leave` only exist from macOS 11, while the Intel binary
//! also runs on older versions. They are therefore looked up at runtime rather than linked, and
//! where they are missing, joining does nothing. On other platforms everything here is a no-op.

#[cfg(target_os = "macos")]
pub use macos::{Publication, Workgroup, publish};
#[cfg(target_os = "macos")]
use macos::{join_published, leave_joined};

/// Join the calling thread to the published audio workgroup, if there is one.
///
/// Call it after the thread has been promoted to real time, and after the startup barrier, so that
/// the devices have had a chance to publish. The membership is held by the thread, and is left by
/// `leave_current_thread`, or at the latest when the thread exits.
pub fn join_current_thread(name: &str) {
    #[cfg(target_os = "macos")]
    join_published(name);
    #[cfg(not(target_os = "macos"))]
    let _ = name;
}

/// Leave the audio workgroup the calling thread joined, if any.
pub fn leave_current_thread() {
    #[cfg(target_os = "macos")]
    leave_joined();
}

#[cfg(target_os = "macos")]
mod macos {
    use parking_lot::Mutex;
    use std::cell::RefCell;
    use std::ffi::{c_int, c_void};
    use std::sync::OnceLock;

    unsafe extern "C" {
        // Part of libSystem since macOS 10.10, so these can be linked normally.
        fn os_retain(object: *mut c_void) -> *mut c_void;
        fn os_release(object: *mut c_void);
    }

    /// `os_workgroup_join_token_s` from `<os/workgroup_base.h>`, for 64-bit targets.
    #[repr(C)]
    struct JoinToken {
        sig: u32,
        opaque: [u8; 36],
    }

    type JoinFn = unsafe extern "C" fn(*mut c_void, *mut JoinToken) -> c_int;
    type LeaveFn = unsafe extern "C" fn(*mut c_void, *mut JoinToken);

    struct Api {
        join: JoinFn,
        leave: LeaveFn,
    }

    /// Look up the join and leave functions, once. `None` before macOS 11.
    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            let join = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_join".as_ptr()) };
            let leave = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"os_workgroup_leave".as_ptr()) };
            if join.is_null() || leave.is_null() {
                debug!("Audio workgroups are not available, they need macOS 11 or later.");
                return None;
            }
            Some(Api {
                join: unsafe { std::mem::transmute::<*mut c_void, JoinFn>(join) },
                leave: unsafe { std::mem::transmute::<*mut c_void, LeaveFn>(leave) },
            })
        })
        .as_ref()
    }

    /// An owned reference to an `os_workgroup_t`, released on drop.
    pub struct Workgroup(*mut c_void);

    // OS objects are reference counted with atomic operations, and the workgroup functions may be
    // called from any thread.
    unsafe impl Send for Workgroup {}
    unsafe impl Sync for Workgroup {}

    impl Workgroup {
        /// Take ownership of a workgroup reference that the caller is responsible for releasing,
        /// such as the one returned by `kAudioDevicePropertyIOThreadOSWorkgroup`.
        ///
        /// # Safety
        /// `ptr` must be a valid, retained `os_workgroup_t`, or null.
        pub unsafe fn from_retained(ptr: *mut c_void) -> Option<Self> {
            if ptr.is_null() { None } else { Some(Self(ptr)) }
        }
    }

    impl Clone for Workgroup {
        fn clone(&self) -> Self {
            Self(unsafe { os_retain(self.0) })
        }
    }

    impl Drop for Workgroup {
        fn drop(&mut self) {
            unsafe { os_release(self.0) };
        }
    }

    struct Published {
        workgroup: Workgroup,
        from_playback: bool,
    }

    static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);

    /// Publish the workgroup of a device for the threads to join.
    ///
    /// A playback workgroup replaces whatever is there, a capture workgroup is only used when
    /// nothing has been published yet. The publication is withdrawn when the returned guard is
    /// dropped, so that it can't outlive the device and be picked up by the next session.
    /// Threads that already joined keep their membership.
    pub fn publish(workgroup: Workgroup, from_playback: bool) -> Publication {
        // Resolve the functions here, rather than on a real-time thread when joining.
        if api().is_some() {
            let mut published = PUBLISHED.lock();
            if from_playback || published.is_none() {
                *published = Some(Published {
                    workgroup,
                    from_playback,
                });
            }
        }
        Publication
    }

    /// Withdraws the published workgroup when dropped, see `publish`.
    pub struct Publication;

    impl Drop for Publication {
        fn drop(&mut self) {
            PUBLISHED.lock().take();
        }
    }

    struct Membership {
        workgroup: Workgroup,
        token: JoinToken,
    }

    impl Drop for Membership {
        fn drop(&mut self) {
            if let Some(api) = api() {
                unsafe { (api.leave)(self.workgroup.0, &mut self.token) };
            }
        }
    }

    thread_local! {
        static MEMBERSHIP: RefCell<Option<Membership>> = const { RefCell::new(None) };
    }

    pub fn join_published(name: &str) {
        let Some(api) = api() else {
            return;
        };
        let Some((workgroup, from_playback)) = PUBLISHED
            .lock()
            .as_ref()
            .map(|p| (p.workgroup.clone(), p.from_playback))
        else {
            debug!("{name} thread: no audio workgroup to join.");
            return;
        };
        let device = if from_playback { "playback" } else { "capture" };
        MEMBERSHIP.with_borrow_mut(|membership| {
            // A thread can only be in one workgroup that does not nest with the new one.
            membership.take();
            let mut token = JoinToken {
                sig: 0,
                opaque: [0; 36],
            };
            let result = unsafe { (api.join)(workgroup.0, &mut token) };
            if result == 0 {
                debug!("{name} thread joined the audio workgroup of the {device} device.");
                *membership = Some(Membership { workgroup, token });
            } else {
                warn!(
                    "{name} thread could not join the audio workgroup of the {device} device, error: {}.",
                    std::io::Error::from_raw_os_error(result)
                );
            }
        });
    }

    pub fn leave_joined() {
        MEMBERSHIP.with_borrow_mut(|membership| membership.take());
    }
}
