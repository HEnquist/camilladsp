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
use crate::filters;
use crate::filters::Filter;
use num_complex::Complex;
use num_traits::Zero;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

// Sample format
use crate::CamillaFloat;
use crate::Res;
use crate::ToCamillaFloat;

#[cfg(target_arch = "aarch64")]
#[path = "fftconv_neon.rs"]
mod neon;

#[cfg(target_arch = "x86_64")]
#[path = "fftconv_avx.rs"]
mod avx;

// element-wise product, result = slice_a * slice_b
#[cfg(any(not(target_arch = "aarch64"), test, feature = "bench"))]
fn multiply_elements_scalar<T: num_traits::Float>(
    result: &mut [Complex<T>],
    slice_a: &[Complex<T>],
    slice_b: &[Complex<T>],
) {
    let len = result.len();
    let mut res = &mut result[..len];
    let mut val_a = &slice_a[..len];
    let mut val_b = &slice_b[..len];

    unsafe {
        while res.len() >= 8 {
            *res.get_unchecked_mut(0) = *val_a.get_unchecked(0) * *val_b.get_unchecked(0);
            *res.get_unchecked_mut(1) = *val_a.get_unchecked(1) * *val_b.get_unchecked(1);
            *res.get_unchecked_mut(2) = *val_a.get_unchecked(2) * *val_b.get_unchecked(2);
            *res.get_unchecked_mut(3) = *val_a.get_unchecked(3) * *val_b.get_unchecked(3);
            *res.get_unchecked_mut(4) = *val_a.get_unchecked(4) * *val_b.get_unchecked(4);
            *res.get_unchecked_mut(5) = *val_a.get_unchecked(5) * *val_b.get_unchecked(5);
            *res.get_unchecked_mut(6) = *val_a.get_unchecked(6) * *val_b.get_unchecked(6);
            *res.get_unchecked_mut(7) = *val_a.get_unchecked(7) * *val_b.get_unchecked(7);
            res = &mut res[8..];
            val_a = val_a.get_unchecked(8..);
            val_b = val_b.get_unchecked(8..);
        }
    }
    for (r, val) in res
        .iter_mut()
        .zip(val_a.iter().zip(val_b.iter()).map(|(a, b)| *a * *b))
    {
        *r = val;
    }
}

// element-wise add product, result = result + slice_a * slice_b
#[cfg(any(not(target_arch = "aarch64"), test, feature = "bench"))]
fn multiply_add_elements_scalar<T: num_traits::Float + num_traits::NumAssign>(
    result: &mut [Complex<T>],
    slice_a: &[Complex<T>],
    slice_b: &[Complex<T>],
) {
    let len = result.len();
    let mut res = &mut result[..len];
    let mut val_a = &slice_a[..len];
    let mut val_b = &slice_b[..len];

    unsafe {
        while res.len() >= 8 {
            *res.get_unchecked_mut(0) += *val_a.get_unchecked(0) * *val_b.get_unchecked(0);
            *res.get_unchecked_mut(1) += *val_a.get_unchecked(1) * *val_b.get_unchecked(1);
            *res.get_unchecked_mut(2) += *val_a.get_unchecked(2) * *val_b.get_unchecked(2);
            *res.get_unchecked_mut(3) += *val_a.get_unchecked(3) * *val_b.get_unchecked(3);
            *res.get_unchecked_mut(4) += *val_a.get_unchecked(4) * *val_b.get_unchecked(4);
            *res.get_unchecked_mut(5) += *val_a.get_unchecked(5) * *val_b.get_unchecked(5);
            *res.get_unchecked_mut(6) += *val_a.get_unchecked(6) * *val_b.get_unchecked(6);
            *res.get_unchecked_mut(7) += *val_a.get_unchecked(7) * *val_b.get_unchecked(7);
            res = &mut res[8..];
            val_a = val_a.get_unchecked(8..);
            val_b = val_b.get_unchecked(8..);
        }
    }
    for (r, val) in res
        .iter_mut()
        .zip(val_a.iter().zip(val_b.iter()).map(|(a, b)| *a * *b))
    {
        *r += val;
    }
}

/// Complex multiply and multiply-accumulate kernels for the convolution inner loop.
///
/// Implemented for both float precisions rather than selected by `cfg`, so that
/// whichever one `CamillaFloat` is not currently set to still gets compiled and tested
/// on every build. The unused implementation is dead code that the linker drops,
/// so the binary is unaffected.
///
/// Each implementation dispatches to NEON (aarch64), AVX+FMA (x86_64 with runtime
/// support), or the scalar fallback.
pub(super) trait ConvKernel: Sized {
    /// result = slice_a * slice_b
    fn multiply_elements(
        result: &mut [Complex<Self>],
        slice_a: &[Complex<Self>],
        slice_b: &[Complex<Self>],
    );

