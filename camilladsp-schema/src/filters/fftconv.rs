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

use crate::config;
use crate::config::{Issue, Issues, issue_path};
use crate::filters;
use std::collections::HashMap;
use std::path::Path;

// Sample format
use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;

/// The impulse responses that validating a configuration read, keyed by filter
/// name.
///
/// Validating a config already reads every coefficient file its pipeline refers
/// to, since a file that cannot be read or that holds something other than
/// finite numbers is exactly what validation is there to reject. Keeping what
/// it read means no one has to read it a second time. The cache travels with
/// the configuration it was built from, so the two can never be mismatched, and
/// by the time the change reaches the processing thread there is no file system
/// left in the path at all.
#[derive(Default, Clone)]
pub struct ImpulseCache {
    entries: HashMap<String, Vec<CamillaFloat>>,
}

impl ImpulseCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&Vec<CamillaFloat>> {
        self.entries.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn insert(&mut self, name: &str, coeffs: Vec<CamillaFloat>) {
        self.entries.insert(name.to_string(), coeffs);
    }

    /// How many distinct filters the cache holds an impulse response for.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Read the impulse response a configuration points at.
pub fn coeffs_from_config(conf: &config::ConvParameters) -> Res<Vec<CamillaFloat>> {
    match conf {
        config::ConvParameters::Values { values } => {
            // Coefficients from the config are f64; file and wav readers
            // already deliver the processing precision.
            Ok(values.iter().map(|v| v.to_camilla_float()).collect())
        }
        config::ConvParameters::Raw(params) => filters::read_coeff_file(
            &params.filename,
            &params.format(),
            params.read_bytes_lines(),
            params.skip_bytes_lines(),
        ),
        config::ConvParameters::Wav(params) => {
            filters::read_wav(&params.filename, params.channel())
        }
        config::ConvParameters::Dummy { length } => {
            let mut values = vec![0.0; length.get()];
            values[0] = 1.0;
            Ok(values)
        }
    }
}

/// Validate a FFT convolution config, keeping the impulse response it read.
///
/// Validating means reading the coefficients, so the read is where the answer
/// comes from either way. Handing them to `impulses` is what stops them being
/// read again, both by a later pipeline step naming the same filter and by the
/// pass that eventually builds it.
///
/// Issue paths are relative to the parameters, and point at the file name for
/// coefficients read from a file. A file that does not exist is reported as
/// [`IssueKind::MissingFile`](config::IssueKind::MissingFile).
pub fn validate_config(
    name: &str,
    conf: &config::ConvParameters,
    impulses: &mut ImpulseCache,
) -> Result<(), Issues> {
    // Filter names are unique within a config, so a name already in the cache
    // was read from these same parameters and is already known to be valid.
    if impulses.contains(name) {
        return Ok(());
    }
    let (field, filename) = match conf {
        config::ConvParameters::Raw(params) => ("filename", Some(params.filename.as_str())),
        config::ConvParameters::Wav(params) => ("filename", Some(params.filename.as_str())),
        config::ConvParameters::Values { .. } => ("values", None),
        config::ConvParameters::Dummy { .. } => ("length", None),
    };
    let coeffs = match coeffs_from_config(conf) {
        Ok(coeffs) => coeffs,
        Err(err) => {
            let missing = filename
                .is_some_and(|filename| matches!(Path::new(filename).try_exists(), Ok(false)));
            let issue = if missing {
                Issue::missing_file(issue_path![field], err.to_string())
            } else {
                Issue::invalid(issue_path![field], err.to_string())
            };
            return Err(issue.into());
        }
    };
    if coeffs.is_empty() {
        return Err(Issue::invalid(issue_path![field], "Conv coefficients are empty").into());
    }
    if let Err(err) = config::check_all_finite("coefficients", &coeffs) {
        return Err(Issue::invalid(issue_path![field], err.to_string()).into());
    }
    impulses.insert(name, coeffs);
    Ok(())
}
