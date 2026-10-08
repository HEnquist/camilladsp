//! Channel reduction only; consumers retain their own rectification/envelope logic.

use crate::CamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config::MonitorMode;

/// Reduce resolved monitor entries into scratch. Sum stays signed; the
/// processors rectify it when calculating the envelope.
pub(crate) fn aggregate_monitor_channels(
    input: &AudioChunk,
    channels: &[usize],
    mode: MonitorMode,
    scratch: &mut [CamillaFloat],
) {
    match mode {
        MonitorMode::Sum => {
            scratch.copy_from_slice(&input.waveforms[channels[0]]);
            for &ch in channels.iter().skip(1) {
                for (acc, val) in scratch.iter_mut().zip(input.waveforms[ch].iter()) {
                    *acc += *val;
                }
            }
        }
        MonitorMode::Max => {
            for (peak, val) in scratch.iter_mut().zip(input.waveforms[channels[0]].iter()) {
                *peak = val.abs();
            }
            for &ch in channels.iter().skip(1) {
                for (peak, val) in scratch.iter_mut().zip(input.waveforms[ch].iter()) {
                    *peak = peak.max(val.abs());
                }
            }
        }
        MonitorMode::Rms => {
            scratch.fill(0.0);
            for &ch in channels {
                // Capture-pruned empty waveforms contribute silence, but still
                // count towards the number of monitor entries.
                for (acc, val) in scratch.iter_mut().zip(input.waveforms[ch].iter()) {
                    *acc += val * val;
                }
            }
            let count = channels.len() as CamillaFloat;
            for val in scratch.iter_mut() {
                *val = (*val / count).sqrt();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitor_modes() {
        let input = AudioChunk::new(
            vec![vec![0.5, -0.5, 0.5], vec![-0.5, -0.25, 0.0], vec![1.0; 3]],
            1.0,
            -1.0,
            3,
            3,
        );
        // Select a subset with a repeated entry; Rms divides by three entries.
        for (mode, expected) in [
            (MonitorMode::Sum, [0.5, -1.25, 1.0]),
            (MonitorMode::Max, [0.5, 0.5, 0.5]),
            (
                MonitorMode::Rms,
                [
                    0.5,
                    CamillaFloat::sqrt(0.1875),
                    CamillaFloat::sqrt(0.5 / 3.0),
                ],
            ),
        ] {
            let mut scratch = [123.0; 3];
            aggregate_monitor_channels(&input, &[0, 1, 0], mode, &mut scratch);
            for (actual, expected) in scratch.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn test_rms_empty_waveforms_count_as_silence() {
        let input = AudioChunk::new(vec![vec![], vec![0.5, -0.5]], 1.0, -1.0, 2, 2);
        let mut scratch = [123.0; 2];
        aggregate_monitor_channels(&input, &[0, 1], MonitorMode::Rms, &mut scratch);
        for value in scratch {
            assert!((value - 0.5 / CamillaFloat::sqrt(2.0)).abs() < 1e-6);
        }
        aggregate_monitor_channels(&input, &[0], MonitorMode::Rms, &mut scratch);
        assert_eq!(scratch, [0.0; 2]);
    }
}
