// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
// Copyright (C) 2026 PureRoad contributors
//
// The native Acceleration2 kernel is derived from Airwindows, Copyright (c)
// Chris Johnson, under the MIT license. See THIRD_PARTY_NOTICES.md.

use crate::audiochunk::AudioChunk;
use crate::config;
use crate::processors::Processor;
use crate::{PrcFmt, Res};
use std::ffi::c_void;
use std::ptr::NonNull;

unsafe extern "C" {
    fn pureroad_acceleration2_create(sample_rate: f64) -> *mut c_void;
    fn pureroad_acceleration2_destroy(instance: *mut c_void);
    fn pureroad_acceleration2_reset(instance: *mut c_void);
    #[cfg(not(feature = "32bit"))]
    fn pureroad_acceleration2_process_f64(
        instance: *mut c_void,
        left: *mut f64,
        right: *mut f64,
        frames: usize,
        intensity: f64,
        transition_samples: usize,
    ) -> i32;
    #[cfg(feature = "32bit")]
    fn pureroad_acceleration2_process_f32(
        instance: *mut c_void,
        left: *mut f32,
        right: *mut f32,
        frames: usize,
        intensity: f64,
        transition_samples: usize,
    ) -> i32;
    fn pureroad_totape8_create(sample_rate: f64) -> *mut c_void;
    fn pureroad_totape8_destroy(instance: *mut c_void);
    fn pureroad_totape8_reset(instance: *mut c_void);
    #[cfg(not(feature = "32bit"))]
    fn pureroad_totape8_process_f64(
        instance: *mut c_void,
        left: *mut f64,
        right: *mut f64,
        frames: usize,
        parameters: *const f64,
        transition_samples: usize,
    ) -> i32;
    #[cfg(feature = "32bit")]
    fn pureroad_totape8_process_f32(
        instance: *mut c_void,
        left: *mut f32,
        right: *mut f32,
        frames: usize,
        parameters: *const f64,
        transition_samples: usize,
    ) -> i32;
}

struct NativeAcceleration2 {
    instance: Option<NonNull<c_void>>,
}

// The pointer has unique ownership and is only accessed through &mut self on
// CamillaDSP's processing thread.
unsafe impl Send for NativeAcceleration2 {}

impl NativeAcceleration2 {
    fn new(sample_rate: usize) -> Self {
        // SAFETY: The constructor accepts a value and returns an owned opaque handle.
        let instance = NonNull::new(unsafe { pureroad_acceleration2_create(sample_rate as f64) });
        Self { instance }
    }

    fn is_available(&self) -> bool {
        self.instance.is_some()
    }

    fn reset(&mut self) {
        if let Some(instance) = self.instance {
            // SAFETY: The handle remains owned by self until Drop.
            unsafe { pureroad_acceleration2_reset(instance.as_ptr()) };
        }
    }

    fn process(
        &mut self,
        left: &mut [PrcFmt],
        right: &mut [PrcFmt],
        intensity: f64,
        transition_samples: usize,
    ) -> bool {
        let Some(instance) = self.instance else {
            return false;
        };
        if left.len() != right.len() {
            return false;
        }
        #[cfg(not(feature = "32bit"))]
        // SAFETY: The slices are valid, non-overlapping channel buffers for frames samples.
        let success = unsafe {
            pureroad_acceleration2_process_f64(
                instance.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len(),
                intensity,
                transition_samples,
            )
        };
        #[cfg(feature = "32bit")]
        // SAFETY: The slices are valid, non-overlapping channel buffers for frames samples.
        let success = unsafe {
            pureroad_acceleration2_process_f32(
                instance.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len(),
                intensity,
                transition_samples,
            )
        };
        success != 0
    }
}

impl Drop for NativeAcceleration2 {
    fn drop(&mut self) {
        if let Some(instance) = self.instance {
            // SAFETY: This is the sole owner and Drop runs exactly once.
            unsafe { pureroad_acceleration2_destroy(instance.as_ptr()) };
        }
    }
}

struct NativeToTape8 {
    instance: Option<NonNull<c_void>>,
}

unsafe impl Send for NativeToTape8 {}

impl NativeToTape8 {
    fn new(sample_rate: usize) -> Self {
        // SAFETY: The constructor returns an owned opaque handle.
        let instance = NonNull::new(unsafe { pureroad_totape8_create(sample_rate as f64) });
        Self { instance }
    }

    fn reset(&mut self) {
        if let Some(instance) = self.instance {
            // SAFETY: The handle remains owned by self until Drop.
            unsafe { pureroad_totape8_reset(instance.as_ptr()) };
        }
    }

    fn is_available(&self) -> bool {
        self.instance.is_some()
    }

    fn process(
        &mut self,
        left: &mut [PrcFmt],
        right: &mut [PrcFmt],
        parameters: &[f64; 9],
        transition_samples: usize,
    ) -> bool {
        let Some(instance) = self.instance else {
            return false;
        };
        if left.len() != right.len() {
            return false;
        }
        #[cfg(not(feature = "32bit"))]
        // SAFETY: Channel slices and the fixed parameter array are valid for this call.
        let success = unsafe {
            pureroad_totape8_process_f64(
                instance.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len(),
                parameters.as_ptr(),
                transition_samples,
            )
        };
        #[cfg(feature = "32bit")]
        // SAFETY: Channel slices and the fixed parameter array are valid for this call.
        let success = unsafe {
            pureroad_totape8_process_f32(
                instance.as_ptr(),
                left.as_mut_ptr(),
                right.as_mut_ptr(),
                left.len(),
                parameters.as_ptr(),
                transition_samples,
            )
        };
        success != 0
    }
}

impl Drop for NativeToTape8 {
    fn drop(&mut self) {
        if let Some(instance) = self.instance {
            // SAFETY: This is the sole owner and Drop runs exactly once.
            unsafe { pureroad_totape8_destroy(instance.as_ptr()) };
        }
    }
}

pub struct PureroadCharacter {
    name: String,
    channels: usize,
    samplerate: usize,
    algorithm: config::PureroadCharacterAlgorithm,
    previous_algorithm: Option<config::PureroadCharacterAlgorithm>,
    intensity: f64,
    previous_intensity: f64,
    totape8: [f64; 9],
    previous_totape8: [f64; 9],
    current_mix: f64,
    target_mix: f64,
    transition_samples: usize,
    transition_remaining: usize,
    dry: [Vec<PrcFmt>; 2],
    previous_wet: [Vec<PrcFmt>; 2],
    algorithm_transition_total: usize,
    algorithm_transition_remaining: usize,
    native: NativeAcceleration2,
    native_totape8: NativeToTape8,
    processing_errors: u64,
    pending_parameters: Option<config::PureroadCharacterParameters>,
}

impl PureroadCharacter {
    pub fn from_config(
        name: &str,
        parameters: config::PureroadCharacterParameters,
        samplerate: usize,
        chunksize: usize,
    ) -> Self {
        let target_mix = effective_mix(parameters.algorithm, prc_to_f64(parameters.mix));
        let native = NativeAcceleration2::new(samplerate);
        let native_totape8 = NativeToTape8::new(samplerate);
        if !native.is_available() {
            error!(
                "PureroadCharacter '{name}': native Acceleration2 initialization failed; processor will bypass"
            );
        }
        if !native_totape8.is_available() {
            error!(
                "PureroadCharacter '{name}': native ToTape8 initialization failed; processor will bypass"
            );
        }
        Self {
            name: name.to_owned(),
            channels: parameters.channels,
            samplerate,
            algorithm: parameters.algorithm,
            previous_algorithm: None,
            intensity: prc_to_f64(parameters.intensity),
            previous_intensity: prc_to_f64(parameters.intensity),
            totape8: totape8_values(parameters.totape8),
            previous_totape8: totape8_values(parameters.totape8),
            current_mix: target_mix,
            target_mix,
            transition_samples: transition_samples(parameters.transition_ms, samplerate),
            transition_remaining: 0,
            dry: [vec![0.0; chunksize], vec![0.0; chunksize]],
            previous_wet: [vec![0.0; chunksize], vec![0.0; chunksize]],
            algorithm_transition_total: 0,
            algorithm_transition_remaining: 0,
            native,
            native_totape8,
            processing_errors: 0,
            pending_parameters: None,
        }
    }

    #[inline]
    fn next_mix(&mut self) -> f64 {
        if self.transition_remaining == 0 {
            self.current_mix = self.target_mix;
        } else {
            self.current_mix +=
                (self.target_mix - self.current_mix) / self.transition_remaining as f64;
            self.transition_remaining -= 1;
        }
        self.current_mix
    }

