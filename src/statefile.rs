//use crate::config::Configuration;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::ProcessingParameters;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub config_path: Option<String>,
    pub mute: [bool; 5],
    pub volume: [f32; 5],
}

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
    let state: State = match serde_yml::from_str(&contents) {
        Ok(st) => st,
        Err(err) => {
            warn!("Invalid statefile, ignoring! Error:\n{err}");
            return None;
        }
    };
    Some(state)
}

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

pub fn save_state_to_file(filename: &str, state: &State) -> bool {
    debug!("Saving state to {}", filename);
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(filename)
    {
        Ok(f) => {
            if let Err(writeerr) = serde_yml::to_writer(&f, &state) {
                error!(
                    "Unable to write to statefile '{}', error: {}",
                    filename, writeerr
                );
                return false;
            }
            if let Err(syncerr) = &f.sync_all() {
                error!(
                    "Unable to commit statefile '{}' data to disk, error: {}",
                    filename, syncerr
                );
                return false;
            }
            true
        }
        Err(openerr) => {
            error!("Unable to open statefile {}, error: {}", filename, openerr);
            false
        }
    }
}