    /// result += slice_a * slice_b
    fn multiply_add_elements(
        result: &mut [Complex<Self>],
        slice_a: &[Complex<Self>],
        slice_b: &[Complex<Self>],
    );
}

macro_rules! impl_conv_kernel {
    ($t:ty, $neon_mul:ident, $neon_mul_add:ident, $avx_mul:ident, $avx_mul_add:ident) => {
        impl ConvKernel for $t {
            #[inline]
            fn multiply_elements(
                result: &mut [Complex<$t>],
                slice_a: &[Complex<$t>],
                slice_b: &[Complex<$t>],
            ) {
                #[cfg(target_arch = "aarch64")]
                {
                    // SAFETY: NEON is mandatory on all AArch64 implementations.
                    unsafe { neon::$neon_mul(result, slice_a, slice_b) };
                }

                #[cfg(target_arch = "x86_64")]
                {
                    if avx::has_avx_fma() {
                        // SAFETY: AVX and FMA support has been verified by has_avx_fma().
                        unsafe { avx::$avx_mul(result, slice_a, slice_b) };
                    } else {
                        multiply_elements_scalar(result, slice_a, slice_b);
                    }
                }

                #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
                multiply_elements_scalar(result, slice_a, slice_b);
            }

            #[inline]
            fn multiply_add_elements(
                result: &mut [Complex<$t>],
                slice_a: &[Complex<$t>],
                slice_b: &[Complex<$t>],
            ) {
                #[cfg(target_arch = "aarch64")]
                {
                    // SAFETY: NEON is mandatory on all AArch64 implementations.
                    unsafe { neon::$neon_mul_add(result, slice_a, slice_b) };
                }

                #[cfg(target_arch = "x86_64")]
                {
                    if avx::has_avx_fma() {
                        // SAFETY: AVX and FMA support has been verified by has_avx_fma().
                        unsafe { avx::$avx_mul_add(result, slice_a, slice_b) };
                    } else {
                        multiply_add_elements_scalar(result, slice_a, slice_b);
                    }
                }

                #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
                multiply_add_elements_scalar(result, slice_a, slice_b);
            }
        }
    };
}

impl_conv_kernel!(
    f64,
    multiply_elements_neon_f64,
    multiply_add_elements_neon_f64,
    multiply_elements_avx_fma_f64,
    multiply_add_elements_avx_fma_f64
);
impl_conv_kernel!(
    f32,
    multiply_elements_neon_f32,
    multiply_add_elements_neon_f32,
    multiply_elements_avx_fma_f32,
    multiply_add_elements_avx_fma_f32
);

/// Shared FFT planner for all convolution filters.
///
/// The plans depend only on the FFT length, which is twice the chunksize, so
/// every convolution filter in a configuration asks for the same pair. A
/// planner caches what it has built, but only for as long as it lives, so a
/// planner per filter threw the cache away and every filter paid full price.
/// Planning scales with the FFT length while a cache hit does not: for a 16384
/// sample chunk a fresh pair costs about 250 us against about 40 ns from a warm
/// planner, which is most of the time an eight channel reload spends.
///
/// `spectrum.rs` keeps its own planner for the same reason. That one is fixed
/// at `f32` while these plans follow `CamillaFloat`, so they cannot be merged.
///
/// The lock is only taken while building filters, never while processing.
static FFT_PLANNER: LazyLock<Mutex<RealFftPlanner<CamillaFloat>>> =
    LazyLock::new(|| Mutex::new(RealFftPlanner::new()));

/// Plan the forward and inverse transforms for a convolution of `data_length`
/// samples, reusing any plan the shared planner has already built.
fn plan_transforms(
    data_length: usize,
) -> (
    Arc<dyn RealToComplex<CamillaFloat>>,
    Arc<dyn ComplexToReal<CamillaFloat>>,
) {
    let mut planner = FFT_PLANNER.lock().unwrap();
    (
        planner.plan_fft_forward(2 * data_length),
        planner.plan_fft_inverse(2 * data_length),
    )
}

/// Convolution coefficients, padded into segments and transformed, ready for
/// the processing loop to multiply against.
pub type ConvCoeffs = SegmentedSpectrum;

/// A run of equal-length spectra held in one contiguous allocation.
///
/// This was a `Vec<Vec<Complex>>`, one heap block per segment. The hot loop
/// walks every segment of both the coefficients and the input history once per
/// chunk, and with a block per segment the hardware prefetcher has to restart
/// at every boundary. A single allocation lets it run the whole way.
pub struct SegmentedSpectrum {
    data: Vec<Complex<CamillaFloat>>,
    seg_len: usize,
}