    fn restore_dry(&mut self, chunk: &mut AudioChunk, frames: usize) {
        chunk.waveforms[0][..frames].copy_from_slice(&self.dry[0][..frames]);
        chunk.waveforms[1][..frames].copy_from_slice(&self.dry[1][..frames]);
    }

    fn apply_parameters(&mut self, parameters: config::PureroadCharacterParameters) {
        let algorithm_changed = self.algorithm != parameters.algorithm;
        let next_target_mix = effective_mix(parameters.algorithm, prc_to_f64(parameters.mix));
        let resumes_from_full_bypass = !algorithm_changed
            && self.current_mix == 0.0
            && self.target_mix == 0.0
            && next_target_mix > 0.0;
        if algorithm_changed {
            self.previous_algorithm = Some(self.algorithm);
            self.previous_intensity = self.intensity;
            self.previous_totape8 = self.totape8;
        }
        self.algorithm = parameters.algorithm;
        self.intensity = prc_to_f64(parameters.intensity.clamp(0.0, 1.0));
        self.totape8 = totape8_values(parameters.totape8);
        self.target_mix = next_target_mix;
        self.transition_samples = transition_samples(parameters.transition_ms, self.samplerate);
        self.transition_remaining = self.transition_samples;
        if algorithm_changed {
            self.algorithm_transition_total = self.transition_samples;
            self.algorithm_transition_remaining = self.transition_samples;
            if self.transition_samples == 0 {
                self.previous_algorithm = None;
            }
        }
        if self.transition_remaining == 0 {
            self.current_mix = self.target_mix;
        }
        if algorithm_changed && parameters.algorithm != config::PureroadCharacterAlgorithm::Original
        {
            match parameters.algorithm {
                config::PureroadCharacterAlgorithm::Original => {}
                config::PureroadCharacterAlgorithm::Acceleration2 => self.native.reset(),
                config::PureroadCharacterAlgorithm::ToTape8 => self.native_totape8.reset(),
            }
        } else if resumes_from_full_bypass {
            // A fully dry processor does not advance its native delay and filter state.
            // Reset before fading it back in so audio captured before the bypass cannot
            // leak into the resumed wet path.
            match parameters.algorithm {
                config::PureroadCharacterAlgorithm::Original => {}
                config::PureroadCharacterAlgorithm::Acceleration2 => self.native.reset(),
                config::PureroadCharacterAlgorithm::ToTape8 => self.native_totape8.reset(),
            }
        }
    }
}

impl Processor for PureroadCharacter {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_chunk(&mut self, chunk: &mut AudioChunk) -> Res<()> {
        if chunk.channels != self.channels || chunk.waveforms.len() != self.channels {
            self.processing_errors = self.processing_errors.saturating_add(1);
            return Ok(());
        }
        let frames = chunk.waveforms[0].len().min(chunk.waveforms[1].len());
        if frames == 0
            || (self.current_mix == 0.0
                && self.target_mix == 0.0
                && self.previous_algorithm.is_none()
                && self.pending_parameters.is_none())
        {
            return Ok(());
        }
        if frames > self.dry[0].len() {
            self.processing_errors = self.processing_errors.saturating_add(1);
            return Ok(());
        }
        self.dry[0][..frames].copy_from_slice(&chunk.waveforms[0][..frames]);
        self.dry[1][..frames].copy_from_slice(&chunk.waveforms[1][..frames]);
        if self.previous_algorithm.is_some() {
            self.previous_wet[0][..frames].copy_from_slice(&self.dry[0][..frames]);
            self.previous_wet[1][..frames].copy_from_slice(&self.dry[1][..frames]);
        }
        if self
            .dry
            .iter()
            .flat_map(|channel| &channel[..frames])
            .any(|v| !v.is_finite())
        {
            self.native.reset();
            self.native_totape8.reset();
            self.previous_algorithm = None;
            return Ok(());
        }

        let (left_channels, right_channels) = chunk.waveforms.split_at_mut(1);
        let success = match self.algorithm {
            config::PureroadCharacterAlgorithm::Original => true,
            config::PureroadCharacterAlgorithm::Acceleration2 => self.native.process(
                &mut left_channels[0][..frames],
                &mut right_channels[0][..frames],
                self.intensity,
                self.transition_samples,
            ),
            config::PureroadCharacterAlgorithm::ToTape8 => self.native_totape8.process(
                &mut left_channels[0][..frames],
                &mut right_channels[0][..frames],
                &self.totape8,
                self.transition_samples,
            ),
        };
        let (previous_left, previous_right) = self.previous_wet.split_at_mut(1);
        let previous_success = match self.previous_algorithm {
            None | Some(config::PureroadCharacterAlgorithm::Original) => true,
            Some(config::PureroadCharacterAlgorithm::Acceleration2) => self.native.process(
                &mut previous_left[0][..frames],
                &mut previous_right[0][..frames],
                self.previous_intensity,
                0,
            ),
            Some(config::PureroadCharacterAlgorithm::ToTape8) => self.native_totape8.process(
                &mut previous_left[0][..frames],
                &mut previous_right[0][..frames],
                &self.previous_totape8,
                0,
            ),
        };
        if !success
            || !previous_success
            || chunk
                .waveforms
                .iter()
                .take(2)
                .flat_map(|channel| &channel[..frames])
                .any(|v| !v.is_finite())
        {
            self.restore_dry(chunk, frames);
            self.native.reset();
            self.native_totape8.reset();
            self.previous_algorithm = None;
            self.processing_errors = self.processing_errors.saturating_add(1);
            return Ok(());
        }

        for frame in 0..frames {
            let mix = prc_from_f64(self.next_mix());
            for channel in 0..2 {
                let dry = self.dry[channel][frame];
                let mut wet = chunk.waveforms[channel][frame];
                if let Some(previous) = self.previous_algorithm {
                    let old_wet = if previous == config::PureroadCharacterAlgorithm::Original {
                        dry
                    } else {
                        self.previous_wet[channel][frame]
                    };
                    let progress = if self.algorithm_transition_total == 0 {
                        1.0
                    } else {
                        1.0 - self.algorithm_transition_remaining as f64
                            / self.algorithm_transition_total as f64
                    };
                    wet = old_wet + (wet - old_wet) * prc_from_f64(progress);
                }
                chunk.waveforms[channel][frame] = dry + (wet - dry) * mix;
            }
            if self.previous_algorithm.is_some() && self.algorithm_transition_remaining > 0 {
                self.algorithm_transition_remaining -= 1;
                if self.algorithm_transition_remaining == 0 {
                    self.previous_algorithm = None;
                }
            }
        }
        if self.previous_algorithm.is_none()
            && let Some(parameters) = self.pending_parameters.take()
        {
            self.apply_parameters(parameters);
        }
        Ok(())
    }

    fn update_parameters(&mut self, processor: config::Processor) {
        let config::Processor::PureroadCharacter { parameters, .. } = processor else {
            self.processing_errors = self.processing_errors.saturating_add(1);
            return;
        };
        if self.previous_algorithm.is_some() && self.algorithm != parameters.algorithm {
            self.pending_parameters = Some(parameters);
        } else {
            self.pending_parameters = None;
            self.apply_parameters(parameters);
        }
    }
}

fn effective_mix(algorithm: config::PureroadCharacterAlgorithm, mix: f64) -> f64 {
    match algorithm {
        config::PureroadCharacterAlgorithm::Original => 0.0,
        config::PureroadCharacterAlgorithm::Acceleration2
        | config::PureroadCharacterAlgorithm::ToTape8 => mix.clamp(0.0, 1.0),
    }
}

fn transition_samples(transition_ms: f32, samplerate: usize) -> usize {
    (f64::from(transition_ms.max(0.0)) * samplerate as f64 / 1000.0).round() as usize
}

fn totape8_values(parameters: config::ToTape8Parameters) -> [f64; 9] {
    [
        airwindows_parameter(parameters.input),
        airwindows_parameter(parameters.tilt),
        airwindows_parameter(parameters.shape),
        airwindows_parameter(parameters.flutter),
        airwindows_parameter(parameters.flutter_speed),
        airwindows_parameter(parameters.bias),
        airwindows_parameter(parameters.head_bump),
        airwindows_parameter(parameters.head_bump_frequency),
        airwindows_parameter(parameters.output),
    ]
}

