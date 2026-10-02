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
use crate::config::{FiniteF32, NotFinite};

/// Persistent state that is saved to and loaded from the state file across restarts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Path to the last active configuration file, if any.
    pub config_path: Option<String>,
    /// Mute status for each of the [`ProcessingParameters::NUM_FADERS`] faders.
    pub mute: [bool; 5],
    /// Volume (dB) for each of the [`ProcessingParameters::NUM_FADERS`] faders.
    ///
    /// A hand-edited file can hold `.nan` or `.inf`, which would reach the volume filter,
    /// so such a file fails to load like any other malformed one.
    pub volume: [FiniteF32; 5],
}

impl State {
    /// Build a [`State`] from runtime values, rejecting a non-finite volume.
    pub fn new(
        config_path: Option<String>,
        mute: [bool; 5],
        volume: [f32; 5],
    ) -> Result<Self, NotFinite> {
        let finite = volume.map(FiniteF32::new);
        if finite.iter().any(Option::is_none) {
            return Err(NotFinite);
        }
        Ok(State {
            config_path,
            mute,
            volume: finite.map(Option::unwrap),
        })
    }

    /// The fader volumes (dB) as plain numbers.
    pub fn volumes(&self) -> [f32; 5] {
        self.volume.map(f32::from)
    }
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
    let state = match State::new(
        config_path.lock().as_ref().map(|s| s.to_string()),
        params.mutes(),
        params.volumes(),
    ) {
        Ok(state) => state,
        Err(err) => {
            error!("Not saving state to '{filename}', error: {err}");
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn roundtrip_finite_state() {
        let state = State::new(
            Some("config.yml".to_string()),
            [false, true, false, false, false],
            [-10.0, 0.0, 5.5, -150.0, 50.0],
        )
        .unwrap();
        let yaml = yaml_serde::to_string(&state).unwrap();
        let loaded: State = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(loaded, state);
        assert_eq!(loaded.volumes(), [-10.0, 0.0, 5.5, -150.0, 50.0]);
    }

    #[test]
    fn reject_non_finite_volume() {
        for bad in [".nan", ".inf", "-.inf"] {
            let yaml = format!(
                "config_path: null\nmute: [false, false, false, false, false]\n\
                 volume: [{bad}, 0.0, 0.0, 0.0, 0.0]\n"
            );
            assert!(yaml_serde::from_str::<State>(&yaml).is_err(), "{bad}");
        }
        assert!(State::new(None, [false; 5], [0.0, f32::NAN, 0.0, 0.0, 0.0]).is_err());
        assert!(State::new(None, [false; 5], [0.0, 0.0, 0.0, 0.0, f32::INFINITY]).is_err());
    }
}
