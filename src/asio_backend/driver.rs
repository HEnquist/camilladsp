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

// Registry of loaded ASIO drivers, keyed by device name.
//
// The Steinberg SDK keeps a process-wide singleton (an `AsioDrivers` instance plus the
// global `theAsioDriver` pointer) and exposes every driver call as a free function, which
// is why only one driver could be open at a time. The azo crate instead hands out an owned
// `Driver` value per instance, so this module keeps one entry per device name and capture
// and playback can use different devices.
//
// A registry is still needed rather than passing handles around: ASIO callbacks are bare
// function pointers with no user-data argument, and in full-duplex mode the capture and
// playback threads share a single instance.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, LazyLock, Mutex};

use azo::Driver;
use azo::utils::com::InitGuard;

use crate::config::ConfigError;

/// Wrapper that lets a driver handle be shared between the capture and playback threads.
///
/// SAFETY: `InitGuard<Driver>` is deliberately `!Send`, because its `Drop` calls
/// `CoUninitialize`, which belongs on the thread that called `CoInitializeEx`. We override
/// that because capture and playback share one driver instance across two threads, and the
/// full-duplex teardown means either of them may be the one to drop it.
///
/// This is sound here for two reasons. ASIO interfaces cannot be marshalled at all (no
/// method returns an `HRESULT`), so every call is a direct vtable call on the calling
/// thread and hosts are expected to drive them from their own threads. And each device
/// thread calls [`com_init_this_thread`] before touching a driver, so whichever thread
/// drops the guard has a matching `CoInitializeEx` of its own to balance.
///
/// The drop must stay a real drop. Recreating the instance is what makes a sample rate
/// change take effect, and that depends on the `CoUninitialize` this guard performs.
struct SharedDriver(InitGuard<Driver>);

// SAFETY: see the note on `SharedDriver`.
unsafe impl Send for SharedDriver {}

/// One loaded driver. The inner mutex serialises calls into a single instance, which
/// matters when capture and playback share one device in full-duplex mode.
type DriverHandle = Arc<Mutex<SharedDriver>>;

static DRIVERS: LazyLock<Mutex<HashMap<String, DriverHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ASIO drivers are COM objects that expect a Single-Threaded Apartment. azo calls
// CoInitializeEx when a driver instance is created, but only on the creating thread. The
// capture and playback threads call into instances they did not necessarily create, and
// either of them may be the one that drops an instance (which calls CoUninitialize), so
// each initialises COM for itself.
unsafe extern "system" {
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
}
const COINIT_APARTMENTTHREADED: u32 = 0x2;

/// Initialise COM on the calling thread as a Single-Threaded Apartment.
///
/// Safe to call more than once per thread; COM keeps a per-thread reference count.
pub(crate) fn com_init_this_thread() {
    let hr = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    trace!("CoInitializeEx returned 0x{hr:08x}");
}

/// Look up a loaded driver by device name.
fn lookup(devname: &str) -> Option<DriverHandle> {
    DRIVERS.lock().unwrap().get(devname).cloned()
}

/// Run `f` with the driver loaded for `devname`.
///
/// The registry lock is released before `f` runs, so calls to different devices do not
/// block each other. The per-driver lock is held for the duration of the call, so `f` must
/// not call back into this module for the same device.
pub(crate) fn with_driver<T>(
    devname: &str,
    f: impl FnOnce(&Driver) -> Result<T, ConfigError>,
) -> Result<T, ConfigError> {
    let Some(handle) = lookup(devname) else {
        return Err(ConfigError::new(&format!(
            "No ASIO driver is loaded for device '{devname}'"
        )));
    };
    let guard = handle.lock().unwrap();
    f(&guard.0)
}

/// Whether a driver is currently loaded for `devname`.
pub(crate) fn driver_is_loaded(devname: &str) -> bool {
    lookup(devname).is_some()
}

/// List the names of all registered ASIO drivers.
///
/// The names are the `description` values under `HKLM\SOFTWARE\ASIO`, which is the same
/// string the SDK's driver list reports, so device names in existing configs stay valid.
pub fn list_device_names() -> Vec<String> {
    match azo::get_drivers() {
        Ok(drivers) => drivers
            .iter()
            .map(|driver| driver.description.to_string())
            .collect(),
        Err(err) => {
            debug!("Failed to enumerate ASIO drivers: {err}");
            Vec::new()
        }
    }
}

/// Load an ASIO driver by name and initialise it.
///
/// Any instance previously loaded for the same name is released first. Drivers loaded for
/// other device names are left alone.
pub fn load_driver_by_name(name: &str) -> Result<(), ConfigError> {
    trace!("load_driver_by_name: loading '{name}'");
    teardown_asio_driver(name);
    com_init_this_thread();

    let drivers = azo::get_drivers()
        .map_err(|err| ConfigError::new(&format!("Failed to enumerate ASIO drivers: {err}")))?;
    let metadata = drivers
        .into_iter()
        .find(|driver| driver.description == name)
        .ok_or_else(|| ConfigError::new(&format!("No ASIO driver named '{name}' is registered")))?;

    let driver = metadata.create_instance().map_err(|err| {
        ConfigError::new(&format!(
            "Failed to create an instance of ASIO driver '{name}': {err}"
        ))
    })?;

    if !driver.init(None) {
        let detail = driver.last_error();
        return Err(ConfigError::new(&format!(
            "Failed to initialise ASIO driver '{name}': {}",
            detail.to_string_lossy()
        )));
    }

    debug!(
        "Loaded ASIO driver '{}', version {}.",
        driver.name().to_string_lossy(),
        driver.version()
    );
    DRIVERS
        .lock()
        .unwrap()
        .insert(name.to_string(), Arc::new(Mutex::new(SharedDriver(driver))));
    trace!("load_driver_by_name: '{name}' loaded and initialised");
    Ok(())
}

/// Release the driver loaded for `devname`, if any.
///
/// Dropping the handle releases the COM object. Safe to call when nothing is loaded.
pub(crate) fn teardown_asio_driver(devname: &str) {
    // Take the handle out under the registry lock, then let it drop after the guard is
    // released, so the COM release does not run while the registry is locked.
    let previous = DRIVERS.lock().unwrap().remove(devname);
    if previous.is_some() {
        trace!("teardown_asio_driver: releasing the instance for '{devname}'");
    } else {
        trace!("teardown_asio_driver: no driver loaded for '{devname}', nothing to do");
    }
}