#[cfg(not(feature = "32bit"))]
fn airwindows_parameter(value: PrcFmt) -> f64 {
    f64::from(value as f32)
}

#[cfg(feature = "32bit")]
fn airwindows_parameter(value: PrcFmt) -> f64 {
    f64::from(value)
}

#[cfg(not(feature = "32bit"))]
fn prc_to_f64(value: PrcFmt) -> f64 {
    value
}

#[cfg(feature = "32bit")]
fn prc_to_f64(value: PrcFmt) -> f64 {
    f64::from(value)
}

#[cfg(not(feature = "32bit"))]
fn prc_from_f64(value: f64) -> PrcFmt {
    value
}

#[cfg(feature = "32bit")]
fn prc_from_f64(value: f64) -> PrcFmt {
    value as f32
}

pub fn validate_pureroad_character(
    parameters: &config::PureroadCharacterParameters,
    samplerate: usize,
) -> Res<()> {
    if samplerate <= 40_000 {
        return Err(config::ConfigError::new(
            "PureroadCharacter requires a sample rate above 40000 Hz.",
        )
        .into());
    }
    if parameters.channels != 2 {
        return Err(config::ConfigError::new(
            "PureroadCharacter currently requires exactly two channels.",
        )
        .into());
    }
    if !parameters.intensity.is_finite() || !(0.0..=1.0).contains(&parameters.intensity) {
        return Err(config::ConfigError::new(
            "Intensity must be finite and in the range 0.0 to 1.0.",
        )
        .into());
    }
    if !parameters.mix.is_finite() || !(0.0..=1.0).contains(&parameters.mix) {
        return Err(
            config::ConfigError::new("Mix must be finite and in the range 0.0 to 1.0.").into(),
        );
    }
    if !parameters.transition_ms.is_finite() || parameters.transition_ms < 0.0 {
        return Err(
            config::ConfigError::new("Transition time must be finite and non-negative.").into(),
        );
    }
    for (name, value) in [
        ("input", parameters.totape8.input),
        ("tilt", parameters.totape8.tilt),
        ("shape", parameters.totape8.shape),
        ("flutter", parameters.totape8.flutter),
        ("flutter_speed", parameters.totape8.flutter_speed),
        ("bias", parameters.totape8.bias),
        ("head_bump", parameters.totape8.head_bump),
        (
            "head_bump_frequency",
            parameters.totape8.head_bump_frequency,
        ),
        ("output", parameters.totape8.output),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(config::ConfigError::new(&format!(
                "ToTape8 parameter '{name}' must be finite and in the range 0.0 to 1.0."
            ))
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::approx_constant, clippy::needless_range_loop)]
    use super::*;
    use rand::rngs::SmallRng;
    use rand::{Rng, SeedableRng};

    #[derive(Clone, Copy, Default)]
    struct ReferenceBiquadState {
        first: f64,
        second: f64,
    }

    #[derive(Clone, Copy)]
    struct ReferenceBiquad {
        a0: f64,
        a1: f64,
        a2: f64,
        b1: f64,
        b2: f64,
    }

    impl ReferenceBiquad {
        fn lowpass(frequency: f64) -> Self {
            let k = (std::f64::consts::PI * frequency).tan();
            let norm = 1.0 / (1.0 + k / 0.7071 + k * k);
            let a0 = k * k * norm;
            Self {
                a0,
                a1: 2.0 * a0,
                a2: a0,
                b1: 2.0 * (k * k - 1.0) * norm,
                b2: (1.0 - k / 0.7071 + k * k) * norm,
            }
        }

        fn process(&self, input: f64, state: &mut ReferenceBiquadState) -> f64 {
            let output = input * self.a0 + state.first;
            state.first = input * self.a1 - output * self.b1 + state.second;
            state.second = input * self.a2 - output * self.b2;
            output
        }
    }

    struct ReferenceChannel {
        history: [f64; 34],
        smooth: ReferenceBiquadState,
        ultrasonic: ReferenceBiquadState,
        random: u32,
    }

    impl ReferenceChannel {
        fn new(random: u32) -> Self {
            Self {
                history: [0.0; 34],
                smooth: ReferenceBiquadState::default(),
                ultrasonic: ReferenceBiquadState::default(),
                random,
            }
        }
    }

    struct UpstreamStyleReference {
        rate: f64,
        channels: [ReferenceChannel; 2],
    }

    impl UpstreamStyleReference {
        fn new(rate: usize) -> Self {
            Self {
                rate: rate as f64,
                channels: [
                    ReferenceChannel::new(0x9e37_79b9),
                    ReferenceChannel::new(0x3c6e_f372),
                ],
            }
        }

        fn process(&mut self, channels: &mut [Vec<PrcFmt>; 2], parameter: f64) {
            let intensity = parameter.powi(3) * 32.0;
            let spacing = ((1.73 * self.rate / 44_100.0) as usize + 1).min(16);
            let smooth = ReferenceBiquad::lowpass(
                20_000.0 * (1.0 - parameter * 0.618_033_988_749_894_8) / self.rate,
            );
            let ultrasonic = ReferenceBiquad::lowpass(20_000.0 / self.rate);
            for frame in 0..channels[0].len() {
                for channel_index in 0..2 {
                    let state = &mut self.channels[channel_index];
                    let mut input = f64::from(channels[channel_index][frame]);
                    if input.abs() < 1.18e-23 {
                        input = state.random as f64 * 1.18e-17;
                    }
                    let smoothed = smooth.process(input, &mut state.smooth);
                    for index in (0..=spacing * 2).rev() {
                        state.history[index + 1] = state.history[index];
                    }
                    state.history[0] = input;
                    let first = state.history[0] - state.history[spacing];
                    let second = state.history[spacing] - state.history[spacing * 2];
                    let m1 = first * first.abs();
                    let m2 = second * second.abs();
                    let sense = (intensity * intensity * (m1 - m2).abs()).min(1.0);
                    input = input * (1.0 - sense) + smoothed * sense;
                    input = ultrasonic.process(input, &mut state.ultrasonic);
                    channels[channel_index][frame] = to_prc(input);
                    state.random ^= state.random << 13;
                    state.random ^= state.random >> 17;
                    state.random ^= state.random << 5;
                }
            }
        }
    }

    #[cfg(not(feature = "32bit"))]
    fn to_prc(value: f64) -> PrcFmt {
        value
    }

    #[cfg(feature = "32bit")]
    fn to_prc(value: f64) -> PrcFmt {
        value as f32
    }

    fn params(
        algorithm: config::PureroadCharacterAlgorithm,
        intensity: PrcFmt,
        mix: PrcFmt,
        transition_ms: f32,
    ) -> config::PureroadCharacterParameters {
        config::PureroadCharacterParameters {
            channels: 2,
            algorithm,
            intensity,
            mix,
            transition_ms,
            totape8: config::ToTape8Parameters::default(),
        }
    }

    fn chunk(left: Vec<PrcFmt>, right: Vec<PrcFmt>) -> AudioChunk {
        let frames = left.len();
        AudioChunk::new(vec![left, right], 0.0, 0.0, frames, frames)
    }

    fn signal(frames: usize, samplerate: usize) -> (Vec<PrcFmt>, Vec<PrcFmt>) {
        let channel = |f1: f64, f2: f64| {
            (0..frames)
                .map(|n| {
                    let t = n as f64 / samplerate as f64;
                    to_prc(
                        0.41 * (std::f64::consts::TAU * f1 * t).sin()
                            + 0.17 * (std::f64::consts::TAU * f2 * t).sin(),
                    )
                })
                .collect()
        };
        (channel(997.0, 13_117.0), channel(503.0, 17_003.0))
    }

    fn tone(frames: usize, samplerate: usize, frequency: f64, amplitude: f64) -> Vec<PrcFmt> {
        (0..frames)
            .map(|frame| {
                to_prc(
                    amplitude
                        * (std::f64::consts::TAU * frequency * frame as f64 / samplerate as f64)
                            .sin(),
                )
            })
            .collect()
    }

    fn rms(samples: &[PrcFmt]) -> f64 {
        (samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    fn amplitude_at(samples: &[PrcFmt], samplerate: usize, frequency: f64) -> f64 {
        let (sin_sum, cos_sum) =
            samples
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(sin_sum, cos_sum), (frame, sample)| {
                    let phase =
                        std::f64::consts::TAU * frequency * frame as f64 / samplerate as f64;
                    let sample = f64::from(*sample);
                    (
                        sin_sum + sample * phase.sin(),
                        cos_sum + sample * phase.cos(),
                    )
                });
        2.0 * sin_sum.hypot(cos_sum) / samples.len() as f64
    }

    fn db_ratio(numerator: f64, denominator: f64) -> f64 {
        20.0 * (numerator / denominator).max(1e-30).log10()
    }

    fn processor(
        rate: usize,
        frames: usize,
        algorithm: config::PureroadCharacterAlgorithm,
        intensity: PrcFmt,
        mix: PrcFmt,
        transition_ms: f32,
    ) -> PureroadCharacter {
        PureroadCharacter::from_config(
            "test",
            params(algorithm, intensity, mix, transition_ms),
            rate,
            frames,
        )
    }

    #[test]
    fn validation_rejects_every_invalid_dimension() {
        let mut value = params(
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            100.0,
        );
        assert!(validate_pureroad_character(&value, 48_000).is_ok());
        assert!(validate_pureroad_character(&value, 40_000).is_err());
        value.channels = 1;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
        value.channels = 2;
        value.intensity = PrcFmt::NAN;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
        value.intensity = 0.5;
        value.mix = -0.1;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
        value.mix = 0.5;
        value.transition_ms = f32::INFINITY;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
    }

    #[test]
    fn yaml_processor_configuration_deserializes() {
        let yaml = r#"
type: PureroadCharacter
parameters:
  channels: 2
  algorithm: Acceleration2
  intensity: 0.32
  mix: 1.0
  transition_ms: 100.0
"#;
        let parsed: config::Processor = yaml_serde::from_str(yaml).unwrap();
        let config::Processor::PureroadCharacter { parameters, .. } = parsed else {
            panic!("wrong processor variant");
        };
        assert_eq!(
            parameters.algorithm,
            config::PureroadCharacterAlgorithm::Acceleration2
        );
        assert_eq!(parameters.channels, 2);
        assert_eq!(parameters.intensity, 0.32);
        assert_eq!(parameters.mix, 1.0);
        assert_eq!(parameters.transition_ms, 100.0);
    }

    #[test]
    fn complete_yaml_builds_and_runs_pipeline() {
        let yaml = r#"
devices:
  samplerate: 96000
  chunksize: 256
  capture:
    type: Stdin
    channels: 2
    format: S16_LE
  playback:
    type: Stdout
    channels: 2
    format: S16_LE
processors:
  gentle_character:
    type: PureroadCharacter
    parameters:
      channels: 2
      algorithm: Acceleration2
      intensity: 0.32
      mix: 1.0
      transition_ms: 100.0
pipeline:
  - type: Processor
    name: gentle_character
  - type: DefaultVolume
"#;
        let mut unsupported: config::Configuration =
            yaml_serde::from_str(&yaml.replace("samplerate: 96000", "samplerate: 40000")).unwrap();
        assert!(crate::config::validate_config(&mut unsupported, None).is_err());

        let mut configuration: config::Configuration = yaml_serde::from_str(yaml).unwrap();
        crate::config::validate_config(&mut configuration, None).unwrap();
        let processing =
            std::sync::Arc::new(crate::ProcessingParameters::new(&[0.0_f32; 5], &[false; 5]));
        let mut pipeline = crate::pipeline::Pipeline::from_config(configuration, processing);
        let (left, right) = signal(256, 96_000);
        let output = pipeline.process_chunk(chunk(left.clone(), right.clone()));
        assert!(
            output
                .waveforms
                .iter()
                .flatten()
                .all(|sample| sample.is_finite())
        );
        assert_ne!(output.waveforms[0], left);
        assert_ne!(output.waveforms[1], right);
    }

    #[test]
    fn explicit_default_volume_step_runs_after_character() {
        fn make_pipeline(explicit_volume: bool) -> crate::pipeline::Pipeline {
            let volume_step = if explicit_volume {
                "  - type: DefaultVolume\n"
            } else {
                ""
            };
            let yaml = format!(
                r#"
devices:
  samplerate: 48000
  chunksize: 64
  capture: {{ type: Stdin, channels: 2, format: S16_LE }}
  playback: {{ type: Stdout, channels: 2, format: S16_LE }}
processors:
  character:
    type: PureroadCharacter
    parameters: {{ channels: 2, algorithm: Acceleration2, intensity: 0.5, mix: 1.0, transition_ms: 0.0 }}
pipeline:
  - {{ type: Processor, name: character }}
{volume_step}"#
            );
            let mut configuration: config::Configuration = yaml_serde::from_str(&yaml).unwrap();
            config::validate_config(&mut configuration, None).unwrap();
            let processing = std::sync::Arc::new(crate::ProcessingParameters::new(
                &[-20.0_f32; 5],
                &[false; 5],
            ));
            crate::pipeline::Pipeline::from_config(configuration, processing)
        }

        let left: Vec<PrcFmt> = (0..64)
            .map(|index| if index % 2 == 0 { 1.0 } else { -0.75 })
            .collect();
        let right: Vec<PrcFmt> = (0..64)
            .map(|index| if index % 3 == 0 { -1.0 } else { 0.5 })
            .collect();
        let mut legacy = make_pipeline(false);
        let mut explicit = make_pipeline(true);
        let legacy_output = legacy.process_chunk(chunk(left.clone(), right.clone()));
        let explicit_output = explicit.process_chunk(chunk(left, right));
        let max_difference = legacy_output
            .waveforms
            .iter()
            .flatten()
            .zip(explicit_output.waveforms.iter().flatten())
            .map(|(before, after)| (before - after).abs())
            .fold(0.0, PrcFmt::max);
        assert!(max_difference > 1e-10, "max_difference={max_difference:e}");
    }

    #[test]
    fn original_and_zero_mix_are_bit_exact() {
        let (left, right) = signal(4096, 192_000);
        for (algorithm, mix) in [
            (config::PureroadCharacterAlgorithm::Original, 1.0),
            (config::PureroadCharacterAlgorithm::Acceleration2, 0.0),
            (config::PureroadCharacterAlgorithm::ToTape8, 0.0),
        ] {
            let mut audio = chunk(left.clone(), right.clone());
            processor(192_000, 4096, algorithm, 1.0, mix, 0.0)
                .process_chunk(&mut audio)
                .unwrap();
            assert_eq!(audio.waveforms[0], left);
            assert_eq!(audio.waveforms[1], right);
        }
    }

    #[test]
    fn output_is_independent_of_chunk_partitioning() {
        let (left, right) = signal(4096, 96_000);
        let mut whole = chunk(left.clone(), right.clone());
        processor(
            96_000,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.77,
            1.0,
            0.0,
        )
        .process_chunk(&mut whole)
        .unwrap();
        let mut split_processor = processor(
            96_000,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.77,
            1.0,
            0.0,
        );
        let mut actual = [Vec::new(), Vec::new()];
        let mut offset = 0;
        for size in [64, 127, 256, 1024, 2048, 577] {
            if offset == 4096 {
                break;
            }
            let end = (offset + size).min(4096);
            let mut part = chunk(left[offset..end].to_vec(), right[offset..end].to_vec());
            split_processor.process_chunk(&mut part).unwrap();
            actual[0].extend(part.waveforms.remove(0));
            actual[1].extend(part.waveforms.remove(0));
            offset = end;
        }
        assert_eq!(whole.waveforms[0], actual[0]);
        assert_eq!(whole.waveforms[1], actual[1]);
    }

    #[test]
    fn all_supported_rates_and_extreme_parameters_stay_finite() {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            for intensity in [0.0, 0.32, 1.0] {
                let (left, right) = signal(8192, rate);
                let mut audio = chunk(left, right);
                processor(
                    rate,
                    8192,
                    config::PureroadCharacterAlgorithm::Acceleration2,
                    intensity,
                    1.0,
                    0.0,
                )
                .process_chunk(&mut audio)
                .unwrap();
                assert!(
                    audio
                        .waveforms
                        .iter()
                        .flatten()
                        .all(|value| value.is_finite())
                );
            }
        }
    }

    #[test]
    fn totape8_supported_rates_and_chunk_partitions_are_stable() {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            let (left, right) = signal(4096, rate);
            let mut whole = chunk(left.clone(), right.clone());
            processor(
                rate,
                4096,
                config::PureroadCharacterAlgorithm::ToTape8,
                0.0,
                1.0,
                0.0,
            )
            .process_chunk(&mut whole)
            .unwrap();
            assert!(
                whole
                    .waveforms
                    .iter()
                    .flatten()
                    .all(|value| value.is_finite())
            );

            let mut split = processor(
                rate,
                4096,
                config::PureroadCharacterAlgorithm::ToTape8,
                0.0,
                1.0,
                0.0,
            );
            let mut actual = [Vec::new(), Vec::new()];
            let mut offset = 0;
            for size in [64, 127, 256, 1024, 2048, 577] {
                if offset == 4096 {
                    break;
                }
                let end = (offset + size).min(4096);
                let mut part = chunk(left[offset..end].to_vec(), right[offset..end].to_vec());
                split.process_chunk(&mut part).unwrap();
                actual[0].extend(part.waveforms.remove(0));
                actual[1].extend(part.waveforms.remove(0));
                offset = end;
            }
            assert_eq!(whole.waveforms[0], actual[0]);
            assert_eq!(whole.waveforms[1], actual[1]);
        }
    }

    #[test]
    fn totape8_extreme_parameters_remain_safety_bounded() {
        let parameter_sets = [
            [0.0; 9],
            [0.5; 9],
            [1.0; 9],
            [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0],
        ];
        let mut random = SmallRng::seed_from_u64(0x5441_5045_424f_554e);
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for values in parameter_sets {
                let mut parameters =
                    params(config::PureroadCharacterAlgorithm::ToTape8, 0.0, 1.0, 0.0);
                parameters.totape8 = config::ToTape8Parameters {
                    input: to_prc(values[0]),
                    tilt: to_prc(values[1]),
                    shape: to_prc(values[2]),
                    flutter: to_prc(values[3]),
                    flutter_speed: to_prc(values[4]),
                    bias: to_prc(values[5]),
                    head_bump: to_prc(values[6]),
                    head_bump_frequency: to_prc(values[7]),
                    output: to_prc(values[8]),
                };
                let left = (0..16_384)
                    .map(|_| to_prc(random.random_range(-8.0..8.0)))
                    .collect();
                let right = (0..16_384)
                    .map(|_| to_prc(random.random_range(-8.0..8.0)))
                    .collect();
                let mut audio = chunk(left, right);
                PureroadCharacter::from_config("bounded", parameters, rate, 16_384)
                    .process_chunk(&mut audio)
                    .unwrap();
                let peak = audio
                    .waveforms
                    .iter()
                    .flatten()
                    .map(|sample| sample.abs())
                    .fold(0.0, PrcFmt::max);
                assert!(
                    peak <= to_prc(1.0),
                    "rate={rate} values={values:?} peak={peak}"
                );
            }
        }
    }

    #[test]
    fn totape8_spacing_sixteen_boundary_is_memory_safe_and_finite() {
        let rate = 705_600;
        let (left, right) = signal(4096, rate);
        let mut audio = chunk(left, right);
        processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        )
        .process_chunk(&mut audio)
        .unwrap();
        assert!(
            audio
                .waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn totape8_parameter_validation_rejects_non_finite_and_out_of_range() {
        let mut value = params(config::PureroadCharacterAlgorithm::ToTape8, 0.0, 1.0, 100.0);
        value.totape8.input = 1.1;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
        value.totape8.input = 0.5;
        value.totape8.bias = PrcFmt::NAN;
        assert!(validate_pureroad_character(&value, 48_000).is_err());
    }

    #[test]
    fn acceleration_to_totape8_switch_crossfades() {
        let rate = 48_000;
        let mut proc = processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            10.0,
        );
        let (left, right) = signal(2048, rate);
        proc.process_chunk(&mut chunk(left[..1024].to_vec(), right[..1024].to_vec()))
            .unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(config::PureroadCharacterAlgorithm::ToTape8, 0.32, 1.0, 10.0),
        });
        let mut transition = chunk(left[1024..].to_vec(), right[1024..].to_vec());
        proc.process_chunk(&mut transition).unwrap();
        assert!(
            transition
                .waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
        assert!(proc.previous_algorithm.is_none());
        let max_step = transition.waveforms[0]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, PrcFmt::max);
        assert!(max_step < 1.5);
    }

    #[test]
    fn rapid_algorithm_changes_queue_only_the_latest_request() {
        let rate = 48_000;
        let mut proc = processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            10.0,
        );
        let (left, right) = signal(4096, rate);
        proc.process_chunk(&mut chunk(left[..512].to_vec(), right[..512].to_vec()))
            .unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(config::PureroadCharacterAlgorithm::ToTape8, 0.32, 1.0, 10.0),
        });
        proc.process_chunk(&mut chunk(
            left[512..768].to_vec(),
            right[512..768].to_vec(),
        ))
        .unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(
                config::PureroadCharacterAlgorithm::Original,
                0.32,
                0.0,
                10.0,
            ),
        });
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(
                config::PureroadCharacterAlgorithm::Acceleration2,
                0.7,
                1.0,
                10.0,
            ),
        });
        assert_eq!(proc.algorithm, config::PureroadCharacterAlgorithm::ToTape8);
        assert_eq!(
            proc.pending_parameters.as_ref().unwrap().algorithm,
            config::PureroadCharacterAlgorithm::Acceleration2
        );
        let mut remainder = chunk(left[768..2048].to_vec(), right[768..2048].to_vec());
        proc.process_chunk(&mut remainder).unwrap();
        assert_eq!(
            proc.algorithm,
            config::PureroadCharacterAlgorithm::Acceleration2
        );
        assert!(
            remainder
                .waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn latest_request_for_current_transition_target_clears_stale_queue() {
        let rate = 48_000;
        let mut proc = processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            10.0,
        );
        let (left, right) = signal(2048, rate);
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(config::PureroadCharacterAlgorithm::ToTape8, 0.32, 1.0, 10.0),
        });
        proc.process_chunk(&mut chunk(left[..128].to_vec(), right[..128].to_vec()))
            .unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(
                config::PureroadCharacterAlgorithm::Original,
                0.32,
                0.0,
                10.0,
            ),
        });
        assert!(proc.pending_parameters.is_some());
        let mut latest = params(config::PureroadCharacterAlgorithm::ToTape8, 0.32, 1.0, 10.0);
        latest.totape8.input = 0.7;
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: latest,
        });
        assert!(proc.pending_parameters.is_none());
        proc.process_chunk(&mut chunk(
            left[128..1024].to_vec(),
            right[128..1024].to_vec(),
        ))
        .unwrap();
        assert_eq!(proc.algorithm, config::PureroadCharacterAlgorithm::ToTape8);
    }

    #[test]
    fn zero_mix_algorithm_transitions_and_pending_requests_complete() {
        let rate = 48_000;
        let mut proc = processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            0.0,
            10.0,
        );
        let mut audio = chunk(vec![0.4; 1024], vec![-0.4; 1024]);
        let expected = audio.waveforms.clone();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(config::PureroadCharacterAlgorithm::ToTape8, 0.32, 0.0, 10.0),
        });
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(
                config::PureroadCharacterAlgorithm::Original,
                0.32,
                0.0,
                10.0,
            ),
        });
        proc.process_chunk(&mut audio).unwrap();
        assert_eq!(audio.waveforms, expected);
        let mut second = chunk(vec![0.4; 1024], vec![-0.4; 1024]);
        let second_expected = second.waveforms.clone();
        proc.process_chunk(&mut second).unwrap();
        assert_eq!(second.waveforms, second_expected);
        assert_eq!(proc.algorithm, config::PureroadCharacterAlgorithm::Original);
        assert!(proc.previous_algorithm.is_none());
        assert!(proc.pending_parameters.is_none());
    }

    #[test]
    fn resuming_totape8_from_zero_mix_discards_pre_bypass_state() {
        let rate = 48_000;
        let frames = 2048;
        let mut wet = params(config::PureroadCharacterAlgorithm::ToTape8, 0.0, 1.0, 0.0);
        wet.totape8.flutter = 1.0;
        wet.totape8.flutter_speed = 1.0;
        wet.totape8.head_bump = 1.0;

        let mut resumed = PureroadCharacter::from_config("resumed", wet.clone(), rate, frames);
        resumed
            .process_chunk(&mut chunk(vec![0.8; frames], vec![-0.8; frames]))
            .unwrap();

        let mut dry = wet.clone();
        dry.mix = 0.0;
        resumed.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: dry,
        });
        resumed
            .process_chunk(&mut chunk(vec![0.2; frames], vec![-0.2; frames]))
            .unwrap();
        resumed.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: wet.clone(),
        });

        let mut actual = chunk(vec![0.0; frames], vec![0.0; frames]);
        resumed.process_chunk(&mut actual).unwrap();

        let mut fresh = PureroadCharacter::from_config("fresh", wet, rate, frames);
        let mut expected = chunk(vec![0.0; frames], vec![0.0; frames]);
        fresh.process_chunk(&mut expected).unwrap();
        assert_eq!(actual.waveforms, expected.waveforms);
    }

    #[test]
    fn default_resume_transition_starts_near_bit_exact_dry() {
        let rate = 48_000;
        let frames = 4096;
        let mut wet = params(config::PureroadCharacterAlgorithm::ToTape8, 0.0, 1.0, 100.0);
        wet.totape8.flutter = 1.0;
        wet.totape8.flutter_speed = 1.0;
        wet.totape8.head_bump = 1.0;
        let mut proc = PureroadCharacter::from_config("resume", wet.clone(), rate, frames);
        proc.process_chunk(&mut chunk(vec![0.8; frames], vec![-0.8; frames]))
            .unwrap();
        let mut dry = wet.clone();
        dry.mix = 0.0;
        dry.transition_ms = 0.0;
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: dry,
        });
        proc.process_chunk(&mut chunk(vec![0.2; frames], vec![-0.2; frames]))
            .unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: wet,
        });

        // Start away from a sine-wave zero crossing so the first-sample
        // assertion actually measures the dry/wet ramp.
        let left = vec![to_prc(0.8); frames];
        let right = vec![to_prc(-0.8); frames];
        let mut actual = chunk(left.clone(), right.clone());
        proc.process_chunk(&mut actual).unwrap();
        for channel in 0..2 {
            let dry = if channel == 0 { &left } else { &right };
            assert!(
                (actual.waveforms[channel][0] - dry[0]).abs() < to_prc(0.0005),
                "channel={channel} actual={} dry={}",
                actual.waveforms[channel][0],
                dry[0]
            );
        }
    }

    #[test]
    fn resuming_acceleration2_from_zero_mix_matches_fresh_state() {
        let rate = 96_000;
        let frames = 2048;
        let wet = params(
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.73,
            1.0,
            0.0,
        );
        let mut resumed = PureroadCharacter::from_config("resumed", wet.clone(), rate, frames);
        resumed
            .process_chunk(&mut chunk(vec![0.8; frames], vec![-0.8; frames]))
            .unwrap();
        let mut dry = wet.clone();
        dry.mix = 0.0;
        resumed.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: dry,
        });
        resumed
            .process_chunk(&mut chunk(vec![0.2; frames], vec![-0.2; frames]))
            .unwrap();
        resumed.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: wet.clone(),
        });
        let mut actual = chunk(vec![0.0; frames], vec![0.0; frames]);
        resumed.process_chunk(&mut actual).unwrap();

        let mut expected = chunk(vec![0.0; frames], vec![0.0; frames]);
        PureroadCharacter::from_config("fresh", wet, rate, frames)
            .process_chunk(&mut expected)
            .unwrap();
        assert_eq!(actual.waveforms, expected.waveforms);
    }

    #[test]
    #[ignore = "objective character report; run explicitly with --nocapture"]
    fn objective_character_diagnostic() {
        let rate = 48_000;
        let frames = rate * 2;
        let analysis_start = rate;
        let input = tone(frames, rate, 1_000.0, 0.25);

        let mut tape = chunk(input.clone(), input.clone());
        processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        )
        .process_chunk(&mut tape)
        .unwrap();
        let tape_samples = &tape.waveforms[0][analysis_start..];
        let fundamental = amplitude_at(tape_samples, rate, 1_000.0);
        let harmonics: Vec<f64> = (2..=10)
            .map(|harmonic| {
                db_ratio(
                    amplitude_at(tape_samples, rate, 1_000.0 * harmonic as f64),
                    fundamental,
                )
            })
            .collect();
        let tape_peak = tape_samples
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0, PrcFmt::max);
        let tape_gain = db_ratio(rms(tape_samples), rms(&input[analysis_start..]));
        eprintln!(
            "ToTape8 default: gain={tape_gain:.3} dB peak={tape_peak:.6} harmonics(2..10)dBc={harmonics:?}"
        );
        assert!(tape_peak <= to_prc(1.0));
        assert!(fundamental > 0.01);

        for intensity in [0.1, 0.2, 0.32] {
            let mut gains = Vec::new();
            for frequency in [1_000.0, 5_000.0, 10_000.0, 15_000.0, 19_000.0] {
                let input = tone(frames, rate, frequency, 0.25);
                let mut audio = chunk(input.clone(), input.clone());
                processor(
                    rate,
                    frames,
                    config::PureroadCharacterAlgorithm::Acceleration2,
                    to_prc(intensity),
                    1.0,
                    0.0,
                )
                .process_chunk(&mut audio)
                .unwrap();
                gains.push(db_ratio(
                    rms(&audio.waveforms[0][analysis_start..]),
                    rms(&input[analysis_start..]),
                ));
            }
            eprintln!("Acceleration2 intensity={intensity:.2}: gain@1/5/10/15/19kHz={gains:?} dB");
        }
    }

    #[test]
    fn default_character_profiles_have_expected_objective_shape() {
        let rate = 48_000;
        let frames = rate;
        let analysis_start = rate / 2;

        let tape_input = tone(frames, rate, 1_000.0, 0.25);
        let mut tape = chunk(tape_input.clone(), tape_input.clone());
        processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        )
        .process_chunk(&mut tape)
        .unwrap();
        let tape_samples = &tape.waveforms[0][analysis_start..];
        let tape_fundamental = amplitude_at(tape_samples, rate, 1_000.0);
        let tape_gain = db_ratio(rms(tape_samples), rms(&tape_input[analysis_start..]));
        let tape_third_harmonic =
            db_ratio(amplitude_at(tape_samples, rate, 3_000.0), tape_fundamental);
        assert!(
            (-0.2..=0.1).contains(&tape_gain),
            "unexpected default ToTape8 gain: {tape_gain} dB"
        );
        assert!(
            (-55.0..=-40.0).contains(&tape_third_harmonic),
            "unexpected default ToTape8 third harmonic: {tape_third_harmonic} dBc"
        );

        let acceleration_gain = |frequency: f64| {
            let input = tone(frames, rate, frequency, 0.25);
            let mut audio = chunk(input.clone(), input.clone());
            processor(
                rate,
                frames,
                config::PureroadCharacterAlgorithm::Acceleration2,
                to_prc(0.32),
                1.0,
                0.0,
            )
            .process_chunk(&mut audio)
            .unwrap();
            db_ratio(
                rms(&audio.waveforms[0][analysis_start..]),
                rms(&input[analysis_start..]),
            )
        };
        let gain_1k = acceleration_gain(1_000.0);
        let gain_10k = acceleration_gain(10_000.0);
        let gain_19k = acceleration_gain(19_000.0);
        assert!(
            gain_1k.abs() < 0.05,
            "Acceleration2 should leave 1 kHz essentially flat: {gain_1k} dB"
        );
        assert!(
            (-2.0..=-0.5).contains(&gain_10k),
            "Acceleration2 10 kHz profile drifted: {gain_10k} dB"
        );
        assert!(
            (-4.0..=-1.5).contains(&gain_19k),
            "Acceleration2 19 kHz profile drifted: {gain_19k} dB"
        );
    }

    #[test]
    fn non_finite_chunk_is_bypassed_and_does_not_poison_state() {
        let mut proc = processor(
            48_000,
            8,
            config::PureroadCharacterAlgorithm::Acceleration2,
            1.0,
            1.0,
            0.0,
        );
        let mut bad = chunk(vec![PrcFmt::NAN, 0.2], vec![PrcFmt::INFINITY, -0.2]);
        proc.process_chunk(&mut bad).unwrap();
        assert!(bad.waveforms[0][0].is_nan());
        assert!(bad.waveforms[1][0].is_infinite());
        let mut good = chunk(vec![0.1; 8], vec![-0.1; 8]);
        proc.process_chunk(&mut good).unwrap();
        let mut expected = chunk(vec![0.1; 8], vec![-0.1; 8]);
        processor(
            48_000,
            8,
            config::PureroadCharacterAlgorithm::Acceleration2,
            1.0,
            1.0,
            0.0,
        )
        .process_chunk(&mut expected)
        .unwrap();
        assert_eq!(good.waveforms, expected.waveforms);
        assert!(
            good.waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn structural_errors_fail_open_without_panicking() {
        let mut proc = processor(
            48_000,
            4,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.5,
            1.0,
            0.0,
        );
        let mut oversized = chunk(vec![0.2; 5], vec![-0.2; 5]);
        let original = oversized.waveforms.clone();
        proc.process_chunk(&mut oversized).unwrap();
        assert_eq!(oversized.waveforms, original);

        let mut mismatch = AudioChunk::new(vec![vec![0.2; 4]], 0.0, 0.0, 4, 4);
        let original = mismatch.waveforms.clone();
        proc.process_chunk(&mut mismatch).unwrap();
        assert_eq!(mismatch.waveforms, original);

        proc.update_parameters(config::Processor::RACE {
            description: None,
            parameters: config::RACEParameters {
                channels: 2,
                channel_a: 0,
                channel_b: 1,
                delay: 0.0,
                subsample_delay: None,
                delay_unit: None,
                attenuation: 0.0,
            },
        });
        assert_eq!(proc.processing_errors, 3);
    }

    #[test]
    fn processor_type_change_rebuilds_pipeline() {
        let base_yaml = r#"
devices:
  samplerate: 48000
  chunksize: 64
  capture: { type: Stdin, channels: 2, format: S16_LE }
  playback: { type: Stdout, channels: 2, format: S16_LE }
processors:
  same_name:
    type: PureroadCharacter
    parameters: { channels: 2, algorithm: Acceleration2, intensity: 0.32, mix: 1.0 }
pipeline:
  - { type: Processor, name: same_name }
"#;
        let current: config::Configuration = yaml_serde::from_str(base_yaml).unwrap();
        let mut changed = current.clone();
        let replacement: config::Processor = yaml_serde::from_str(
            r#"
type: RACE
parameters:
  channels: 2
  channel_a: 0
  channel_b: 1
  delay: 0.1
  attenuation: 6.0
"#,
        )
        .unwrap();
        changed
            .processors
            .as_mut()
            .unwrap()
            .insert("same_name".to_owned(), replacement);
        assert!(matches!(
            config::config_diff(&current, &changed),
            config::ConfigChange::Pipeline
        ));
    }

    #[test]
    fn intensity_update_is_smoothed_from_the_previous_state() {
        let rate = 48_000;
        let make = || {
            processor(
                rate,
                256,
                config::PureroadCharacterAlgorithm::Acceleration2,
                0.0,
                1.0,
                10.0,
            )
        };
        let mut ramped = make();
        let mut unchanged = make();
        let (left, right) = signal(256, rate);
        ramped
            .process_chunk(&mut chunk(left.clone(), right.clone()))
            .unwrap();
        unchanged.process_chunk(&mut chunk(left, right)).unwrap();
        ramped.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(
                config::PureroadCharacterAlgorithm::Acceleration2,
                1.0,
                1.0,
                10.0,
            ),
        });
        let left: Vec<PrcFmt> = (0..256)
            .map(|index| if index % 2 == 0 { 0.8 } else { -0.8 })
            .collect();
        let right: Vec<PrcFmt> = (0..256)
            .map(|index| if index % 3 == 0 { -0.8 } else { 0.6 })
            .collect();
        let mut ramped_next = chunk(left.clone(), right.clone());
        let mut unchanged_next = chunk(left, right);
        ramped.process_chunk(&mut ramped_next).unwrap();
        unchanged.process_chunk(&mut unchanged_next).unwrap();
        assert!((ramped_next.waveforms[0][0] - unchanged_next.waveforms[0][0]).abs() < 1e-4);
        assert_ne!(ramped_next.waveforms, unchanged_next.waveforms);
    }

    #[test]
    fn update_ramps_to_exact_bypass() {
        let rate = 96_000;
        let (left, right) = signal(4096, rate);
        let mut proc = processor(
            rate,
            4096,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.8,
            1.0,
            1.0,
        );
        let mut prime = chunk(left[..512].to_vec(), right[..512].to_vec());
        proc.process_chunk(&mut prime).unwrap();
        proc.update_parameters(config::Processor::PureroadCharacter {
            description: None,
            parameters: params(config::PureroadCharacterAlgorithm::Original, 0.8, 0.0, 1.0),
        });
        let dry_left = left[512..].to_vec();
        let dry_right = right[512..].to_vec();
        let mut audio = chunk(dry_left.clone(), dry_right.clone());
        proc.process_chunk(&mut audio).unwrap();
        assert_eq!(&audio.waveforms[0][96..], &dry_left[96..]);
        assert_eq!(&audio.waveforms[1][96..], &dry_right[96..]);
    }

    #[test]
    fn deterministic_golden_prefix() {
        let mut audio = chunk(
            vec![0.25, -0.5, 0.75, -0.125],
            vec![-0.25, 0.5, -0.75, 0.125],
        );
        processor(
            48_000,
            4,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            0.0,
        )
        .process_chunk(&mut audio)
        .unwrap();
        #[cfg(not(feature = "32bit"))]
        {
            let expected_l = [
                0.1660179931694639,
                -0.14420571744157262,
                0.1565725878731523,
                0.5044080801070826,
            ];
            for (actual, expected) in audio.waveforms[0].iter().zip(expected_l) {
                assert!(
                    (actual - expected).abs() < 1e-14,
                    "actual={actual:.17} expected={expected:.17}"
                );
            }
        }
        #[cfg(feature = "32bit")]
        assert!(
            audio
                .waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    fn totape8_matches_official_double_callback_golden() {
        let left = (0..64)
            .map(|n| {
                let t = n as f64 / 96_000.0;
                to_prc(
                    0.123
                        + 0.41 * (std::f64::consts::TAU * 997.0 * t).sin()
                        + 0.17 * (std::f64::consts::TAU * 13_117.0 * t).sin(),
                )
            })
            .collect();
        let right = (0..64)
            .map(|n| {
                let t = n as f64 / 96_000.0;
                to_prc(
                    0.37 * (std::f64::consts::TAU * 503.0 * t).cos()
                        - 0.13 * (std::f64::consts::TAU * 17_003.0 * t).sin(),
                )
            })
            .collect();
        let mut audio = chunk(left, right);
        processor(
            96_000,
            64,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        )
        .process_chunk(&mut audio)
        .unwrap();
        #[cfg(not(feature = "32bit"))]
        {
            let expected_left = [
                0.0,
                0.0,
                -0.05908256659145218,
                -0.18419984383210958,
                -0.07889736905295641,
                0.05541077750182544,
                0.04236869836318852,
                0.2534918171136649,
                0.32096349274335834,
                0.3096514985248335,
                0.32560605553138255,
                0.33539318250750105,
                0.33984280289756386,
                0.35690509657782143,
                0.36338000005254556,
                0.3608659931963123,
            ];
            let expected_right = [
                0.0,
                0.0,
                -0.17727930544085579,
                -0.18229415441372665,
                -0.04943299197252582,
                -0.042215528551326265,
                0.13689331241055747,
                0.22116008011354304,
                0.3410533928926902,
                0.4591154306775859,
                0.49733091486781683,
                0.47998531364513203,
                0.4679393986068464,
                0.4361397818920294,
                0.3599016865171621,
                0.3169977074617095,
            ];
            for frame in 0..expected_left.len() {
                assert!(
                    (audio.waveforms[0][frame] - expected_left[frame]).abs() < 2e-14,
                    "left frame {frame}: actual={} expected={}",
                    audio.waveforms[0][frame],
                    expected_left[frame]
                );
                assert!(
                    (audio.waveforms[1][frame] - expected_right[frame]).abs() < 2e-14,
                    "right frame {frame}: actual={} expected={}",
                    audio.waveforms[1][frame],
                    expected_right[frame]
                );
            }
        }
        #[cfg(feature = "32bit")]
        assert!(
            audio
                .waveforms
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }

    #[test]
    #[cfg(not(feature = "32bit"))]
    fn totape8_nondefault_parameters_match_official_golden() {
        let left = (0..64)
            .map(|n| {
                let t = n as f64 / 96_000.0;
                0.123
                    + 0.41 * (std::f64::consts::TAU * 997.0 * t).sin()
                    + 0.17 * (std::f64::consts::TAU * 13_117.0 * t).sin()
            })
            .collect();
        let right = (0..64)
            .map(|n| {
                let t = n as f64 / 96_000.0;
                0.37 * (std::f64::consts::TAU * 503.0 * t).cos()
                    - 0.13 * (std::f64::consts::TAU * 17_003.0 * t).sin()
            })
            .collect();
        let mut parameters = params(config::PureroadCharacterAlgorithm::ToTape8, 0.0, 1.0, 0.0);
        parameters.totape8 = config::ToTape8Parameters {
            input: 0.7,
            tilt: 0.8,
            shape: 0.3,
            flutter: 0.7,
            flutter_speed: 0.6,
            bias: 0.3,
            head_bump: 0.8,
            head_bump_frequency: 0.7,
            output: 0.4,
        };
        let mut proc = PureroadCharacter::from_config("golden", parameters, 96_000, 64);
        let mut audio = chunk(left, right);
        proc.process_chunk(&mut audio).unwrap();
        let expected_left = [
            0.033883537966439864,
            0.09302481173399588,
            0.2510462259979344,
            0.4219666454191225,
            0.44491273048740626,
            0.16809271675742252,
            0.22696338701998142,
            0.2789659041781755,
            0.4283705361693388,
            0.5380000630179732,
            0.6370771085114862,
            0.6452540587045286,
            0.41709415491109336,
            0.4094270972511281,
            0.4281576647377495,
            0.6052139836765706,
            0.6825368168426849,
            0.7479417391255544,
            0.7599838627592659,
        ];
        let expected_right = [
            0.045253376676258575,
            0.13887778103085302,
            0.2972163299632242,
            0.479444017198837,
            0.582971625881849,
            0.6828763167445717,
            0.5089707334684175,
            0.37845369118577404,
            0.41136032105207326,
            0.4816379959557568,
            0.6084938931598901,
            0.5648921500184202,
            0.4229072667993281,
            0.39410394568108764,
            0.39276958797758327,
            0.5628325707246028,
            0.5962301432198784,
            0.4985943101330154,
            0.34554598184550095,
        ];
        for (offset, (&left, &right)) in expected_left.iter().zip(&expected_right).enumerate() {
            let frame = offset + 13;
            assert!(
                (audio.waveforms[0][frame] - left).abs() < 1e-10,
                "left {frame}: {} vs {left}",
                audio.waveforms[0][frame]
            );
            assert!(
                (audio.waveforms[1][frame] - right).abs() < 1e-10,
                "right {frame}: {} vs {right}",
                audio.waveforms[1][frame]
            );
        }
    }

    #[test]
    fn native_kernel_matches_independent_upstream_style_reference() {
        let mut random = SmallRng::seed_from_u64(0x5055_5245_524f_4144);
        #[cfg(not(feature = "32bit"))]
        let tolerance = 2e-14;
        #[cfg(feature = "32bit")]
        let tolerance = 2e-6;
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for intensity in [0.0, 0.17, 0.32, 0.73, 1.0] {
                let left: Vec<PrcFmt> = (0..4096)
                    .map(|_| to_prc(random.random_range(-1.5..1.5)))
                    .collect();
                let right: Vec<PrcFmt> = (0..4096)
                    .map(|_| to_prc(random.random_range(-1.5..1.5)))
                    .collect();
                let mut expected = [left.clone(), right.clone()];
                UpstreamStyleReference::new(rate).process(&mut expected, intensity);
                let mut actual = chunk(left, right);
                processor(
                    rate,
                    4096,
                    config::PureroadCharacterAlgorithm::Acceleration2,
                    to_prc(intensity),
                    1.0,
                    0.0,
                )
                .process_chunk(&mut actual)
                .unwrap();
                for channel in 0..2 {
                    for frame in 0..4096 {
                        let delta =
                            (actual.waveforms[channel][frame] - expected[channel][frame]).abs();
                        assert!(
                            delta <= tolerance,
                            "rate={rate} intensity={intensity} channel={channel} frame={frame} delta={delta:e}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn million_sample_randomized_stress_remains_finite() {
        let rate = 192_000;
        let frames = 4096;
        let mut random = SmallRng::seed_from_u64(0x5354_5245_5353);
        let mut proc = processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::ToTape8,
            1.0,
            1.0,
            0.0,
        );
        for _ in 0..256 {
            let left = (0..frames)
                .map(|_| to_prc(random.random_range(-4.0..4.0)))
                .collect();
            let right = (0..frames)
                .map(|_| to_prc(random.random_range(-4.0..4.0)))
                .collect();
            let mut audio = chunk(left, right);
            proc.process_chunk(&mut audio).unwrap();
            assert!(
                audio
                    .waveforms
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite())
            );
        }
    }

    #[test]
    fn totape8_half_million_sample_stress_remains_finite() {
        let mut random = SmallRng::seed_from_u64(0x5441_5045_5354_5245);
        let mut proc = processor(
            192_000,
            4096,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        );
        for _ in 0..128 {
            let left = (0..4096)
                .map(|_| to_prc(random.random_range(-2.0..2.0)))
                .collect();
            let right = (0..4096)
                .map(|_| to_prc(random.random_range(-2.0..2.0)))
                .collect();
            let mut audio = chunk(left, right);
            proc.process_chunk(&mut audio).unwrap();
            assert!(
                audio
                    .waveforms
                    .iter()
                    .flatten()
                    .all(|sample| sample.is_finite())
            );
        }
    }

    #[test]
    #[ignore = "host-specific realtime diagnostic; run explicitly in release mode"]
    fn totape8_host_realtime_budget_diagnostic() {
        let rate = 192_000;
        let frames = 4096;
        let iterations = 512;
        let (left, right) = signal(frames, rate);
        let mut proc = processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::ToTape8,
            0.0,
            1.0,
            0.0,
        );
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let mut audio = chunk(left.clone(), right.clone());
            proc.process_chunk(&mut audio).unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();
        let audio_duration = iterations as f64 * frames as f64 / rate as f64;
        eprintln!(
            "ToTape8 processed {audio_duration:.3}s audio in {elapsed:.6}s ({:.3}% realtime budget)",
            elapsed / audio_duration * 100.0
        );
        assert!(elapsed < audio_duration);
    }

    #[test]
    #[ignore = "host-specific worst-case diagnostic; run explicitly in release mode"]
    fn totape8_sweep_and_crossfade_realtime_budget_diagnostic() {
        let rate = 192_000;
        let frames = 4096;
        let iterations = 128;
        let (left, right) = signal(frames, rate);
        let mut proc = processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::Acceleration2,
            0.32,
            1.0,
            20.0,
        );
        let start = std::time::Instant::now();
        for iteration in 0..iterations {
            if iteration == 32 || iteration == 40 || iteration % 2 == 0 {
                let mut next = params(
                    if iteration == 32 {
                        config::PureroadCharacterAlgorithm::Acceleration2
                    } else {
                        config::PureroadCharacterAlgorithm::ToTape8
                    },
                    if iteration % 16 == 0 { 1.0 } else { 0.1 },
                    1.0,
                    20.0,
                );
                let edge = if iteration % 16 == 0 { 0.95 } else { 0.05 };
                next.totape8 = config::ToTape8Parameters {
                    input: edge,
                    tilt: 1.0 - edge,
                    shape: edge,
                    flutter: edge,
                    flutter_speed: 1.0 - edge,
                    bias: edge,
                    head_bump: edge,
                    head_bump_frequency: 1.0 - edge,
                    output: edge,
                };
                proc.update_parameters(config::Processor::PureroadCharacter {
                    description: None,
                    parameters: next,
                });
            }
            proc.process_chunk(&mut chunk(left.clone(), right.clone()))
                .unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();
        let audio_duration = iterations as f64 * frames as f64 / rate as f64;
        eprintln!("worst-case processed {audio_duration:.3}s in {elapsed:.6}s");
        assert!(elapsed < audio_duration);
    }

    #[test]
    #[ignore = "host-specific realtime diagnostic; run explicitly in release mode"]
    fn host_realtime_budget_diagnostic() {
        let rate = 192_000;
        let frames = 4096;
        let iterations = 512;
        let (left, right) = signal(frames, rate);
        let mut proc = processor(
            rate,
            frames,
            config::PureroadCharacterAlgorithm::Acceleration2,
            1.0,
            1.0,
            0.0,
        );
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let mut audio = chunk(left.clone(), right.clone());
            proc.process_chunk(&mut audio).unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();
        let audio_duration = iterations as f64 * frames as f64 / rate as f64;
        eprintln!(
            "processed {audio_duration:.3}s audio in {elapsed:.6}s ({:.3}% realtime budget)",
            elapsed / audio_duration * 100.0
        );
        assert!(elapsed < audio_duration);
    }
}