impl SegmentedSpectrum {
    /// `segments` spectra of `seg_len` bins each, all zero.
    pub fn zeroed(segments: usize, seg_len: usize) -> Self {
        SegmentedSpectrum {
            data: vec![Complex::zero(); segments * seg_len],
            seg_len,
        }
    }

    /// Number of segments.
    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.seg_len).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn segment(&self, index: usize) -> &[Complex<CamillaFloat>] {
        &self.data[index * self.seg_len..(index + 1) * self.seg_len]
    }

    pub fn segment_mut(&mut self, index: usize) -> &mut [Complex<CamillaFloat>] {
        &mut self.data[index * self.seg_len..(index + 1) * self.seg_len]
    }
}

/// Transformed coefficients keyed by filter name, so that channels running the
/// same convolution filter share one copy.
///
/// Loading and transforming an impulse response is the bulk of the work of
/// building a convolution filter, and for a file-backed filter it includes
/// reading the file. Doing that once per channel duplicated both the work and
/// the result, and the duplicated coefficients then competed for cache and
/// memory bandwidth while the channels were processed in parallel.
///
/// A cache is only valid for one build or update pass, where every name maps
/// to exactly one configuration. Create one, use it for the pass, drop it;
/// keeping it any longer risks handing out coefficients from a stale config.
///
/// [`transformed`](Self::transformed) fills one in before the pass rather than
/// during it, so that the transforms happen off the processing thread. That is
/// safe for the same reason: the cache is built from one configuration and
/// travels with it to the pass that applies it, never alongside a later one.
#[derive(Default)]
pub struct ConvCoeffCache {
    /// Keyed by name and segment length together. The transformed spectra are
    /// cut into segments sized for one particular FFT, so handing a set built
    /// for one length to a filter running another would leave the multiply
    /// kernel short of bins. Nothing in the pipeline mixes lengths in one
    /// pass, but this is a public constructor and the invariant is not
    /// otherwise visible from it.
    entries: HashMap<(String, usize), Arc<ConvCoeffs>>,
}

