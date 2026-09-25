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
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ProcessingParameters;

/// Persistent state that is saved to and loaded from the state file across restarts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Path to the last active configuration file, if any.
    pub config_path: Option<String>,
    /// Mute status for each of the [`ProcessingParameters::NUM_FADERS`] faders.
    pub mute: [bool; 5],
    /// Volume (dB) for each of the [`ProcessingParameters::NUM_FADERS`] faders.
    pub volume: [f32; 5],
}

/// Load a [`State`] from `filename`, returning `None` and logging a warning on any error.
pub fn load_state(filename: &str) -> Option<State> {
    let file = match File::open(filename) {
        Ok(f) => f,
        Err(err) => {
            warn!("Could not read statefile '{filename}'. Error: {err}");
            return None;
        }
    };
    let mut buffered_reader = BufReader::new(file);
    let mut contents = String::new();
    let _number_of_bytes: usize = match buffered_reader.read_to_string(&mut contents) {
        Ok(number_of_bytes) => number_of_bytes,
        Err(err) => {
            warn!("Could not read statefile '{filename}'. Error: {err}");
            return None;
        }
    };
    let state: State = match yaml_serde::from_str(&contents) {
        Ok(st) => st,
        Err(err) => {
            warn!("Invalid statefile, ignoring! Error:\n{err}");
            return None;
        }
    };
    Some(state)
}

/// Build a [`State`] from the current parameters and save it to `filename`,
/// clearing the `unsaved_changes` flag on success.
pub fn save_state(
    filename: &str,
    config_path: &Arc<Mutex<Option<String>>>,
    params: &ProcessingParameters,
    unsaved_changes: &Arc<AtomicBool>,
) {
    let state = State {
        config_path: config_path.lock().as_ref().map(|s| s.to_string()),
        volume: params.volumes(),
        mute: params.mutes(),
    };
    if save_state_to_file(filename, &state) {
        unsaved_changes.store(false, Ordering::Relaxed);
    }
}

/// Serialize `state` to `filename`, returning `true` on success.
pub fn save_state_to_file(filename: &str, state: &State) -> bool {
    debug!("Saving state to {filename}");
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(filename)
    {
        Ok(f) => {
            if let Err(writeerr) = yaml_serde::to_writer(&f, &state) {
                error!("Unable to write to statefile '{filename}', error: {writeerr}");
                return false;
            }
            if let Err(syncerr) = &f.sync_all() {
                error!("Unable to commit statefile '{filename}' data to disk, error: {syncerr}");
                return false;
            }
            true
        }
        Err(openerr) => {
            error!("Unable to open statefile {filename}, error: {openerr}");
            false
        }
    }
}
