//! Characterization of the original signed-sum path, independent of the new reducer.
use super::*;

#[test]
fn legacy_signed_sum_matches_bitwise_across_chunks() {
    for mode in [None, Some(config::MonitorMode::Sum)] {
        let mut parameters: config::NoiseGateParameters = yaml_serde::from_str(
            "channels: 3
monitor_channels: [2, 0, 2]
process_channels: [0, 1]
attack: 3
attack_unit: samples
release: 11
release_unit: samples
threshold: -20
attenuation: 30
",
        )
        .unwrap();
        parameters.monitor_mode = mode;
        let mut actual_processor = NoiseGate::from_config("legacy", parameters, 48000, 16);
        let mut legacy = actual_processor.clone();
        for block in 0..8 {
            let waveforms: Vec<Vec<CamillaFloat>> = (0..3)
                .map(|ch| {
                    (0..16)
                        .map(|i| {
                            let n = (block * 16 + i + 3 * ch) as CamillaFloat;
                            (n * 0.17).sin() * (ch + 1) as CamillaFloat
                        })
                        .collect()
                })
                .collect();
            let mut actual = AudioChunk::new(waveforms.clone(), 1.0, -1.0, 16, 16);
            let mut expected = AudioChunk::new(waveforms, 1.0, -1.0, 16, 16);
            actual_processor.process_chunk(&mut actual);

            // Verbatim reduction order from the pre-feature code. The unchanged
            // private envelope/gain functions remain the dynamics reference.
            legacy
                .scratch
                .copy_from_slice(&expected.waveforms[legacy.monitor_channels[0]]);
            for &ch in legacy.monitor_channels.iter().skip(1) {
                for (acc, value) in legacy.scratch.iter_mut().zip(&expected.waveforms[ch]) {
                    *acc += *value;
                }
            }
            legacy.estimate_loudness();
            legacy.calculate_linear_gain();
            for &ch in &legacy.process_channels {
                legacy.apply_gain(&mut expected.waveforms[ch]);
            }
            for (a, b) in actual
                .waveforms
                .iter()
                .flatten()
                .zip(expected.waveforms.iter().flatten())
            {
                assert_eq!(a.to_bits(), b.to_bits());
            }
            assert_eq!(
                actual_processor.prev_loudness.to_bits(),
                legacy.prev_loudness.to_bits()
            );
        }
    }
}

#[test]
fn all_mode_updates_preserve_smoothed_level() {
    let modes = [
        config::MonitorMode::Sum,
        config::MonitorMode::Max,
        config::MonitorMode::Rms,
    ];
    for before in modes {
        for after in modes {
            let mut parameters: config::NoiseGateParameters = yaml_serde::from_str(
                "channels: 2\nattack: 3\nattack_unit: samples\nrelease: 19\nrelease_unit: samples\nthreshold: -20\nattenuation: 40\n",
            ).unwrap();
            parameters.monitor_mode = Some(before);
            let mut p = NoiseGate::from_config("p", parameters.clone(), 48000, 8);
            let mut input = AudioChunk::new(vec![vec![0.5; 8], vec![-0.25; 8]], 1.0, -1.0, 8, 8);
            p.process_chunk(&mut input);
            let previous = p.prev_loudness.to_bits();
            parameters.monitor_mode = Some(after);
            p.update_parameters(config::Processor::NoiseGate {
                description: None,
                parameters,
            });
            assert_eq!(p.prev_loudness.to_bits(), previous);
            assert_eq!(p.monitor_mode, after);
        }
    }
}

#[test]
fn zero_channel_count_is_rejected() {
    let parameters: config::NoiseGateParameters = yaml_serde::from_str(
        "channels: 0\nattack: 3\nattack_unit: samples\nrelease: 19\nrelease_unit: samples\nthreshold: -20\nattenuation: 40\n",
    ).unwrap();
    assert!(validate_noise_gate(&parameters).is_err());
}
