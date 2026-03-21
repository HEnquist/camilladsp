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
use std::sync::Arc;

// Sample format
use crate::PrcFmt;
use crate::Res;

// element-wise product, result = slice_a * slice_b
fn multiply_elements_scalar(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
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
fn multiply_add_elements_scalar(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
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

// AVX + FMA SIMD kernels for complex multiply/multiply-add.
// result.re = a.re*b.re - a.im*b.im, result.im = a.re*b.im + a.im*b.re
// Uses _mm256_fmaddsub for the sign pattern; 4xYMM unrolled + 1xYMM cleanup + scalar tail.
// multiply-add computes the product via fmaddsub, then adds the accumulator with _mm256_add.

// f64: each YMM holds 2 Complex<f64>; 4xYMM = 8 complex per iter.

#[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
#[target_feature(enable = "avx", enable = "fma")]
unsafe fn multiply_elements_avx_fma(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::x86_64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f64;
    let a_ptr = slice_a.as_ptr() as *const f64;
    let b_ptr = slice_b.as_ptr() as *const f64;

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4xYMM main loop: 8 complex f64 per iteration ----
        let chunks_8 = len / 8;
        for i in 0..chunks_8 {
            let off = i * 16; // 8 complex * 2 f64/complex

            let a0 = _mm256_loadu_pd(a_ptr.add(off));
            let b0 = _mm256_loadu_pd(b_ptr.add(off));
            let a1 = _mm256_loadu_pd(a_ptr.add(off + 4));
            let b1 = _mm256_loadu_pd(b_ptr.add(off + 4));
            let a2 = _mm256_loadu_pd(a_ptr.add(off + 8));
            let b2 = _mm256_loadu_pd(b_ptr.add(off + 8));
            let a3 = _mm256_loadu_pd(a_ptr.add(off + 12));
            let b3 = _mm256_loadu_pd(b_ptr.add(off + 12));

            // movedup_pd: broadcast re. permute_pd(0xF=0b1111): select high double per lane -> broadcast im.
            let (a_re0, a_im0) = (_mm256_movedup_pd(a0), _mm256_permute_pd(a0, 0xF));
            let (a_re1, a_im1) = (_mm256_movedup_pd(a1), _mm256_permute_pd(a1, 0xF));
            let (a_re2, a_im2) = (_mm256_movedup_pd(a2), _mm256_permute_pd(a2, 0xF));
            let (a_re3, a_im3) = (_mm256_movedup_pd(a3), _mm256_permute_pd(a3, 0xF));

            let (b_sw0, b_sw1) = (_mm256_permute_pd(b0, 0x5), _mm256_permute_pd(b1, 0x5));
            let (b_sw2, b_sw3) = (_mm256_permute_pd(b2, 0x5), _mm256_permute_pd(b3, 0x5));

            let r0 = _mm256_fmaddsub_pd(a_re0, b0, _mm256_mul_pd(a_im0, b_sw0));
            let r1 = _mm256_fmaddsub_pd(a_re1, b1, _mm256_mul_pd(a_im1, b_sw1));
            let r2 = _mm256_fmaddsub_pd(a_re2, b2, _mm256_mul_pd(a_im2, b_sw2));
            let r3 = _mm256_fmaddsub_pd(a_re3, b3, _mm256_mul_pd(a_im3, b_sw3));

            _mm256_storeu_pd(r_ptr.add(off), r0);
            _mm256_storeu_pd(r_ptr.add(off + 4), r1);
            _mm256_storeu_pd(r_ptr.add(off + 8), r2);
            _mm256_storeu_pd(r_ptr.add(off + 12), r3);
        }

        // ---- 1xYMM cleanup: 2 complex f64 per step ----
        let tail_start = chunks_8 * 8;
        let remaining_pairs = (len - tail_start) / 2;
        for j in 0..remaining_pairs {
            let off = (tail_start + j * 2) * 2;
            let a0 = _mm256_loadu_pd(a_ptr.add(off));
            let b0 = _mm256_loadu_pd(b_ptr.add(off));
            let a_re0 = _mm256_movedup_pd(a0);
            let a_im0 = _mm256_permute_pd(a0, 0xF); // 0xF=0b1111: select high double per lane -> broadcast im
            let b_sw0 = _mm256_permute_pd(b0, 0x5);
            _mm256_storeu_pd(
                r_ptr.add(off),
                _mm256_fmaddsub_pd(a_re0, b0, _mm256_mul_pd(a_im0, b_sw0)),
            );
        }

        // ---- Scalar tail: 0-1 remaining Complex<f64> ----
        let simd_done = tail_start + remaining_pairs * 2;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) = *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

#[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
#[target_feature(enable = "avx", enable = "fma")]
unsafe fn multiply_add_elements_avx_fma(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::x86_64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f64;
    let a_ptr = slice_a.as_ptr() as *const f64;
    let b_ptr = slice_b.as_ptr() as *const f64;

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4xYMM main loop: 8 complex f64 per iteration ----
        let chunks_8 = len / 8;
        for i in 0..chunks_8 {
            let off = i * 16;

            let acc0 = _mm256_loadu_pd(r_ptr.add(off));
            let acc1 = _mm256_loadu_pd(r_ptr.add(off + 4));
            let acc2 = _mm256_loadu_pd(r_ptr.add(off + 8));
            let acc3 = _mm256_loadu_pd(r_ptr.add(off + 12));

            let a0 = _mm256_loadu_pd(a_ptr.add(off));
            let b0 = _mm256_loadu_pd(b_ptr.add(off));
            let a1 = _mm256_loadu_pd(a_ptr.add(off + 4));
            let b1 = _mm256_loadu_pd(b_ptr.add(off + 4));
            let a2 = _mm256_loadu_pd(a_ptr.add(off + 8));
            let b2 = _mm256_loadu_pd(b_ptr.add(off + 8));
            let a3 = _mm256_loadu_pd(a_ptr.add(off + 12));
            let b3 = _mm256_loadu_pd(b_ptr.add(off + 12));

            // movedup_pd: broadcast re. permute_pd(0xF=0b1111): select high double per lane -> broadcast im.
            let (a_re0, a_im0) = (_mm256_movedup_pd(a0), _mm256_permute_pd(a0, 0xF));
            let (a_re1, a_im1) = (_mm256_movedup_pd(a1), _mm256_permute_pd(a1, 0xF));
            let (a_re2, a_im2) = (_mm256_movedup_pd(a2), _mm256_permute_pd(a2, 0xF));
            let (a_re3, a_im3) = (_mm256_movedup_pd(a3), _mm256_permute_pd(a3, 0xF));

            let (b_sw0, b_sw1) = (_mm256_permute_pd(b0, 0x5), _mm256_permute_pd(b1, 0x5));
            let (b_sw2, b_sw3) = (_mm256_permute_pd(b2, 0x5), _mm256_permute_pd(b3, 0x5));

            // Compute product a*b, then add accumulator.
            // Note: _mm256_add_pd cannot be fused into fmaddsub (alternating sign pattern ≠ plain add).
            let prod0 = _mm256_fmaddsub_pd(a_re0, b0, _mm256_mul_pd(a_im0, b_sw0));
            let prod1 = _mm256_fmaddsub_pd(a_re1, b1, _mm256_mul_pd(a_im1, b_sw1));
            let prod2 = _mm256_fmaddsub_pd(a_re2, b2, _mm256_mul_pd(a_im2, b_sw2));
            let prod3 = _mm256_fmaddsub_pd(a_re3, b3, _mm256_mul_pd(a_im3, b_sw3));

            _mm256_storeu_pd(r_ptr.add(off), _mm256_add_pd(acc0, prod0));
            _mm256_storeu_pd(r_ptr.add(off + 4), _mm256_add_pd(acc1, prod1));
            _mm256_storeu_pd(r_ptr.add(off + 8), _mm256_add_pd(acc2, prod2));
            _mm256_storeu_pd(r_ptr.add(off + 12), _mm256_add_pd(acc3, prod3));
        }

        // ---- 1xYMM cleanup: 2 complex f64 per step ----
        let tail_start = chunks_8 * 8;
        let remaining_pairs = (len - tail_start) / 2;
        for j in 0..remaining_pairs {
            let off = (tail_start + j * 2) * 2;
            let acc0 = _mm256_loadu_pd(r_ptr.add(off));
            let a0 = _mm256_loadu_pd(a_ptr.add(off));
            let b0 = _mm256_loadu_pd(b_ptr.add(off));
            let a_re0 = _mm256_movedup_pd(a0);
            let a_im0 = _mm256_permute_pd(a0, 0xF); // 0xF=0b1111: select high double per lane -> broadcast im
            let b_sw0 = _mm256_permute_pd(b0, 0x5);
            let prod0 = _mm256_fmaddsub_pd(a_re0, b0, _mm256_mul_pd(a_im0, b_sw0));
            _mm256_storeu_pd(r_ptr.add(off), _mm256_add_pd(acc0, prod0));
        }

        // ---- Scalar tail: 0-1 remaining Complex<f64> ----
        let simd_done = tail_start + remaining_pairs * 2;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) += *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

// f32: each YMM holds 4 Complex<f32>; 4xYMM = 16 complex per iter.

#[cfg(all(target_arch = "x86_64", feature = "32bit"))]
#[target_feature(enable = "avx", enable = "fma")]
unsafe fn multiply_elements_avx_fma(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::x86_64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f32;
    let a_ptr = slice_a.as_ptr() as *const f32;
    let b_ptr = slice_b.as_ptr() as *const f32;

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4xYMM main loop: 16 complex f32 per iteration ----
        let chunks_16 = len / 16;
        for i in 0..chunks_16 {
            let off = i * 32; // 16 complex * 2 f32/complex

            let a0 = _mm256_loadu_ps(a_ptr.add(off));
            let b0 = _mm256_loadu_ps(b_ptr.add(off));
            let a1 = _mm256_loadu_ps(a_ptr.add(off + 8));
            let b1 = _mm256_loadu_ps(b_ptr.add(off + 8));
            let a2 = _mm256_loadu_ps(a_ptr.add(off + 16));
            let b2 = _mm256_loadu_ps(b_ptr.add(off + 16));
            let a3 = _mm256_loadu_ps(a_ptr.add(off + 24));
            let b3 = _mm256_loadu_ps(b_ptr.add(off + 24));

            let (a_re0, a_im0) = (_mm256_moveldup_ps(a0), _mm256_movehdup_ps(a0));
            let (a_re1, a_im1) = (_mm256_moveldup_ps(a1), _mm256_movehdup_ps(a1));
            let (a_re2, a_im2) = (_mm256_moveldup_ps(a2), _mm256_movehdup_ps(a2));
            let (a_re3, a_im3) = (_mm256_moveldup_ps(a3), _mm256_movehdup_ps(a3));

            let (b_sw0, b_sw1) = (_mm256_permute_ps(b0, 0xB1), _mm256_permute_ps(b1, 0xB1));
            let (b_sw2, b_sw3) = (_mm256_permute_ps(b2, 0xB1), _mm256_permute_ps(b3, 0xB1));

            let r0 = _mm256_fmaddsub_ps(a_re0, b0, _mm256_mul_ps(a_im0, b_sw0));
            let r1 = _mm256_fmaddsub_ps(a_re1, b1, _mm256_mul_ps(a_im1, b_sw1));
            let r2 = _mm256_fmaddsub_ps(a_re2, b2, _mm256_mul_ps(a_im2, b_sw2));
            let r3 = _mm256_fmaddsub_ps(a_re3, b3, _mm256_mul_ps(a_im3, b_sw3));

            _mm256_storeu_ps(r_ptr.add(off), r0);
            _mm256_storeu_ps(r_ptr.add(off + 8), r1);
            _mm256_storeu_ps(r_ptr.add(off + 16), r2);
            _mm256_storeu_ps(r_ptr.add(off + 24), r3);
        }

        // ---- 1xYMM cleanup: 4 complex f32 per step ----
        let tail_start = chunks_16 * 16;
        let remaining_quads = (len - tail_start) / 4;
        for j in 0..remaining_quads {
            let off = (tail_start + j * 4) * 2;
            let a0 = _mm256_loadu_ps(a_ptr.add(off));
            let b0 = _mm256_loadu_ps(b_ptr.add(off));
            let a_re0 = _mm256_moveldup_ps(a0);
            let a_im0 = _mm256_movehdup_ps(a0);
            let b_sw0 = _mm256_permute_ps(b0, 0xB1);
            _mm256_storeu_ps(
                r_ptr.add(off),
                _mm256_fmaddsub_ps(a_re0, b0, _mm256_mul_ps(a_im0, b_sw0)),
            );
        }

        // ---- Scalar tail: 0-3 remaining Complex<f32> ----
        let simd_done = tail_start + remaining_quads * 4;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) = *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

#[cfg(all(target_arch = "x86_64", feature = "32bit"))]
#[target_feature(enable = "avx", enable = "fma")]
unsafe fn multiply_add_elements_avx_fma(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::x86_64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f32;
    let a_ptr = slice_a.as_ptr() as *const f32;
    let b_ptr = slice_b.as_ptr() as *const f32;

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4xYMM main loop: 16 complex f32 per iteration ----
        let chunks_16 = len / 16;
        for i in 0..chunks_16 {
            let off = i * 32;

            let acc0 = _mm256_loadu_ps(r_ptr.add(off));
            let acc1 = _mm256_loadu_ps(r_ptr.add(off + 8));
            let acc2 = _mm256_loadu_ps(r_ptr.add(off + 16));
            let acc3 = _mm256_loadu_ps(r_ptr.add(off + 24));

            let a0 = _mm256_loadu_ps(a_ptr.add(off));
            let b0 = _mm256_loadu_ps(b_ptr.add(off));
            let a1 = _mm256_loadu_ps(a_ptr.add(off + 8));
            let b1 = _mm256_loadu_ps(b_ptr.add(off + 8));
            let a2 = _mm256_loadu_ps(a_ptr.add(off + 16));
            let b2 = _mm256_loadu_ps(b_ptr.add(off + 16));
            let a3 = _mm256_loadu_ps(a_ptr.add(off + 24));
            let b3 = _mm256_loadu_ps(b_ptr.add(off + 24));

            let (a_re0, a_im0) = (_mm256_moveldup_ps(a0), _mm256_movehdup_ps(a0));
            let (a_re1, a_im1) = (_mm256_moveldup_ps(a1), _mm256_movehdup_ps(a1));
            let (a_re2, a_im2) = (_mm256_moveldup_ps(a2), _mm256_movehdup_ps(a2));
            let (a_re3, a_im3) = (_mm256_moveldup_ps(a3), _mm256_movehdup_ps(a3));

            let (b_sw0, b_sw1) = (_mm256_permute_ps(b0, 0xB1), _mm256_permute_ps(b1, 0xB1));
            let (b_sw2, b_sw3) = (_mm256_permute_ps(b2, 0xB1), _mm256_permute_ps(b3, 0xB1));

            // Compute product a*b, then add accumulator.
            // Note: _mm256_add_ps cannot be fused into fmaddsub (alternating sign pattern ≠ plain add).
            let prod0 = _mm256_fmaddsub_ps(a_re0, b0, _mm256_mul_ps(a_im0, b_sw0));
            let prod1 = _mm256_fmaddsub_ps(a_re1, b1, _mm256_mul_ps(a_im1, b_sw1));
            let prod2 = _mm256_fmaddsub_ps(a_re2, b2, _mm256_mul_ps(a_im2, b_sw2));
            let prod3 = _mm256_fmaddsub_ps(a_re3, b3, _mm256_mul_ps(a_im3, b_sw3));

            _mm256_storeu_ps(r_ptr.add(off), _mm256_add_ps(acc0, prod0));
            _mm256_storeu_ps(r_ptr.add(off + 8), _mm256_add_ps(acc1, prod1));
            _mm256_storeu_ps(r_ptr.add(off + 16), _mm256_add_ps(acc2, prod2));
            _mm256_storeu_ps(r_ptr.add(off + 24), _mm256_add_ps(acc3, prod3));
        }

        // ---- 1xYMM cleanup: 4 complex f32 per step ----
        let tail_start = chunks_16 * 16;
        let remaining_quads = (len - tail_start) / 4;
        for j in 0..remaining_quads {
            let off = (tail_start + j * 4) * 2;
            let acc0 = _mm256_loadu_ps(r_ptr.add(off));
            let a0 = _mm256_loadu_ps(a_ptr.add(off));
            let b0 = _mm256_loadu_ps(b_ptr.add(off));
            let a_re0 = _mm256_moveldup_ps(a0);
            let a_im0 = _mm256_movehdup_ps(a0);
            let b_sw0 = _mm256_permute_ps(b0, 0xB1);
            let prod0 = _mm256_fmaddsub_ps(a_re0, b0, _mm256_mul_ps(a_im0, b_sw0));
            _mm256_storeu_ps(r_ptr.add(off), _mm256_add_ps(acc0, prod0));
        }

        // ---- Scalar tail: 0-3 remaining Complex<f32> ----
        let simd_done = tail_start + remaining_quads * 4;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) += *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

// cached AVX+FMA detection; avoids repeated atomic loads in the hot path.
#[cfg(target_arch = "x86_64")]
#[inline]
fn has_avx_fma() -> bool {
    use std::sync::OnceLock;
    static DETECTED: OnceLock<bool> = OnceLock::new();
    *DETECTED.get_or_init(|| is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma"))
}

// Element-wise product: result = slice_a * slice_b. Dispatches to AVX+FMA or scalar.
#[inline]
fn multiply_elements(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    #[cfg(target_arch = "x86_64")]
    if has_avx_fma() {
        // SAFETY: AVX and FMA support has been verified by has_avx_fma().
        return unsafe { multiply_elements_avx_fma(result, slice_a, slice_b) };
    }
    multiply_elements_scalar(result, slice_a, slice_b);
}

// Element-wise accumulate product: result += slice_a * slice_b. Dispatches to AVX+FMA or scalar.
#[inline]
fn multiply_add_elements(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    #[cfg(target_arch = "x86_64")]
    if has_avx_fma() {
        // SAFETY: AVX and FMA support has been verified by has_avx_fma().
        return unsafe { multiply_add_elements_avx_fma(result, slice_a, slice_b) };
    }
    multiply_add_elements_scalar(result, slice_a, slice_b);
}

pub struct FftConv {
    name: String,
    npoints: usize,
    nsegments: usize,
    overlap: Vec<PrcFmt>,
    coeffs_f: Vec<Vec<Complex<PrcFmt>>>,
    fft: Arc<dyn RealToComplex<PrcFmt>>,
    ifft: Arc<dyn ComplexToReal<PrcFmt>>,
    scratch_fw: Vec<Complex<PrcFmt>>,
    scratch_inv: Vec<Complex<PrcFmt>>,
    input_buf: Vec<PrcFmt>,
    input_f: Vec<Vec<Complex<PrcFmt>>>,
    temp_buf: Vec<Complex<PrcFmt>>,
    output_buf: Vec<PrcFmt>,
    index: usize,
}

impl FftConv {
    /// Create a new FFT convolution filter.
    pub fn new(name: &str, data_length: usize, coeffs: &[PrcFmt]) -> Self {
        let name = name.to_string();
        let input_buf: Vec<PrcFmt> = vec![0.0; 2 * data_length];
        let temp_buf: Vec<Complex<PrcFmt>> = vec![Complex::zero(); data_length + 1];
        let output_buf: Vec<PrcFmt> = vec![0.0; 2 * data_length];
        let mut planner = RealFftPlanner::<PrcFmt>::new();
        let fft = planner.plan_fft_forward(2 * data_length);
        let ifft = planner.plan_fft_inverse(2 * data_length);
        let mut scratch_fw = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();

        let nsegments = ((coeffs.len() as PrcFmt) / (data_length as PrcFmt)).ceil() as usize;

        let input_f = vec![vec![Complex::zero(); data_length + 1]; nsegments];
        let mut coeffs_padded = vec![vec![0.0; 2 * data_length]; nsegments];
        let mut coeffs_f = vec![vec![Complex::zero(); data_length + 1]; nsegments];

        debug!("Conv {name} is using {nsegments} segments");

        for (n, coeff) in coeffs.iter().enumerate() {
            coeffs_padded[n / data_length][n % data_length] = coeff / (2 * data_length) as PrcFmt;
        }

        for (segment, segment_f) in coeffs_padded.iter_mut().zip(coeffs_f.iter_mut()) {
            fft.process_with_scratch(segment, segment_f, &mut scratch_fw)
                .unwrap();
        }

        FftConv {
            name,
            npoints: data_length,
            nsegments,
            overlap: vec![0.0; data_length],
            coeffs_f,
            fft,
            ifft,
            scratch_fw,
            scratch_inv,
            input_f,
            input_buf,
            output_buf,
            temp_buf,
            index: 0,
        }
    }

    pub fn from_config(name: &str, data_length: usize, conf: config::ConvParameters) -> Self {
        let values = match conf {
            config::ConvParameters::Values { values } => values,
            config::ConvParameters::Raw(params) => filters::read_coeff_file(
                &params.filename,
                &params.format(),
                params.read_bytes_lines(),
                params.skip_bytes_lines(),
            )
            .unwrap(),
            config::ConvParameters::Wav(params) => {
                filters::read_wav(&params.filename, params.channel()).unwrap()
            }
            config::ConvParameters::Dummy { length } => {
                let mut values = vec![0.0; length];
                values[0] = 1.0;
                values
            }
        };
        FftConv::new(name, data_length, &values)
    }
}

impl Filter for FftConv {
    fn name(&self) -> &str {
        &self.name
    }

    /// Process a waveform by FT, then multiply transform with transform of filter, and then transform back.
    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
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
                &mut self.input_f[self.index],
                &mut self.scratch_fw,
            )
            .unwrap();

        // Loop through history of input FTs, multiply with filter FTs, accumulate result
        let segm = 0;
        let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
        multiply_elements(
            &mut self.temp_buf,
            &self.input_f[hist_idx],
            &self.coeffs_f[segm],
        );
        for segm in 1..self.nsegments {
            let hist_idx = (self.index + self.nsegments - segm) % self.nsegments;
            multiply_add_elements(
                &mut self.temp_buf,
                &self.input_f[hist_idx],
                &self.coeffs_f[segm],
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
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Conv {
            parameters: conf, ..
        } = conf
        {
            let coeffs = match conf {
                config::ConvParameters::Values { values } => values,
                config::ConvParameters::Raw(params) => filters::read_coeff_file(
                    &params.filename,
                    &params.format(),
                    params.read_bytes_lines(),
                    params.skip_bytes_lines(),
                )
                .unwrap(),
                config::ConvParameters::Wav(params) => {
                    filters::read_wav(&params.filename, params.channel()).unwrap()
                }
                config::ConvParameters::Dummy { length } => {
                    let mut values = vec![0.0; length];
                    values[0] = 1.0;
                    values
                }
            };

            let nsegments = ((coeffs.len() as PrcFmt) / (self.npoints as PrcFmt)).ceil() as usize;

            if nsegments == self.nsegments {
                // Same length, lets keep history
            } else {
                // length changed, clearing history
                self.nsegments = nsegments;
                let input_f = vec![vec![Complex::zero(); self.npoints + 1]; nsegments];
                self.input_f = input_f;
            }

            let mut coeffs_f = vec![vec![Complex::zero(); self.npoints + 1]; nsegments];
            let mut coeffs_padded = vec![vec![0.0; 2 * self.npoints]; nsegments];

            debug!("conv using {nsegments} segments");

            for (n, coeff) in coeffs.iter().enumerate() {
                coeffs_padded[n / self.npoints][n % self.npoints] =
                    coeff / (2 * self.npoints) as PrcFmt;
            }

            for (segment, segment_f) in coeffs_padded.iter_mut().zip(coeffs_f.iter_mut()) {
                self.fft
                    .process_with_scratch(segment, segment_f, &mut self.scratch_fw)
                    .unwrap();
            }
            self.coeffs_f = coeffs_f;
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a FFT convolution config.
pub fn validate_config(conf: &config::ConvParameters) -> Res<()> {
    match conf {
        config::ConvParameters::Values { .. } | config::ConvParameters::Dummy { .. } => Ok(()),
        config::ConvParameters::Raw(params) => {
            let coeffs = filters::read_coeff_file(
                &params.filename,
                &params.format(),
                params.read_bytes_lines(),
                params.skip_bytes_lines(),
            )?;
            if coeffs.is_empty() {
                return Err(config::ConfigError::new("Conv coefficients are empty").into());
            }
            Ok(())
        }
        config::ConvParameters::Wav(params) => {
            let coeffs = filters::read_wav(&params.filename, params.channel())?;
            if coeffs.is_empty() {
                return Err(config::ConfigError::new("Conv coefficients are empty").into());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use crate::config::ConvParameters;
    use crate::filters::Filter;
    use crate::filters::fftconv::FftConv;
    use num_complex::Complex;

    fn is_close(left: PrcFmt, right: PrcFmt, maxdiff: PrcFmt) -> bool {
        println!("{left} - {right}");
        (left - right).abs() < maxdiff
    }

    fn compare_waveforms(left: Vec<PrcFmt>, right: Vec<PrcFmt>, maxdiff: PrcFmt) -> bool {
        for (val_l, val_r) in left.iter().zip(right.iter()) {
            if !is_close(*val_l, *val_r, maxdiff) {
                return false;
            }
        }
        true
    }

    #[test]
    fn check_result() {
        let coeffs = vec![0.5, 0.5];
        let conf = ConvParameters::Values { values: coeffs };
        let mut filter = FftConv::from_config("test", 8, conf);
        let mut wave1 = vec![1.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
        let expected = vec![0.5, 1.0, 1.0, 0.5, 0.0, -0.5, -0.5, 0.0];
        filter.process_waveform(&mut wave1).unwrap();
        assert!(compare_waveforms(wave1, expected, 1e-7));
    }

    #[test]
    fn check_result_segmented() {
        let mut coeffs = Vec::<PrcFmt>::new();
        for m in 0..32 {
            coeffs.push(m as PrcFmt);
        }
        let mut filter = FftConv::new("test", 8, &coeffs);
        let mut wave1 = vec![0.0 as PrcFmt; 8];
        let mut wave2 = vec![0.0 as PrcFmt; 8];
        let mut wave3 = vec![0.0 as PrcFmt; 8];
        let mut wave4 = vec![0.0 as PrcFmt; 8];
        let mut wave5 = vec![0.0 as PrcFmt; 8];

        wave1[0] = 1.0;
        filter.process_waveform(&mut wave1).unwrap();
        filter.process_waveform(&mut wave2).unwrap();
        filter.process_waveform(&mut wave3).unwrap();
        filter.process_waveform(&mut wave4).unwrap();
        filter.process_waveform(&mut wave5).unwrap();

        let exp1 = Vec::from(&coeffs[0..8]);
        let exp2 = Vec::from(&coeffs[8..16]);
        let exp3 = Vec::from(&coeffs[16..24]);
        let exp4 = Vec::from(&coeffs[24..32]);
        let exp5 = vec![0.0 as PrcFmt; 8];

        assert!(compare_waveforms(wave1, exp1, 1e-5));
        assert!(compare_waveforms(wave2, exp2, 1e-5));
        assert!(compare_waveforms(wave3, exp3, 1e-5));
        assert!(compare_waveforms(wave4, exp4, 1e-5));
        assert!(compare_waveforms(wave5, exp5, 1e-5));
    }

    fn make_test_vectors(len: usize) -> (Vec<Complex<PrcFmt>>, Vec<Complex<PrcFmt>>) {
        let a = (0..len)
            .map(|i| Complex::new((i + 1) as PrcFmt * 0.5, i as PrcFmt * 0.3 - 0.1))
            .collect();
        let b = (0..len)
            .map(|i| Complex::new(i as PrcFmt * 0.7 - 0.2, (i + 2) as PrcFmt * 0.4))
            .collect();
        (a, b)
    }

    // FMA rounds differently from scalar; SIMD results may differ by a few ULPs.
    #[cfg(not(feature = "32bit"))]
    const SIMD_TOL: PrcFmt = 1e-9;
    #[cfg(feature = "32bit")]
    const SIMD_TOL: PrcFmt = 1e-5;

    #[test]
    fn multiply_elements_scalar_known_values() {
        use super::multiply_elements_scalar;

        // (1 + 2i) * (3 + 4i) = (3-8) + (4+6)i = -5 + 10i
        let a = vec![Complex::new(1.0 as PrcFmt, 2.0)];
        let b = vec![Complex::new(3.0 as PrcFmt, 4.0)];
        let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0)];

        multiply_elements_scalar(&mut result, &a, &b);

        assert!(is_close(result[0].re, -5.0, SIMD_TOL));
        assert!(is_close(result[0].im, 10.0, SIMD_TOL));
    }

    #[test]
    fn multiply_add_elements_scalar_known_values() {
        use super::multiply_add_elements_scalar;

        // result starts at (1 + 1i), then += (1 + 2i) * (3 + 4i) = -5 + 10i
        // expected final: (-4 + 11i)
        let a = vec![Complex::new(1.0 as PrcFmt, 2.0)];
        let b = vec![Complex::new(3.0 as PrcFmt, 4.0)];
        let mut result = vec![Complex::new(1.0 as PrcFmt, 1.0)];

        multiply_add_elements_scalar(&mut result, &a, &b);

        assert!(is_close(result[0].re, -4.0, SIMD_TOL));
        assert!(is_close(result[0].im, 11.0, SIMD_TOL));
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    #[test]
    fn multiply_elements_avx_matches_scalar_all_lengths() {
        use super::{multiply_elements_avx_fma, multiply_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        // cover all f64 tail lengths: mod 2 (cleanup) and mod 8 (main loop).
        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 13, 15, 16, 17, 24, 25, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                assert!(
                    (e.re - r.re).abs() < SIMD_TOL,
                    "multiply_elements len={len} i={j}: re expected={} got={}",
                    e.re,
                    r.re
                );
                assert!(
                    (e.im - r.im).abs() < SIMD_TOL,
                    "multiply_elements len={len} i={j}: im expected={} got={}",
                    e.im,
                    r.im
                );
            }
        }
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_elements_avx_matches_scalar_all_lengths() {
        use super::{multiply_add_elements_avx_fma, multiply_add_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 13, 15, 16, 17, 24, 25, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            // Initialize with non-zero accumulators to exercise the accumulate path.
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_add_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                assert!(
                    (e.re - r.re).abs() < SIMD_TOL,
                    "multiply_add_elements len={len} i={j}: re expected={} got={}",
                    e.re,
                    r.re
                );
                assert!(
                    (e.im - r.im).abs() < SIMD_TOL,
                    "multiply_add_elements len={len} i={j}: im expected={} got={}",
                    e.im,
                    r.im
                );
            }
        }
    }

    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    #[test]
    fn multiply_elements_avx_f32_matches_scalar_all_lengths() {
        use super::{multiply_elements_avx_fma, multiply_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        // cover all f32 tail lengths: mod 4 (cleanup) and mod 16 (main loop).
        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 32, 33, 48, 49, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                assert!(
                    (e.re - r.re).abs() < simd_tol_for(e.re, r.re),
                    "multiply_elements(f32) len={len} i={j}: re expected={} got={}",
                    e.re,
                    r.re
                );
                assert!(
                    (e.im - r.im).abs() < simd_tol_for(e.im, r.im),
                    "multiply_elements(f32) len={len} i={j}: im expected={} got={}",
                    e.im,
                    r.im
                );
            }
        }
    }

    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    #[test]
    fn multiply_add_elements_avx_f32_matches_scalar_all_lengths() {
        use super::{multiply_add_elements_avx_fma, multiply_add_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 32, 33, 48, 49, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_add_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                assert!(
                    (e.re - r.re).abs() < simd_tol_for(e.re, r.re),
                    "multiply_add_elements(f32) len={len} i={j}: re expected={} got={}",
                    e.re,
                    r.re
                );
                assert!(
                    (e.im - r.im).abs() < simd_tol_for(e.im, r.im),
                    "multiply_add_elements(f32) len={len} i={j}: im expected={} got={}",
                    e.im,
                    r.im
                );
            }
        }
    }

    // ---- Stress tests ----

    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_multi_round_avx_matches_scalar() {
        use super::{
            multiply_add_elements_avx_fma, multiply_add_elements_scalar, multiply_elements_avx_fma,
            multiply_elements_scalar,
        };

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        let nsegments = 16;
        let len = 1025; // typical FFT spectrum size for chunksize=1024

        #[allow(clippy::type_complexity)]
        let segments: Vec<(Vec<Complex<PrcFmt>>, Vec<Complex<PrcFmt>>)> = (0..nsegments)
            .map(|s| {
                let a = (0..len)
                    .map(|i| {
                        Complex::new(
                            ((s * len + i + 1) as PrcFmt) * 0.001,
                            ((s * len + i) as PrcFmt) * 0.002 - 0.5,
                        )
                    })
                    .collect();
                let b = (0..len)
                    .map(|i| {
                        Complex::new(
                            ((s * len + i) as PrcFmt) * 0.003 + 0.1,
                            ((s * len + i + 2) as PrcFmt) * 0.004,
                        )
                    })
                    .collect();
                (a, b)
            })
            .collect();

        let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
        let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

        // First segment: overwrite.
        multiply_elements_scalar(&mut expected, &segments[0].0, &segments[0].1);
        unsafe { multiply_elements_avx_fma(&mut result, &segments[0].0, &segments[0].1) };

        // Remaining segments: accumulate.
        for (a, b) in &segments[1..] {
            multiply_add_elements_scalar(&mut expected, a, b);
            unsafe { multiply_add_elements_avx_fma(&mut result, a, b) };
        }

        for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
            let tol_re = simd_tol_for(e.re, r.re);
            let tol_im = simd_tol_for(e.im, r.im);
            assert!(
                (e.re - r.re).abs() < tol_re,
                "multi-round len={len} nseg={nsegments} i={j}: re expected={} got={}, diff={}",
                e.re,
                r.re,
                (e.re - r.re).abs()
            );
            assert!(
                (e.im - r.im).abs() < tol_im,
                "multi-round len={len} nseg={nsegments} i={j}: im expected={} got={}, diff={}",
                e.im,
                r.im,
                (e.im - r.im).abs()
            );
        }
    }

    // Relative tolerance helper: for large-magnitude values the absolute FMA
    // rounding difference can exceed SIMD_TOL, so we use
    //   max(SIMD_TOL, SIMD_REL_TOL * max(|expected|, |got|))
    // to scale the tolerance with the magnitude of the operands.
    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    const SIMD_REL_TOL: PrcFmt = 1e-14;
    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    const SIMD_REL_TOL: PrcFmt = 1e-6;

    #[cfg(target_arch = "x86_64")]
    fn simd_tol_for(a: PrcFmt, b: PrcFmt) -> PrcFmt {
        let mag = a.abs().max(b.abs());
        SIMD_TOL.max(SIMD_REL_TOL * mag)
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    #[test]
    fn multiply_elements_avx_large_buffers() {
        use super::{multiply_elements_avx_fma, multiply_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        // 4097: typical large FFT spectrum, exercises 4xYMM body + remainder.
        // 8192: power-of-two, exercises 4xYMM body with no remainder.
        // 8193: one past power-of-two, exercises scalar tail (1 element).
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            unsafe { multiply_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                let tol_re = simd_tol_for(e.re, r.re);
                let tol_im = simd_tol_for(e.im, r.im);
                assert!(
                    (e.re - r.re).abs() < tol_re,
                    "large multiply len={len} i={j}: re expected={} got={}, diff={}, tol={}",
                    e.re,
                    r.re,
                    (e.re - r.re).abs(),
                    tol_re
                );
                assert!(
                    (e.im - r.im).abs() < tol_im,
                    "large multiply len={len} i={j}: im expected={} got={}, diff={}, tol={}",
                    e.im,
                    r.im,
                    (e.im - r.im).abs(),
                    tol_im
                );
            }
        }
    }

    #[cfg(all(target_arch = "x86_64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_elements_avx_large_buffers() {
        use super::{multiply_add_elements_avx_fma, multiply_add_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            unsafe { multiply_add_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                let tol_re = simd_tol_for(e.re, r.re);
                let tol_im = simd_tol_for(e.im, r.im);
                assert!(
                    (e.re - r.re).abs() < tol_re,
                    "large multiply_add len={len} i={j}: re expected={} got={}, diff={}, tol={}",
                    e.re,
                    r.re,
                    (e.re - r.re).abs(),
                    tol_re
                );
                assert!(
                    (e.im - r.im).abs() < tol_im,
                    "large multiply_add len={len} i={j}: im expected={} got={}, diff={}, tol={}",
                    e.im,
                    r.im,
                    (e.im - r.im).abs(),
                    tol_im
                );
            }
        }
    }

    // ---- f32 stress tests ----

    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    #[test]
    fn multiply_add_multi_round_avx_f32_matches_scalar() {
        use super::{
            multiply_add_elements_avx_fma, multiply_add_elements_scalar, multiply_elements_avx_fma,
            multiply_elements_scalar,
        };

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        let nsegments = 16;
        let len = 1025;

        #[allow(clippy::type_complexity)]
        let segments: Vec<(Vec<Complex<PrcFmt>>, Vec<Complex<PrcFmt>>)> = (0..nsegments)
            .map(|s| {
                let a = (0..len)
                    .map(|i| {
                        Complex::new(
                            ((s * len + i + 1) as PrcFmt) * 0.001,
                            ((s * len + i) as PrcFmt) * 0.002 - 0.5,
                        )
                    })
                    .collect();
                let b = (0..len)
                    .map(|i| {
                        Complex::new(
                            ((s * len + i) as PrcFmt) * 0.003 + 0.1,
                            ((s * len + i + 2) as PrcFmt) * 0.004,
                        )
                    })
                    .collect();
                (a, b)
            })
            .collect();

        let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
        let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

        // First segment: overwrite.
        multiply_elements_scalar(&mut expected, &segments[0].0, &segments[0].1);
        // SAFETY: AVX and FMA support verified above.
        unsafe { multiply_elements_avx_fma(&mut result, &segments[0].0, &segments[0].1) };

        // Remaining segments: accumulate.
        for (a, b) in &segments[1..] {
            multiply_add_elements_scalar(&mut expected, a, b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_add_elements_avx_fma(&mut result, a, b) };
        }

        for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
            let tol_re = simd_tol_for(e.re, r.re);
            let tol_im = simd_tol_for(e.im, r.im);
            assert!(
                (e.re - r.re).abs() < tol_re,
                "multi-round(f32) len={len} nseg={nsegments} i={j}: re expected={} got={}, diff={}",
                e.re,
                r.re,
                (e.re - r.re).abs()
            );
            assert!(
                (e.im - r.im).abs() < tol_im,
                "multi-round(f32) len={len} nseg={nsegments} i={j}: im expected={} got={}, diff={}",
                e.im,
                r.im,
                (e.im - r.im).abs()
            );
        }
    }

    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    #[test]
    fn multiply_elements_avx_f32_large_buffers() {
        use super::{multiply_elements_avx_fma, multiply_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        // 4097: 256 full 4xYMM iters (4096 complex) + 1 scalar tail.
        // 8192: 512 full 4xYMM iters, no tail.
        // 8193: 512 full 4xYMM iters + 1 scalar tail.
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                let tol_re = simd_tol_for(e.re, r.re);
                let tol_im = simd_tol_for(e.im, r.im);
                assert!(
                    (e.re - r.re).abs() < tol_re,
                    "large multiply(f32) len={len} i={j}: re expected={} got={}, diff={}, tol={}",
                    e.re,
                    r.re,
                    (e.re - r.re).abs(),
                    tol_re
                );
                assert!(
                    (e.im - r.im).abs() < tol_im,
                    "large multiply(f32) len={len} i={j}: im expected={} got={}, diff={}, tol={}",
                    e.im,
                    r.im,
                    (e.im - r.im).abs(),
                    tol_im
                );
            }
        }
    }

    #[cfg(all(target_arch = "x86_64", feature = "32bit"))]
    #[test]
    fn multiply_add_elements_avx_f32_large_buffers() {
        use super::{multiply_add_elements_avx_fma, multiply_add_elements_scalar};

        if !is_x86_feature_detected!("avx") || !is_x86_feature_detected!("fma") {
            return;
        }

        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            // SAFETY: AVX and FMA support verified above.
            unsafe { multiply_add_elements_avx_fma(&mut result, &a, &b) };

            for (j, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                let tol_re = simd_tol_for(e.re, r.re);
                let tol_im = simd_tol_for(e.im, r.im);
                assert!(
                    (e.re - r.re).abs() < tol_re,
                    "large multiply_add(f32) len={len} i={j}: re expected={} got={}, diff={}, tol={}",
                    e.re,
                    r.re,
                    (e.re - r.re).abs(),
                    tol_re
                );
                assert!(
                    (e.im - r.im).abs() < tol_im,
                    "large multiply_add(f32) len={len} i={j}: im expected={} got={}, diff={}, tol={}",
                    e.im,
                    r.im,
                    (e.im - r.im).abs(),
                    tol_im
                );
            }
        }
    }
}
