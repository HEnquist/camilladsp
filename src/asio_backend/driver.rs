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
use std::sync::{Arc, LazyLock, Mutex};

use azo::Driver;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

use crate::config::ConfigError;

/// One loaded driver. The inner mutex serialises calls into a single instance, which
/// matters when capture and playback share one device in full-duplex mode.
type DriverHandle = Arc<Mutex<Driver>>;

static DRIVERS: LazyLock<Mutex<HashMap<String, DriverHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Initialise COM on the calling thread as a Single-Threaded Apartment.
///
/// ASIO drivers are COM objects that expect an STA. Instances are created with
/// [`Driver::new_unguarded`], so this module owns the COM lifecycle instead of azo. Every
/// thread that calls into a driver calls this first, and nothing here ever calls
/// `CoUninitialize`, so COM stays initialised for as long as the process runs. That is what
/// `new_unguarded` requires, and it means an instance can be created on one thread and used
/// or dropped on another, which is what full-duplex mode does.
///
/// Sharing an instance between threads is fine for ASIO specifically: the interfaces cannot
/// be marshalled at all (no method returns an `HRESULT`), so every call is a direct vtable
/// call on the calling thread, and hosts are expected to drive them from their own threads.
///
/// Safe to call more than once per thread; COM keeps a per-thread reference count.
pub(crate) fn com_init_this_thread() {
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    trace!("CoInitializeEx returned 0x{:08x}", hr.0);
}

/// Drivers that tolerate only one instance per process.
///
/// ASIO4ALL keeps the audio device open until `ASIOStop` is called or its DLL is unloaded,
/// which its author has confirmed on the ASIO4ALL forum. Releasing an instance that was
/// only initialised, never started, therefore leaves the device held, and creating the next
/// instance either deadlocks in `ASIOInit` or takes the process down. `ASIOStop` before the
/// release does not reliably help, it worked once in three attempts.
///
/// Matched case-insensitively as a substring: the same driver appears as `ASIO4ALL v2` in
/// the registry key and from `getDriverName`, but as `Asio4all v2` in the description that
/// device names are taken from.
const SINGLE_INSTANCE_DRIVERS: &[&str] = &["asio4all"];

/// Whether only one instance of this driver may be created per process.
///
/// Such drivers cannot be reloaded to apply a sample rate change, and cannot be probed for
/// capabilities, since probing loads an instance and releases it again.
pub(crate) fn is_single_instance_driver(devname: &str) -> bool {
    let lowercase = devname.to_lowercase();
    SINGLE_INSTANCE_DRIVERS
        .iter()
        .any(|quirky| lowercase.contains(quirky))
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
    f(&guard)
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

    // SAFETY: `com_init_this_thread` above initialised COM on this thread, and nothing in
    // this process ever calls `CoUninitialize`, so COM stays initialised for as long as the
    // instance lives. `DriverMetadata::create_instance` is not used because the COM guard it
    // returns is `!Send`, which would tie the instance to this thread.
    let driver = unsafe { Driver::new_unguarded(&metadata.clsid) }.map_err(|err| {
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
        .insert(name.to_string(), Arc::new(Mutex::new(driver)));
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