impl ConvCoeffCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Transform the impulse responses of the named convolution filters ahead
    /// of the pass that builds them.
    ///
    /// Transforming an impulse response is the expensive half of building a
    /// convolution filter, and it is all this needs to do: `impulses` holds
    /// what config validation already read, so nothing is read from disk here.
    /// A filter that is somehow not in there is read on the spot, which is why
    /// this is fallible at all.
    ///
    /// Names that are not in the config, or that are not convolution filters,
    /// are skipped. There is nothing to transform for them, and a name that
    /// does not resolve is the build pass's problem to report, not this one's.
    pub fn transformed(
        conf: &config::Configuration,
        names: &[String],
        impulses: &ImpulseCache,
    ) -> Res<Self> {
        let mut cache = Self::new();
        let Some(filters) = conf.filters.as_ref() else {
            return Ok(cache);
        };
        let convs: Vec<(&String, &config::ConvParameters)> = names
            .iter()
            .filter_map(|name| match filters.get(name) {
                Some(config::Filter::Conv { parameters, .. }) => Some((name, parameters)),
                _ => None,
            })
            .collect();
        if convs.is_empty() {
            return Ok(cache);
        }
        // One plan and one scratch buffer for the whole pass. `plan_transforms`
        // takes the lock on the shared planner, which the processing thread
        // needs too whenever it constructs a filter, so this thread holds it
        // once rather than once per filter. The inverse plan is not wanted
        // here: nothing transforms back until the filter is running.
        let npoints = conf.devices.chunksize();
        let (fft, _) = plan_transforms(npoints);
        let mut scratch = fft.make_scratch_vec();
        for (name, parameters) in convs {
            cache.transform(name, npoints, parameters, impulses, &*fft, &mut scratch)?;
        }
        Ok(cache)
    }

    fn transform(
        &mut self,
        name: &str,
        npoints: usize,
        conf: &config::ConvParameters,
        impulses: &ImpulseCache,
        fft: &dyn RealToComplex<CamillaFloat>,
        scratch: &mut [Complex<CamillaFloat>],
    ) -> Res<()> {
        if self.entries.contains_key(&(name.to_string(), npoints)) {
            return Ok(());
        }
        let read;
        let coeffs = match impulses.get(name) {
            Some(coeffs) => coeffs,
            None => {
                debug!("Conv {name} was not read during validation, reading it now");
                read = coeffs_from_config(conf)?;
                &read
            }
        };
        let coeffs_f = transform_coeffs(coeffs, npoints, fft, scratch);
        debug!("Conv {name} transformed into {} segments", coeffs_f.len());
        self.insert(name, npoints, &coeffs_f);
        Ok(())
    }

    /// How many distinct filters the cache holds coefficients for.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn get(&self, name: &str, npoints: usize) -> Option<Arc<ConvCoeffs>> {
        self.entries.get(&(name.to_string(), npoints)).cloned()
    }

    fn insert(&mut self, name: &str, npoints: usize, coeffs: &Arc<ConvCoeffs>) {
        self.entries
            .insert((name.to_string(), npoints), coeffs.clone());
    }
}

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
fn coeffs_from_config(conf: &config::ConvParameters) -> Res<Vec<CamillaFloat>> {
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

/// Read the impulse response for a filter that is being built as part of a
/// live pipeline, where there is no way to report a failure and nothing
/// sensible to do about one.
///
/// Every config change on its way to a running pipeline arrives with its
/// impulse responses already read and transformed, so this is only reached by
/// the first build of a session and by the public constructors.
fn coeffs_for_pass(name: &str, conf: &config::ConvParameters) -> Vec<CamillaFloat> {
    match coeffs_from_config(conf) {
        Ok(coeffs) => coeffs,
        Err(err) => panic!("Could not read coefficients for conv filter '{name}': {err}"),
    }
}

/// Split an impulse response into `data_length` sized segments and transform
/// each one.
fn transform_coeffs(
    coeffs: &[CamillaFloat],
    data_length: usize,
    fft: &dyn RealToComplex<CamillaFloat>,
    scratch: &mut [Complex<CamillaFloat>],
) -> Arc<ConvCoeffs> {
    // Never zero. Processing indexes the segment ring with `% nsegments`, so a
    // filter built with no coefficients has to come out as one silent segment
    // rather than a division by zero. A configuration cannot reach this,
    // `validate_config` rejecting empty coefficients, but the constructors are
    // public.
    let nsegments = coeffs.len().div_ceil(data_length).max(1);
    let mut coeffs_padded = vec![vec![0.0; 2 * data_length]; nsegments];
    let mut coeffs_f = SegmentedSpectrum::zeroed(nsegments, data_length + 1);
    for (n, coeff) in coeffs.iter().enumerate() {
        coeffs_padded[n / data_length][n % data_length] = coeff / (2 * data_length) as CamillaFloat;
    }
    for (n, segment) in coeffs_padded.iter_mut().enumerate() {
        fft.process_with_scratch(segment, coeffs_f.segment_mut(n), scratch)
            .unwrap();
    }
    Arc::new(coeffs_f)
}

pub struct FftConv {
    name: String,
    npoints: usize,
    nsegments: usize,
    overlap: Vec<CamillaFloat>,
    coeffs_f: Arc<ConvCoeffs>,
    fft: Arc<dyn RealToComplex<CamillaFloat>>,
    ifft: Arc<dyn ComplexToReal<CamillaFloat>>,
    scratch_fw: Vec<Complex<CamillaFloat>>,
    scratch_inv: Vec<Complex<CamillaFloat>>,
    input_buf: Vec<CamillaFloat>,
    input_f: SegmentedSpectrum,
    temp_buf: Vec<Complex<CamillaFloat>>,
    output_buf: Vec<CamillaFloat>,
    index: usize,
}

impl FftConv {
    /// Create a new FFT convolution filter.
    pub fn new(name: &str, data_length: usize, coeffs: &[CamillaFloat]) -> Self {
        let (fft, _) = plan_transforms(data_length);
        let mut scratch_fw = fft.make_scratch_vec();
        let coeffs_f = transform_coeffs(coeffs, data_length, &*fft, &mut scratch_fw);
        FftConv::with_coeffs(name, data_length, coeffs_f)
    }

    /// Create a filter from coefficients that are already transformed, sharing
    /// them with whichever other channels were given the same `Arc`.
    ///
    /// Only the coefficients are shared. Everything the processing loop writes
    /// to stays private to this filter.
    pub fn with_coeffs(name: &str, data_length: usize, coeffs_f: Arc<ConvCoeffs>) -> Self {
        let (fft, ifft) = plan_transforms(data_length);
        let scratch_fw = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        let nsegments = coeffs_f.len();
        debug!("Conv {name} is using {nsegments} segments");
        FftConv {
            name: name.to_string(),
            npoints: data_length,
            nsegments,
            overlap: vec![0.0; data_length],
            coeffs_f,
            fft,
            ifft,
            scratch_fw,
            scratch_inv,
            input_f: SegmentedSpectrum::zeroed(nsegments, data_length + 1),
            input_buf: vec![0.0; 2 * data_length],
            temp_buf: vec![Complex::zero(); data_length + 1],
            output_buf: vec![0.0; 2 * data_length],
            index: 0,
        }
    }

    pub fn from_config(name: &str, data_length: usize, conf: config::ConvParameters) -> Self {
        FftConv::new(name, data_length, &coeffs_for_pass(name, &conf))
    }

    /// Build from a configuration, reusing the transformed coefficients if
    /// another channel already built this filter in the same pass.
    pub fn from_config_cached(
        name: &str,
        data_length: usize,
        conf: config::ConvParameters,
        cache: &mut ConvCoeffCache,
    ) -> Self {
        if let Some(coeffs_f) = cache.get(name, data_length) {
            debug!("Conv {name} reuses coefficients already built for another channel");
            return FftConv::with_coeffs(name, data_length, coeffs_f);
        }
        let filter = FftConv::from_config(name, data_length, conf);
        cache.insert(name, data_length, &filter.coeffs_f);
        filter
    }
}

impl Filter for FftConv {
    fn name(&self) -> &str {
        &self.name
    }

    /// Process a waveform by FT, then multiply transform with transform of filter, and then transform back.
    fn process_waveform(&mut self, waveform: &mut [CamillaFloat]) {
        // Copy to input buffer and clear overlap area
        self.input_buf[0..self.npoints].copy_from_slice(waveform);
        for item in self
            .input_buf
            .iter_mut()
            .skip(self.npoints)
            .take(self.npoints)
        {
            *item = 0.0;
        }

        // FFT and store result in history, update index
        self.index = (self.index + 1) % self.nsegments;
        self.fft
            .process_with_scratch(
                &mut self.input_buf,
                self.input_f.segment_mut(self.index),
                &mut self.scratch_fw,
            )
            .unwrap();

        // Loop through history of input FTs, multiply with filter FTs, accumulate result
        let segm = 0;
        let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
        CamillaFloat::multiply_elements(
            &mut self.temp_buf,
            self.input_f.segment(hist_idx),
            self.coeffs_f.segment(segm),
        );
        for segm in 1..self.nsegments {
            let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
            CamillaFloat::multiply_add_elements(
                &mut self.temp_buf,
                self.input_f.segment(hist_idx),
                self.coeffs_f.segment(segm),
            );
        }

        // IFFT result, store result and overlap
        self.ifft
            .process_with_scratch(
                &mut self.temp_buf,
                &mut self.output_buf,
                &mut self.scratch_inv,
            )
            .unwrap();
        for (n, item) in waveform.iter_mut().enumerate().take(self.npoints) {
            *item = self.output_buf[n] + self.overlap[n];
        }
        self.overlap
            .copy_from_slice(&self.output_buf[self.npoints..]);
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        self.update_parameters_cached(conf, &mut ConvCoeffCache::new());
    }

    fn update_parameters_cached(&mut self, conf: config::Filter, cache: &mut ConvCoeffCache) {
        let config::Filter::Conv {
            parameters: conf, ..
        } = conf
        else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        };
        let coeffs_f = match cache.get(&self.name, self.npoints) {
            Some(coeffs_f) => coeffs_f,
            None => {
                // First channel to reach this filter does the reading and the
                // transform; the rest take the result. With a cache filled by
                // `ConvCoeffCache::transformed` neither happens here at all.
                let coeffs = coeffs_for_pass(&self.name, &conf);
                let coeffs_f =
                    transform_coeffs(&coeffs, self.npoints, &*self.fft, &mut self.scratch_fw);
                cache.insert(&self.name, self.npoints, &coeffs_f);
                coeffs_f
            }
        };

        let nsegments = coeffs_f.len();
        debug!("conv using {nsegments} segments");
        if nsegments != self.nsegments {
            // Length changed, so the history no longer lines up. Clear it.
            self.nsegments = nsegments;
            self.input_f = SegmentedSpectrum::zeroed(nsegments, self.npoints + 1);
        }
        self.coeffs_f = coeffs_f;
    }
}

