use crate::audiodevice::AudioChunk;
use crate::config;
use crate::recycle_container;
use crate::vec_from_stash;
use crate::PrcFmt;
use audioadapter::direct::{InterleavedSlice, SequentialSliceOfVecs, SparseSequentialSliceOfVecs};
use rubato::{
    calculate_cutoff, Async, Fft, FixedAsync, FixedSync, Indexing, PolynomialDegree, Resampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub struct ChunkResampler {
    pub resampler: Box<dyn Resampler<PrcFmt>>,
    pub indexing: Indexing,
}

pub fn resampler_is_async(conf: &Option<config::Resampler>) -> bool {
    matches!(
        &conf,
        Some(config::Resampler::AsyncSinc { .. }) | Some(config::Resampler::AsyncPoly { .. })
    )
}

pub fn new_async_sinc_parameters(
    resampler_conf: &config::AsyncSincParameters,
) -> SincInterpolationParameters {
    match &resampler_conf {
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::VeryFast,
        } => {
            let sinc_len = 64;
            let oversampling_factor = 1024;
            let interpolation = SincInterpolationType::Linear;
            let window = WindowFunction::Hann2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Fast,
        } => {
            let sinc_len = 128;
            let oversampling_factor = 1024;
            let interpolation = SincInterpolationType::Linear;
            let window = WindowFunction::Blackman2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Balanced,
        } => {
            let sinc_len = 192;
            let oversampling_factor = 512;
            let interpolation = SincInterpolationType::Quadratic;
            let window = WindowFunction::BlackmanHarris2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Profile {
            profile: config::AsyncSincProfile::Accurate,
        } => {
            let sinc_len = 256;
            let oversampling_factor = 256;
            let interpolation = SincInterpolationType::Cubic;
            let window = WindowFunction::BlackmanHarris2;
            let f_cutoff = calculate_cutoff(sinc_len, window);
            SincInterpolationParameters {
                sinc_len,
                f_cutoff,
                oversampling_factor,
                interpolation,
                window,
            }
        }
        config::AsyncSincParameters::Free {
            sinc_len,
            window,
            f_cutoff,
            interpolation,
            oversampling_factor,
        } => {
            let interpolation = match interpolation {
                config::AsyncSincInterpolation::Nearest => SincInterpolationType::Nearest,
                config::AsyncSincInterpolation::Linear => SincInterpolationType::Linear,
                config::AsyncSincInterpolation::Quadratic => SincInterpolationType::Quadratic,
                config::AsyncSincInterpolation::Cubic => SincInterpolationType::Cubic,
            };

            let wind = match window {
                config::AsyncSincWindow::Hann => WindowFunction::Hann,
                config::AsyncSincWindow::Hann2 => WindowFunction::Hann2,
                config::AsyncSincWindow::Blackman => WindowFunction::Blackman,
                config::AsyncSincWindow::Blackman2 => WindowFunction::Blackman2,
                config::AsyncSincWindow::BlackmanHarris => WindowFunction::BlackmanHarris,
                config::AsyncSincWindow::BlackmanHarris2 => WindowFunction::BlackmanHarris2,
            };
            let cutoff = if let Some(co) = f_cutoff {
                *co
            } else {
                calculate_cutoff(*sinc_len, wind)
            };
            SincInterpolationParameters {
                sinc_len: *sinc_len,
                f_cutoff: cutoff,
                oversampling_factor: *oversampling_factor,
                interpolation,
                window: wind,
            }
        }
    }
}

pub fn new_resampler(
    resampler_conf: &Option<config::Resampler>,
    num_channels: usize,
    samplerate: usize,
    capture_samplerate: usize,
    chunksize: usize,
) -> Option<ChunkResampler> {
    let indexing = Indexing {
        input_offset: 0,
        output_offset: 0,
        partial_len: None,
        active_channels_mask: Some(vec![true; num_channels]),
    };
    match &resampler_conf {
        Some(config::Resampler::AsyncSinc(parameters)) => {
            let sinc_params = new_async_sinc_parameters(parameters);
            debug!(
                "Creating asynchronous resampler with parameters: {:?}",
                sinc_params
            );
            Some(ChunkResampler {
                resampler: Box::new(
                    Async::<PrcFmt>::new_sinc(
                        samplerate as f64 / capture_samplerate as f64,
                        1.1,
                        sinc_params,
                        chunksize,
                        num_channels,
                        FixedAsync::Output,
                    )
                    .unwrap(),
                ),
                indexing,
            })
        }
        Some(config::Resampler::AsyncPoly { interpolation }) => {
            let degree = match interpolation {
                config::AsyncPolyInterpolation::Linear => PolynomialDegree::Linear,
                config::AsyncPolyInterpolation::Cubic => PolynomialDegree::Cubic,
                config::AsyncPolyInterpolation::Quintic => PolynomialDegree::Quintic,
                config::AsyncPolyInterpolation::Septic => PolynomialDegree::Septic,
            };
            Some(ChunkResampler {
                resampler: Box::new(
                    Async::<PrcFmt>::new_poly(
                        samplerate as f64 / capture_samplerate as f64,
                        1.1,
                        degree,
                        chunksize,
                        num_channels,
                        FixedAsync::Output,
                    )
                    .unwrap(),
                ),
                indexing,
            })
        }
        Some(config::Resampler::Synchronous) => Some(ChunkResampler {
            resampler: Box::new(
                Fft::<PrcFmt>::new(
                    capture_samplerate,
                    samplerate,
                    chunksize,
                    2,
                    num_channels,
                    FixedSync::Output,
                )
                .unwrap(),
            ),
            indexing,
        }),
        None => None,
    }
}

impl ChunkResampler {
    pub fn resample_chunk(&mut self, chunk: &mut AudioChunk, chunksize: usize, channels: usize) {
        chunk.update_channel_mask(self.indexing.active_channels_mask.as_mut().unwrap());
        let mut new_waves = Vec::with_capacity(channels);
        for wave in &chunk.waveforms {
            if !wave.is_empty() {
                new_waves.push(vec_from_stash(chunksize));
            } else {
                new_waves.push(vec_from_stash(0));
            }
        }
        let adapter_in = SparseSequentialSliceOfVecs::new(
            &chunk.waveforms,
            channels,
            chunk.frames,
            self.indexing.active_channels_mask.as_ref().unwrap(),
        )
        .unwrap();
        let mut adapter_out = SparseSequentialSliceOfVecs::new_mut(
            &mut new_waves,
            channels,
            chunksize,
            self.indexing.active_channels_mask.as_ref().unwrap(),
        )
        .unwrap();
        self.resampler
            .process_into_buffer(&adapter_in, &mut adapter_out, Some(&self.indexing))
            .unwrap();
        if chunk.valid_frames < chunk.frames {
            chunk.valid_frames = (chunksize * chunk.valid_frames) / chunk.frames;
        } else {
            chunk.valid_frames = chunksize;
        }
        chunk.frames = chunksize;
        // swap old and new
        std::mem::swap(&mut new_waves, &mut chunk.waveforms);
        recycle_container(new_waves);
    }

    pub fn pump_silence(&mut self, channels: usize, chunksize: usize) -> Vec<Vec<PrcFmt>> {
        let mut new_waves = Vec::with_capacity(channels);
        for _ in 0..channels {
            new_waves.push(vec![0.0; chunksize]);
        }
        let dummy_data = Vec::new();
        let adapter_in = InterleavedSlice::new(&dummy_data, channels, 0).unwrap();
        let mut adapter_out =
            SequentialSliceOfVecs::new_mut(&mut new_waves, channels, chunksize).unwrap();
        // create earlier and reuse
        let indexing = rubato::Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(0),
            active_channels_mask: None,
        };
        self.resampler
            .process_into_buffer(&adapter_in, &mut adapter_out, Some(&indexing))
            .unwrap();
        new_waves
    }
}
