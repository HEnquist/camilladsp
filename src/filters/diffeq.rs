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
use crate::filters::Filter;

// Sample format
//type SmpFmt = i16;
use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;

#[derive(Clone, Debug)]
pub struct DiffEq {
    pub x: Vec<CamillaFloat>,
    pub y: Vec<CamillaFloat>,
    pub a: Vec<CamillaFloat>,
    pub a_len: usize,
    pub b: Vec<CamillaFloat>,
    pub b_len: usize,
    pub idx_x: usize,
    pub idx_y: usize,
    pub name: String,
}

impl DiffEq {
    pub fn new(name: &str, a_in: Vec<f64>, b_in: Vec<f64>) -> Self {
        let name = name.to_string();

        // Coefficients arrive from the config in f64 and cross into the
        // processing precision once, here.
        let a: Vec<CamillaFloat> = if a_in.is_empty() {
            vec![1.0]
        } else {
            a_in.into_iter().map(|v| v.to_camilla_float()).collect()
        };

        let b: Vec<CamillaFloat> = if b_in.is_empty() {
            vec![1.0]
        } else {
            b_in.into_iter().map(|v| v.to_camilla_float()).collect()
        };

        let x = vec![0.0; b.len()];
        let y = vec![0.0; a.len()];

        let a_len = a.len();
        let b_len = b.len();
        DiffEq {
            x,
            y,
            a,
            a_len,
            b,
            b_len,
            idx_x: 0,
            idx_y: 0,
            name,
        }
    }

    pub fn from_config(name: &str, conf: config::DiffEqParameters) -> Self {
        let a = conf.a();
        let b = conf.b();
        DiffEq::new(name, a, b)
    }

    /// Process a single sample
    fn process_single(&mut self, input: CamillaFloat) -> CamillaFloat {
        let mut out = 0.0;
        self.idx_x = (self.idx_x + 1) % self.b_len;
        self.idx_y = (self.idx_y + 1) % self.a_len;
        self.x[self.idx_x] = input;
        for n in 0..self.b_len {
            let n_idx = (self.idx_x + self.b_len - n) % self.b_len;
            out += self.b[n] * self.x[n_idx];
        }
        for p in 1..self.a_len {
            let p_idx = (self.idx_y + self.a_len - p) % self.a_len;
            out -= self.a[p] * self.y[p_idx];
        }
        self.y[self.idx_y] = out;
        out
    }

    /// Flush stored subnormal numbers to zero.
    fn flush_subnormals(&mut self) {
        for (n, x) in self.x.iter_mut().enumerate() {
            if x.is_subnormal() {
                trace!(
                    "DiffEq filter '{}', flushing subnormal x at index {}",
                    self.name, n
                );
                *x = 0.0;
            }
        }
        for (n, y) in self.y.iter_mut().enumerate() {
            if y.is_subnormal() {
                trace!(
                    "DiffEq filter '{}', flushing subnormal y at index {}",
                    self.name, n
                );
                *y = 0.0;
            }
        }
    }
}

impl Filter for DiffEq {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        for item in waveform.iter_mut() {
            *item = self.process_single(*item);
        }
        self.flush_subnormals();
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::DiffEq {
            parameters: conf, ..
        } = conf
        {
            *self = DiffEq::from_config(&self.name, conf);
        } else {
            // This should never happen unless there is a bug somewhere else
            unreachable!("Invalid config change!");
        }
    }
}

pub fn validate_config(_parameters: &config::DiffEqParameters) -> Res<()> {
    // TODO add check for stability
    Ok(())
}

#[cfg(test)]
#[cfg_attr(camillafloat_f32, allow(clippy::excessive_precision))]
mod tests {
    use crate::CamillaFloat;
    use crate::filters::Filter;
    use crate::filters::diffeq::DiffEq;

    fn is_close(left: CamillaFloat, right: CamillaFloat, maxdiff: CamillaFloat) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(
        left: Vec<CamillaFloat>,
        right: Vec<CamillaFloat>,
        maxdiff: CamillaFloat,
    ) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn check_result() {
        let mut filter = DiffEq::new(
            "test",
            vec![1.0, -0.1462978543780541, 0.005350765548905586],
            vec![0.21476322779271284, 0.4295264555854257, 0.21476322779271284],
        );
        let mut wave = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let expected = vec![0.215, 0.461, 0.281, 0.039, 0.004, 0.0, 0.0, 0.0];
        filter.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, expected, 1e-3));
    }
}