// Benchmark API: kernel wrappers for benches/fftconv_kernels.rs (feature = "bench" only).

#[cfg(feature = "bench")]
pub fn bench_multiply_elements_scalar(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    multiply_elements_scalar(result, slice_a, slice_b);
}

#[cfg(feature = "bench")]
pub fn bench_multiply_add_elements_scalar(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    multiply_add_elements_scalar(result, slice_a, slice_b);
}

#[cfg(all(target_arch = "x86_64", feature = "bench"))]
pub fn bench_has_avx_fma() -> bool {
    avx::has_avx_fma()
}

/// Runs the AVX+FMA kernel for the precision `CamillaFloat` is built with.
///
/// # Safety
/// Caller must verify AVX+FMA availability via `bench_has_avx_fma()`.
#[cfg(all(target_arch = "x86_64", feature = "bench"))]
pub unsafe fn bench_multiply_elements_avx_fma(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    CamillaFloat::multiply_elements(result, slice_a, slice_b);
}

/// Runs the AVX+FMA kernel for the precision `CamillaFloat` is built with.
///
/// # Safety
/// Caller must verify AVX+FMA availability via `bench_has_avx_fma()`.
#[cfg(all(target_arch = "x86_64", feature = "bench"))]
pub unsafe fn bench_multiply_add_elements_avx_fma(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    CamillaFloat::multiply_add_elements(result, slice_a, slice_b);
}

