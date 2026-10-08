//! Channel reduction only; consumers retain their own rectification/envelope logic.

use crate::CamillaFloat;
use crate::audiochunk::AudioChunk;
use crate::config::MonitorMode;

/// Reduce selected monitor entries into an existing scratch buffer.
///
/// `channels` must be nonempty, indices valid, and selected waveforms must have
/// the configured chunk length. These are the existing processor preconditions.
/// Entries are neither sorted nor deduplicated. Sum deliberately stays signed:
/// both level estimators and LookaheadGain already rectify their input.
///
/// Sum and Max retain their original accumulation order and exceptional-value
/// behavior. In particular, Max uses the limiter's floating-point `max`, which
/// ignores a single NaN operand. Rms propagates any NaN, otherwise infinity.
/// This is not input sanitization or a downstream recovery guarantee.
pub(crate) fn aggregate_monitor_channels(
    input: &AudioChunk,
    channels: &[usize],
    mode: MonitorMode,
    scratch: &mut [CamillaFloat],
) {
    assert!(
        !channels.is_empty(),
        "Resolved monitor channels must not be empty"
    );
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
            let count = channels.len() as CamillaFloat;
            for (frame, out) in scratch.iter_mut().enumerate() {
                // Scaling avoids squaring large finite samples before reducing
                // them. A second channel pass needs no extra scratch allocation.
                let mut scale: CamillaFloat = 0.0;
                for &ch in channels {
                    let magnitude = input.waveforms[ch][frame].abs();
                    if magnitude.is_nan() {
                        scale = CamillaFloat::NAN;
                        break;
                    }
                    scale = scale.max(magnitude);
                }
                if scale == 0.0 || !scale.is_finite() {
                    *out = scale;
                    continue;
                }
                let mut sum: CamillaFloat = 0.0;
                for &ch in channels {
                    let normalized = input.waveforms[ch][frame] / scale;
                    sum += normalized * normalized;
                }
                // The exact mean is <= 1. Bound rounding error before the
                // multiplication so a representable result cannot overflow.
                *out = scale * (sum / count).min(1.0).sqrt();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduce(samples: &[CamillaFloat], channels: &[usize], mode: MonitorMode) -> CamillaFloat {
        let chunk = AudioChunk::new(
            samples.iter().map(|&sample| vec![sample]).collect(),
            1.0,
            -1.0,
            1,
            1,
        );
        let mut scratch = [123.0];
        aggregate_monitor_channels(&chunk, channels, mode, &mut scratch);
        scratch[0]
    }

    fn close(actual: CamillaFloat, expected: CamillaFloat) {
        let tolerance = 16.0 * CamillaFloat::EPSILON * expected.abs().max(1.0);
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn finite_examples() {
        let cases: &[(&[CamillaFloat], [CamillaFloat; 3])] = &[
            (&[0.5], [0.5, 0.5, 0.5]),
            (&[-0.5], [-0.5, 0.5, 0.5]),
            (&[0.5, 0.0], [0.5, 0.5, 0.5 / CamillaFloat::sqrt(2.0)]),
            (&[0.0, 0.5], [0.5, 0.5, 0.5 / CamillaFloat::sqrt(2.0)]),
            (&[0.5, 0.5], [1.0, 0.5, 0.5]),
            (&[0.5, -0.5], [0.0, 0.5, 0.5]),
            (&[0.0, 0.0], [0.0, 0.0, 0.0]),
            (&[0.5; 4], [2.0, 0.5, 0.5]),
        ];
        for &(samples, expected) in cases {
            let channels: Vec<_> = (0..samples.len()).collect();
            for (mode, want) in [MonitorMode::Sum, MonitorMode::Max, MonitorMode::Rms]
                .into_iter()
                .zip(expected)
            {
                close(reduce(samples, &channels, mode), want);
            }
        }
    }

    #[test]
    fn subsets_repetitions_and_polarity() {
        let samples = [0.5, -0.25, 8.0];
        close(reduce(&samples, &[0, 1], MonitorMode::Sum), 0.25);
        close(reduce(&samples, &[0, 0, 1], MonitorMode::Sum), 0.75);
        close(
            reduce(&samples, &[0, 0, 1], MonitorMode::Rms),
            CamillaFloat::sqrt(0.1875),
        );
        for mode in [MonitorMode::Max, MonitorMode::Rms] {
            let expected = reduce(&samples, &[0, 1], mode);
            for signs in [[1.0, 1.0], [-1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]] {
                let changed = [samples[0] * signs[0], samples[1] * signs[1]];
                close(reduce(&changed, &[0, 1], mode), expected);
                close(reduce(&changed, &[1, 0], mode), expected);
            }
        }
    }

    #[test]
    fn rms_extreme_finite_values() {
        for scale in [CamillaFloat::MAX, CamillaFloat::MIN_POSITIVE, 4.0] {
            let identical = reduce(&[scale, -scale], &[0, 1], MonitorMode::Rms);
            assert_eq!(identical, scale);
            let diluted = reduce(&[scale, 0.0], &[0, 1], MonitorMode::Rms);
            assert!(diluted.is_finite() && diluted > 0.0);
            close(diluted / scale, CamillaFloat::sqrt(0.5));
        }
        let tiny = CamillaFloat::from_bits(1);
        assert_eq!(reduce(&[tiny, tiny], &[0, 1], MonitorMode::Rms), tiny);
    }

    #[test]
    fn exceptional_values_are_explicit() {
        let nan = CamillaFloat::NAN;
        let inf = CamillaFloat::INFINITY;
        for samples in [[nan, 0.5], [0.5, nan], [inf, nan], [nan, inf]] {
            assert!(reduce(&samples, &[0, 1], MonitorMode::Rms).is_nan());
        }
        for samples in [[inf, 0.5], [-inf, 0.5], [inf, -inf]] {
            assert_eq!(reduce(&samples, &[0, 1], MonitorMode::Rms), inf);
        }
        for samples in [[nan, 0.5], [0.5, nan]] {
            assert_eq!(reduce(&samples, &[0, 1], MonitorMode::Max), 0.5);
        }
        assert!(reduce(&[nan, nan], &[0, 1], MonitorMode::Max).is_nan());
        assert!(reduce(&[inf, -inf], &[0, 1], MonitorMode::Sum).is_nan());
        assert!(reduce(&[nan, 0.5], &[0, 1], MonitorMode::Sum).is_nan());
        let negative_zero: CamillaFloat = -0.0;
        assert_eq!(
            reduce(&[-0.0], &[0], MonitorMode::Sum).to_bits(),
            negative_zero.to_bits()
        );
    }

    #[test]
    #[should_panic(expected = "Resolved monitor channels")]
    fn empty_resolved_selection_is_an_error() {
        reduce(&[0.5], &[], MonitorMode::Rms);
    }
}
