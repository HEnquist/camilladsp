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

use std::time::Instant;

use crate::PrcFmt;
use crate::utils::decibels::linear_to_db;

/// Main container of audio data
pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub maxval: PrcFmt,
    pub minval: PrcFmt,
    pub timestamp: Instant,
    pub valid_frames: usize,
    pub waveforms: Vec<Vec<PrcFmt>>,
}

/// Container for RMS and peak values of a chunk
pub struct ChunkStats {
    pub rms: Vec<PrcFmt>,
    pub peak: Vec<PrcFmt>,
}

impl ChunkStats {
    pub fn rms_db(&self) -> Vec<f32> {
        self.rms
            .iter()
            .map(|val| linear_to_db(*val as f32))
            .collect()
    }

    pub fn rms_linear(&self) -> Vec<f32> {
        self.rms.iter().map(|val| *val as f32).collect()
    }

    pub fn peak_db(&self) -> Vec<f32> {
        self.peak
            .iter()
            .map(|val| linear_to_db(*val as f32))
            .collect()
    }

    pub fn peak_linear(&self) -> Vec<f32> {
        self.peak.iter().map(|val| *val as f32).collect()
    }
}

impl AudioChunk {
    pub fn new(
        waveforms: Vec<Vec<PrcFmt>>,
        maxval: PrcFmt,
        minval: PrcFmt,
        frames: usize,
        valid_frames: usize,
    ) -> Self {
        let timestamp = Instant::now();
        let channels = waveforms.len();
        AudioChunk {
            frames,
            channels,
            maxval,
            minval,
            timestamp,
            valid_frames,
            waveforms,
        }
    }

    pub fn from(chunk: &AudioChunk, waveforms: Vec<Vec<PrcFmt>>) -> Self {
        let timestamp = chunk.timestamp;
        let maxval = chunk.maxval;
        let minval = chunk.minval;
        let frames = chunk.frames;
        let valid_frames = chunk.valid_frames;
        let channels = waveforms.len();
        AudioChunk {
            frames,
            channels,
            maxval,
            minval,
            timestamp,
            valid_frames,
            waveforms,
        }
    }

    pub fn stats(&self) -> ChunkStats {
        let rms_peak: Vec<(PrcFmt, PrcFmt)> =
            self.waveforms.iter().map(|wf| rms_and_peak(wf)).collect();
        let rms: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.0).collect();
        let peak: Vec<PrcFmt> = rms_peak.iter().map(|rp| rp.1).collect();
        ChunkStats { rms, peak }
    }

    pub fn update_stats(&self, stats: &mut ChunkStats) {
        stats.rms.resize(self.channels, 0.0);
        stats.peak.resize(self.channels, 0.0);
        for (wf, (peakval, rmsval)) in self
            .waveforms
            .iter()
            .zip(stats.peak.iter_mut().zip(stats.rms.iter_mut()))
        {
            let (rms, peak) = rms_and_peak(wf);
            *peakval = peak;
            *rmsval = rms;
        }
        xtrace!(
            "Stats: rms {:?}, peak {:?}",
            stats.rms_db(),
            stats.peak_db()
        );
    }

    pub fn update_channel_mask(&self, mask: &mut [bool]) {
        mask.iter_mut()
            .zip(self.waveforms.iter())
            .for_each(|(m, w)| *m = !w.is_empty());
    }
}

/// Get RMS and peak value of a vector
pub fn rms_and_peak(data: &[PrcFmt]) -> (PrcFmt, PrcFmt) {
    if !data.is_empty() {
        let (squaresum, peakval) = data.iter().fold((0.0, 0.0), |(sqsum, peak), value| {
            let newpeak = if peak > value.abs() {
                peak
            } else {
                value.abs()
            };
            (sqsum + *value * *value, newpeak)
        });
        ((squaresum / data.len() as PrcFmt).sqrt(), peakval)
    } else {
        (0.0, 0.0)
    }
}