/// Runs the NEON kernel for the precision `CamillaFloat` is built with.
///
/// # Safety
/// Caller must ensure this is only used on aarch64 where NEON is available.
#[cfg(all(target_arch = "aarch64", feature = "bench"))]
pub unsafe fn bench_multiply_elements_neon(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    CamillaFloat::multiply_elements(result, slice_a, slice_b);
}

/// Runs the NEON kernel for the precision `CamillaFloat` is built with.
///
/// # Safety
/// Caller must ensure this is only used on aarch64 where NEON is available.
#[cfg(all(target_arch = "aarch64", feature = "bench"))]
pub unsafe fn bench_multiply_add_elements_neon(
    result: &mut [Complex<CamillaFloat>],
    slice_a: &[Complex<CamillaFloat>],
    slice_b: &[Complex<CamillaFloat>],
) {
    CamillaFloat::multiply_add_elements(result, slice_a, slice_b);
}

/// Validate a FFT convolution config, keeping the impulse response it read.
///
/// Validating means reading the coefficients, so the read is where the answer
/// comes from either way. Handing them to `impulses` is what stops them being
/// read again, both by a later pipeline step naming the same filter and by the
/// pass that eventually builds it.
pub fn validate_config(
    name: &str,
    conf: &config::ConvParameters,
    impulses: &mut ImpulseCache,
) -> Res<()> {
    // Filter names are unique within a config, so a name already in the cache
    // was read from these same parameters and is already known to be valid.
    if impulses.contains(name) {
        return Ok(());
    }
    let coeffs = coeffs_from_config(conf)?;
    if coeffs.is_empty() {
        return Err(config::ConfigError::new("Conv coefficients are empty").into());
    }
    config::check_all_finite("coefficients", &coeffs)?;
    impulses.insert(name, coeffs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::CamillaFloat;
    use crate::ToCamillaFloat;
    use crate::config;
    use crate::config::ConvParameters;
    use crate::config::finite;
    use crate::filters::Filter;
    use crate::filters::fftconv::{ConvCoeffCache, FftConv, ImpulseCache};
    use num_complex::Complex;
    use std::sync::Arc;

    /// A round trip through the FFT loses more of the mantissa in an f32
    /// build than in an f64 one, so the tolerance follows the processing
    /// precision. Slackening both to what f32 needs would stop the f64 build
    /// noticing a real loss of accuracy.
    #[cfg(not(camillafloat_f32))]
    const ROUNDTRIP_TOL: CamillaFloat = 1e-9;
    #[cfg(camillafloat_f32)]
    const ROUNDTRIP_TOL: CamillaFloat = 1e-5;

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
        let coeffs = vec![finite!(0.5), finite!(0.5)];
        let conf = ConvParameters::Values { values: coeffs };
        let mut filter = FftConv::from_config("test", 8, conf);
        let mut wave1 = vec![1.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
        let expected = vec![0.5, 1.0, 1.0, 0.5, 0.0, -0.5, -0.5, 0.0];
        filter.process_waveform(&mut wave1);
        assert!(compare_waveforms(wave1, expected, 1e-7));
    }

    /// Filters sharing the planner must behave exactly as they did with a
    /// private one, including when several chunksizes are in play so the
    /// planner is holding more than one pair of plans.
    #[test]
    fn shared_planner_gives_same_result() {
        let coeffs: Vec<CamillaFloat> = (0..48).map(|m| m as CamillaFloat).collect();
        for data_length in [8, 16, 8] {
            let mut first = FftConv::new("first", data_length, &coeffs);
            let mut second = FftConv::new("second", data_length, &coeffs);
            for block in 0..6 {
                let input: Vec<CamillaFloat> = (0..data_length)
                    .map(|n| ((n + block * data_length) as CamillaFloat * 0.25).sin())
                    .collect();
                let mut wave_first = input.clone();
                let mut wave_second = input;
                first.process_waveform(&mut wave_first);
                second.process_waveform(&mut wave_second);
                assert!(
                    compare_waveforms(wave_first, wave_second, 1e-9),
                    "length {data_length}, block {block}"
                );
            }
        }
    }

    /// Empty inline coefficients used to pass validation and then panic on the
    /// first chunk, dividing by a segment count of zero. Validation rejects
    /// them now, and a filter built through the public constructor comes out
    /// silent rather than fatal.
    #[test]
    fn empty_inline_coefficients_are_rejected_and_harmless() {
        let conf = ConvParameters::Values { values: vec![] };
        assert!(
            super::validate_config("empty", &conf, &mut ImpulseCache::new()).is_err(),
            "empty coefficients should not validate"
        );

        // Built directly, bypassing validation, it must be silent not fatal.
        let mut filter = FftConv::from_config("empty", 8, conf);
        let mut wave: Vec<CamillaFloat> = (0..8).map(|n| n as CamillaFloat).collect();
        filter.process_waveform(&mut wave);
        assert!(
            wave.iter().all(|v| *v == 0.0),
            "expected silence, got {wave:?}"
        );
    }

    /// The spectra are cut into segments sized for one FFT, so the same name
    /// at two lengths must not share them. Before the length joined the key
    /// the second filter got the first one's segment width and panicked in
    /// the multiply kernel.
    #[test]
    fn cache_does_not_share_across_lengths() {
        let conf = config::ConvParameters::Values {
            values: vec![finite!(0.1), finite!(0.2), finite!(0.3), finite!(0.4)],
        };
        let mut cache = ConvCoeffCache::new();
        let mut short = FftConv::from_config_cached("conv", 8, conf.clone(), &mut cache);
        let mut long = FftConv::from_config_cached("conv", 16, conf, &mut cache);
        let mut short_wave = vec![0.0 as CamillaFloat; 8];
        short_wave[0] = 1.0;
        let mut long_wave = vec![0.0 as CamillaFloat; 16];
        long_wave[0] = 1.0;
        short.process_waveform(&mut short_wave);
        long.process_waveform(&mut long_wave);
        // An impulse in, so what comes out is the impulse response itself.
        // Both lengths must give it, which they cannot if either took the
        // other's coefficients.
        let expected: Vec<CamillaFloat> = vec![0.1, 0.2, 0.3, 0.4]
            .into_iter()
            .map(|v: f64| v.to_camilla_float())
            .collect();
        assert!(compare_waveforms(
            short_wave[..4].to_vec(),
            expected.clone(),
            ROUNDTRIP_TOL
        ));
        assert!(compare_waveforms(
            long_wave[..4].to_vec(),
            expected,
            ROUNDTRIP_TOL
        ));
    }

    /// Two channels built from the same filter must share one allocation of
    /// coefficients, and must still behave exactly like independent filters.
    /// The shared part is read-only; the overlap history is not.
    #[test]
    fn cached_build_shares_coeffs_but_not_state() {
        // A config always carries f64, whatever the processing precision is.
        let values: Vec<f64> = (0..48).map(|m| m as f64).collect();
        let coeffs: Vec<CamillaFloat> = values.iter().map(|v| v.to_camilla_float()).collect();
        let values = values.into_iter().map(|v| finite!(v)).collect();
        let conf = ConvParameters::Values { values };
        let mut cache = ConvCoeffCache::new();
        let mut left = FftConv::from_config_cached("conv", 8, conf.clone(), &mut cache);
        let mut right = FftConv::from_config_cached("conv", 8, conf, &mut cache);
        assert!(
            Arc::ptr_eq(&left.coeffs_f, &right.coeffs_f),
            "second channel should reuse the cached coefficients"
        );

        // An impulse into the left channel only. The right channel must stay
        // silent, which it cannot do if the two share overlap history.
        let mut reference = FftConv::new("reference", 8, &coeffs);
        for block in 0..6 {
            let mut wave_left = vec![0.0 as CamillaFloat; 8];
            let mut wave_ref = vec![0.0 as CamillaFloat; 8];
            if block == 0 {
                wave_left[0] = 1.0;
                wave_ref[0] = 1.0;
            }
            let mut wave_right = vec![0.0 as CamillaFloat; 8];
            left.process_waveform(&mut wave_left);
            right.process_waveform(&mut wave_right);
            reference.process_waveform(&mut wave_ref);
            assert!(
                compare_waveforms(wave_left, wave_ref, 1e-5),
                "shared build changed the result, block {block}"
            );
            assert!(
                wave_right.iter().all(|v| v.abs() < 1e-9),
                "silent channel picked up the other channel's history, block {block}"
            );
        }
    }

    /// The same for the hot-reload path: one read and transform, shared out,
    /// with per-filter history left alone.
    #[test]
    fn cached_update_shares_coeffs() {
        let initial: Vec<CamillaFloat> = vec![1.0, 0.0, 0.0, 0.0];
        let mut left = FftConv::new("conv", 4, &initial);
        let mut right = FftConv::new("conv", 4, &initial);
        assert!(!Arc::ptr_eq(&left.coeffs_f, &right.coeffs_f));

        let conf = config::Filter::Conv {
            description: None,
            parameters: ConvParameters::Values {
                values: vec![finite!(0.0), finite!(1.0), finite!(0.0), finite!(0.0)],
            },
        };
        let mut cache = ConvCoeffCache::new();
        left.update_parameters_cached(conf.clone(), &mut cache);
        right.update_parameters_cached(conf, &mut cache);
        assert!(
            Arc::ptr_eq(&left.coeffs_f, &right.coeffs_f),
            "second channel should reuse the coefficients transformed for the first"
        );

        // A one sample delay now, so an impulse comes back shifted by one.
        let mut wave = vec![1.0 as CamillaFloat, 0.0, 0.0, 0.0];
        left.process_waveform(&mut wave);
        assert!(compare_waveforms(wave, vec![0.0, 1.0, 0.0, 0.0], 1e-5));
    }

    #[test]
    fn check_result_segmented() {
        let mut coeffs = Vec::<CamillaFloat>::new();
        for m in 0..32 {
            coeffs.push(m as CamillaFloat);
        }
        let mut filter = FftConv::new("test", 8, &coeffs);
        let mut wave1 = vec![0.0 as CamillaFloat; 8];
        let mut wave2 = vec![0.0 as CamillaFloat; 8];
        let mut wave3 = vec![0.0 as CamillaFloat; 8];
        let mut wave4 = vec![0.0 as CamillaFloat; 8];
        let mut wave5 = vec![0.0 as CamillaFloat; 8];

        wave1[0] = 1.0;
        filter.process_waveform(&mut wave1);
        filter.process_waveform(&mut wave2);
        filter.process_waveform(&mut wave3);
        filter.process_waveform(&mut wave4);
        filter.process_waveform(&mut wave5);

        let exp1 = Vec::from(&coeffs[0..8]);
        let exp2 = Vec::from(&coeffs[8..16]);
        let exp3 = Vec::from(&coeffs[16..24]);
        let exp4 = Vec::from(&coeffs[24..32]);
        let exp5 = vec![0.0 as CamillaFloat; 8];

        assert!(compare_waveforms(wave1, exp1, 1e-5));
        assert!(compare_waveforms(wave2, exp2, 1e-5));
        assert!(compare_waveforms(wave3, exp3, 1e-5));
        assert!(compare_waveforms(wave4, exp4, 1e-5));
        assert!(compare_waveforms(wave5, exp5, 1e-5));
    }

    // FMA rounds differently from scalar; SIMD results may differ by a few ULPs.
    #[cfg(not(camillafloat_f32))]
    const SIMD_TOL: CamillaFloat = 1e-9;
    #[cfg(camillafloat_f32)]
    const SIMD_TOL: CamillaFloat = 1e-5;

    #[test]
    fn multiply_elements_scalar_known_values() {
        use super::multiply_elements_scalar;

        // (1 + 2i) * (3 + 4i) = (3-8) + (4+6)i = -5 + 10i
        let a = vec![Complex::new(1.0 as CamillaFloat, 2.0)];
        let b = vec![Complex::new(3.0 as CamillaFloat, 4.0)];
        let mut result = vec![Complex::new(0.0 as CamillaFloat, 0.0)];

        multiply_elements_scalar(&mut result, &a, &b);

        assert!(is_close(result[0].re, -5.0, SIMD_TOL));
        assert!(is_close(result[0].im, 10.0, SIMD_TOL));
    }

    #[test]
    fn multiply_add_elements_scalar_known_values() {
        use super::multiply_add_elements_scalar;

        // result starts at (1 + 1i), then += (1 + 2i) * (3 + 4i) = -5 + 10i
        // expected final: (-4 + 11i)
        let a = vec![Complex::new(1.0 as CamillaFloat, 2.0)];
        let b = vec![Complex::new(3.0 as CamillaFloat, 4.0)];
        let mut result = vec![Complex::new(1.0 as CamillaFloat, 1.0)];

        multiply_add_elements_scalar(&mut result, &a, &b);

        assert!(is_close(result[0].re, -4.0, SIMD_TOL));
        assert!(is_close(result[0].im, 11.0, SIMD_TOL));
    }
}
