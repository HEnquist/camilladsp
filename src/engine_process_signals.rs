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

use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;

use crate::engine::EXIT_FORCED;
use crate::{ControllerMessage, SHUTDOWN_REQUESTED};

/// Launch a thread that watches for sigint, sighup, etc.
pub fn launch_process_signals_thread(
    active_path_thread: Arc<Mutex<Option<String>>>,
    tx_command_thread: crossbeam_channel::Sender<ControllerMessage>,
    logger: flexi_logger::LoggerHandle,
) {
    if let Err(e) = thread::Builder::new()
        .name("signal-monitor".to_string())
        .spawn(move || {
            monitor_signals(active_path_thread, tx_command_thread, logger);
        })
    {
        error!("could not launch process signals monitor thread: {e:?}");
    }
}

#[cfg(not(windows))]
fn monitor_signals(
    active_path_thread: Arc<Mutex<Option<String>>>,
    tx_command_thread: crossbeam_channel::Sender<ControllerMessage>,
    logger: flexi_logger::LoggerHandle,
) {
    // these are internally gated to non-windows configs in signal-hook.
    // they're only used in this fn, so local import keeps the gate trim.
    use signal_hook::consts::{SIGHUP, SIGUSR1, TERM_SIGNALS};
    use signal_hook::iterator::{SignalsInfo, exfiltrator::SignalOnly};

    let mut sigs = vec![SIGHUP, SIGUSR1];
    sigs.extend(TERM_SIGNALS);
    let mut signals = SignalsInfo::<SignalOnly>::new(&sigs).unwrap();
    let mut exit_requested = false;
    for info in &mut signals {
        debug!("Received signal: {info}");
        match info {
            SIGHUP => {
                let path = (*active_path_thread.lock()).clone();
                if let Some(path) = path {
                    match crate::config::load_validate_config(path.as_str()) {
                        Ok(conf) => {
                            debug!("Config is valid");
                            if let Err(e) = tx_command_thread
                                .try_send(ControllerMessage::ConfigChanged(Box::new(conf)))
                            {
                                error!("Error sending reload message: {e}");
                            }
                        }
                        Err(err) => {
                            error!("Config error during reload: {err}");
                        }
                    };
                } else {
                    error!("Config path not specified, cannot reload");
                }
            }
            SIGUSR1 => {
                if let Err(e) = tx_command_thread.try_send(ControllerMessage::Stop) {
                    error!("Error sending stop message: {e}");
                }
            }
            _ => {
                if exit_requested {
                    warn!("Forcing a shutdown");
                    logger.flush();
                    std::process::exit(EXIT_FORCED);
                }
                info!("Shutting down");
                exit_requested = true;
                SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                    error!("Error sending exit message: {e}");
                }
            }
        };
    }
}

#[cfg(windows)]
fn monitor_signals(
    _active_path_thread: Arc<Mutex<Option<String>>>,
    tx_command_thread: crossbeam_channel::Sender<ControllerMessage>,
    logger: flexi_logger::LoggerHandle,
) {
    // On windows we don't have signal_hook::iterator, so we just poll for signal...
    const DELAY: std::time::Duration = std::time::Duration::from_millis(100);
    let signal_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&signal_exit)).unwrap();
    let mut exit_requested = false;
    loop {
        if signal_exit.load(std::sync::atomic::Ordering::Relaxed) {
            signal_exit.store(false, std::sync::atomic::Ordering::Relaxed);
            if exit_requested {
                warn!("Forcing a shutdown");
                logger.flush();
                std::process::exit(EXIT_FORCED);
            }
            info!("Shutting down");
            exit_requested = true;
            SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Err(e) = tx_command_thread.try_send(ControllerMessage::Exit) {
                error!("Error sending exit message: {e}");
            }
        }
        thread::sleep(DELAY);
    }
}
