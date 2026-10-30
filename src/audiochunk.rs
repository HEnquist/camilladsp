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

use crate::CamillaFloat;
use crate::ToF32;
use crate::utils::decibels::linear_to_db;

/// Main container of audio data
pub struct AudioChunk {
    pub frames: usize,
    pub channels: usize,
    pub maxval: CamillaFloat,
    pub minval: CamillaFloat,
    pub timestamp: Instant,
    pub valid_frames: usize,
    pub waveforms: Vec<Vec<CamillaFloat>>,
}

/// Container for RMS and peak values of a chunk.
///
/// These are telemetry rather than audio: they exist to be reported over the
/// websocket, which uses `f32` throughout, so they are stored as `f32`. The
/// values are still accumulated at processing precision in [`rms_and_peak`] and
/// converted once here.
pub struct ChunkStats {
    pub rms: Vec<f32>,
    pub peak: Vec<f32>,
}

impl ChunkStats {
    fn write_values<F>(&self, source: &[f32], values: &mut Vec<f32>, map: F)
    where
        F: Fn(f32) -> f32,
    {
        values.resize(source.len(), 0.0);
        for (slot, value) in values.iter_mut().zip(source.iter()) {
            *slot = map(*value);
        }
    }

    pub fn rms_db(&self, values: &mut Vec<f32>) {
        self.write_values(&self.rms, values, linear_to_db)
    }

    pub fn rms_linear(&self, values: &mut Vec<f32>) {
        self.write_values(&self.rms, values, |value| value)
    }

    pub fn peak_db(&self, values: &mut Vec<f32>) {
        self.write_values(&self.peak, values, linear_to_db)
    }

    pub fn peak_linear(&self, values: &mut Vec<f32>) {
        self.write_values(&self.peak, values, |value| value)
    }
}

impl AudioChunk {
    /// Create a new chunk from `waveforms`, recording the expected max/min values and frame count.
    pub fn new(
        waveforms: Vec<Vec<CamillaFloat>>,
        maxval: CamillaFloat,
        minval: CamillaFloat,
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

    pub fn from(chunk: &AudioChunk, waveforms: Vec<Vec<CamillaFloat>>) -> Self {
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
        let rms_peak: Vec<(CamillaFloat, CamillaFloat)> =
            self.waveforms.iter().map(|wf| rms_and_peak(wf)).collect();
        let rms: Vec<f32> = rms_peak.iter().map(|rp| rp.0.to_f32()).collect();
        let peak: Vec<f32> = rms_peak.iter().map(|rp| rp.1.to_f32()).collect();
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
            *peakval = peak.to_f32();
            *rmsval = rms.to_f32();
        }
        xtrace!("Stats: rms {:?}, peak {:?}", stats.rms, stats.peak);
    }

    pub fn update_channel_mask(&self, mask: &mut [bool]) {
        mask.iter_mut()
            .zip(self.waveforms.iter())
            .for_each(|(m, w)| *m = !w.is_empty());
    }
}

/// Get RMS and peak value of a vector
pub fn rms_and_peak(data: &[CamillaFloat]) -> (CamillaFloat, CamillaFloat) {
    if !data.is_empty() {
        // Independent accumulators let the compiler vectorize both reductions.
        let mut squares = [0.0; 8];
        let mut peaks: [CamillaFloat; 8] = [0.0; 8];
        let (chunks, remainder) = data.as_chunks::<8>();
        for chunk in chunks {
            for i in 0..8 {
                squares[i] += chunk[i] * chunk[i];
                peaks[i] = peaks[i].max(chunk[i].abs());
            }
        }
        let mut squaresum = squares.iter().sum::<CamillaFloat>();
        let mut peakval = peaks.into_iter().fold(0.0, CamillaFloat::max);
        for value in remainder {
            squaresum += *value * *value;
            peakval = peakval.max(value.abs());
        }
        ((squaresum / data.len() as CamillaFloat).sqrt(), peakval)
    } else {
        (0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToCamillaFloat;

    #[test]
    fn chunk_levels_include_partial_blocks_and_silent_channels() {
        for frames in [1, 7, 8, 9, 63, 64, 65, 1023, 1024, 1025, 4097] {
            let mut reference: Vec<_> =
                (0..frames).map(|i| 0.7 * (i as f64 * 0.13).sin()).collect();
            reference[frames - 1] = -1.25;
            let waveform = reference.iter().map(|v| v.to_camilla_float()).collect();
            let expected_rms = (reference.iter().map(|v| v * v).sum::<f64>() / frames as f64)
                .sqrt()
                .to_f32();
            let chunk = AudioChunk::new(
                vec![waveform, Vec::new(), vec![0.0; frames]],
                0.7,
                -1.25,
                frames,
                frames,
            );
            let mut stats = ChunkStats {
                rms: vec![1.0; 3],
                peak: vec![1.0; 3],
            };
            chunk.update_stats(&mut stats);
            assert!(
                (stats.rms[0] - expected_rms).abs() < 2e-6,
                "frames={frames}"
            );
            assert_eq!(stats.peak, [1.25, 0.0, 0.0]);
            assert_eq!(&stats.rms[1..], &[0.0, 0.0]);
        }
    }
}
