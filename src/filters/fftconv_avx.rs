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

use num_complex::Complex;

// AVX + FMA SIMD kernels for complex multiply/multiply-add.
// result.re = a.re*b.re - a.im*b.im, result.im = a.re*b.im + a.im*b.re
// Uses _mm256_fmaddsub for the sign pattern; 4xYMM unrolled + 1xYMM cleanup + scalar tail.
// multiply-add computes the product via fmaddsub, then adds the accumulator with _mm256_add.

// Both precisions are always compiled, regardless of which one `CamillaFloat` is set
// to. The unused one is dead code that the linker drops, and in exchange both
// stay under the compiler's eye and are covered by the tests below on every
// build.

// f64: each YMM holds 2 Complex<f64>; 4xYMM = 8 complex per iter.

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx", enable = "fma")]
pub(super) unsafe fn multiply_elements_avx_fma_f64(
    result: &mut [Complex<f64>],
    slice_a: &[Complex<f64>],
    slice_b: &[Complex<f64>],
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx", enable = "fma")]
pub(super) unsafe fn multiply_add_elements_avx_fma_f64(
    result: &mut [Complex<f64>],
    slice_a: &[Complex<f64>],
    slice_b: &[Complex<f64>],
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx", enable = "fma")]
pub(super) unsafe fn multiply_elements_avx_fma_f32(
    result: &mut [Complex<f32>],
    slice_a: &[Complex<f32>],
    slice_b: &[Complex<f32>],
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx", enable = "fma")]
pub(super) unsafe fn multiply_add_elements_avx_fma_f32(
    result: &mut [Complex<f32>],
    slice_a: &[Complex<f32>],
    slice_b: &[Complex<f32>],
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
pub(super) fn has_avx_fma() -> bool {
    use std::sync::OnceLock;
    static DETECTED: OnceLock<bool> = OnceLock::new();
    *DETECTED.get_or_init(|| is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma"))
}
#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::super::{multiply_add_elements_scalar, multiply_elements_scalar};
    use super::{
        multiply_add_elements_avx_fma_f32, multiply_add_elements_avx_fma_f64,
        multiply_elements_avx_fma_f32, multiply_elements_avx_fma_f64,
    };
    use num_complex::Complex;

    // Both precisions are tested on every build. FMA rounds differently from the
    // scalar reference, so results are compared with a tolerance that scales with
    // the magnitude of the operands: max(ABS_TOL, REL_TOL * max(|expected|, |got|)).
    macro_rules! kernel_tests {
        ($modname:ident, $t:ty, $mul:ident, $mul_add:ident, $abs_tol:expr, $rel_tol:expr) => {
            mod $modname {
                use super::*;

                const ABS_TOL: $t = $abs_tol;
                const REL_TOL: $t = $rel_tol;

                // Covers every tail shape of both kernels: the f64 main loop steps by 8
                // complex, the f32 one by 16, each with a 1xYMM cleanup and a scalar tail.
                const LENGTHS: [usize; 21] = [
                    0, 1, 2, 3, 4, 5, 7, 8, 9, 13, 15, 16, 17, 24, 25, 32, 33, 48, 49, 100, 1025,
                ];

                // 4097: large FFT spectrum, main loop plus remainder.
                // 8192: power of two, main loop with no remainder.
                // 8193: one past, exercises the cleanup path.
                const LARGE_LENGTHS: [usize; 3] = [4097, 8192, 8193];

                fn avx_available() -> bool {
                    is_x86_feature_detected!("avx") && is_x86_feature_detected!("fma")
                }

                fn make_test_vectors(len: usize) -> (Vec<Complex<$t>>, Vec<Complex<$t>>) {
                    let a = (0..len)
                        .map(|i| Complex::new((i + 1) as $t * 0.5, i as $t * 0.3 - 0.1))
                        .collect();
                    let b = (0..len)
                        .map(|i| Complex::new(i as $t * 0.7 - 0.2, (i + 2) as $t * 0.4))
                        .collect();
                    (a, b)
                }

                fn make_accumulator(len: usize) -> Vec<Complex<$t>> {
                    (0..len)
                        .map(|i| Complex::new(i as $t * 0.1, -(i as $t) * 0.2))
                        .collect()
                }

                fn tol_for(a: $t, b: $t) -> $t {
                    ABS_TOL.max(REL_TOL * a.abs().max(b.abs()))
                }

                fn assert_close(expected: &[Complex<$t>], result: &[Complex<$t>], ctx: &str) {
                    for (i, (e, r)) in expected.iter().zip(result.iter()).enumerate() {
                        assert!(
                            (e.re - r.re).abs() < tol_for(e.re, r.re),
                            "{ctx} i={i}: re expected={} got={} diff={}",
                            e.re,
                            r.re,
                            (e.re - r.re).abs()
                        );
                        assert!(
                            (e.im - r.im).abs() < tol_for(e.im, r.im),
                            "{ctx} i={i}: im expected={} got={} diff={}",
                            e.im,
                            r.im,
                            (e.im - r.im).abs()
                        );
                    }
                }

                #[test]
                fn multiply_matches_scalar_all_lengths() {
                    if !avx_available() {
                        return;
                    }
                    for len in LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = vec![Complex::new(0.0 as $t, 0.0); len];
                        let mut result = vec![Complex::new(0.0 as $t, 0.0); len];

                        multiply_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: AVX and FMA availability checked above.
                        unsafe { $mul(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("multiply len={len}"));
                    }
                }

                #[test]
                fn multiply_add_matches_scalar_all_lengths() {
                    if !avx_available() {
                        return;
                    }
                    for len in LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        // Non-zero accumulator, so the accumulate path is exercised.
                        let mut expected = make_accumulator(len);
                        let mut result = expected.clone();

                        multiply_add_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: AVX and FMA availability checked above.
                        unsafe { $mul_add(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("multiply_add len={len}"));
                    }
                }

                #[test]
                fn multiply_large_buffers() {
                    if !avx_available() {
                        return;
                    }
                    for len in LARGE_LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = vec![Complex::new(0.0 as $t, 0.0); len];
                        let mut result = vec![Complex::new(0.0 as $t, 0.0); len];

                        multiply_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: AVX and FMA availability checked above.
                        unsafe { $mul(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("large multiply len={len}"));
                    }
                }

                #[test]
                fn multiply_add_large_buffers() {
                    if !avx_available() {
                        return;
                    }
                    for len in LARGE_LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = make_accumulator(len);
                        let mut result = expected.clone();

                        multiply_add_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: AVX and FMA availability checked above.
                        unsafe { $mul_add(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("large multiply_add len={len}"));
                    }
                }

                // Mimics the convolution inner loop: one overwriting multiply followed by
                // repeated accumulates, so rounding differences have a chance to build up.
                #[test]
                fn multiply_add_multi_round_matches_scalar() {
                    if !avx_available() {
                        return;
                    }
                    let nsegments = 16;
                    let len = 1025; // typical FFT spectrum size for chunksize=1024

                    #[allow(clippy::type_complexity)]
                    let segments: Vec<(Vec<Complex<$t>>, Vec<Complex<$t>>)> = (0..nsegments)
                        .map(|s| {
                            let a = (0..len)
                                .map(|i| {
                                    Complex::new(
                                        ((s * len + i + 1) as $t) * 0.001,
                                        ((s * len + i) as $t) * 0.002 - 0.5,
                                    )
                                })
                                .collect();
                            let b = (0..len)
                                .map(|i| {
                                    Complex::new(
                                        ((s * len + i) as $t) * 0.003 + 0.1,
                                        ((s * len + i + 2) as $t) * 0.004,
                                    )
                                })
                                .collect();
                            (a, b)
                        })
                        .collect();

                    let mut expected = vec![Complex::new(0.0 as $t, 0.0); len];
                    let mut result = vec![Complex::new(0.0 as $t, 0.0); len];

                    // First segment overwrites, the rest accumulate.
                    multiply_elements_scalar(&mut expected, &segments[0].0, &segments[0].1);
                    // SAFETY: AVX and FMA availability checked above.
                    unsafe { $mul(&mut result, &segments[0].0, &segments[0].1) };

                    for (a, b) in &segments[1..] {
                        multiply_add_elements_scalar(&mut expected, a, b);
                        // SAFETY: AVX and FMA availability checked above.
                        unsafe { $mul_add(&mut result, a, b) };
                    }

                    assert_close(
                        &expected,
                        &result,
                        &format!("multi-round len={len} nseg={nsegments}"),
                    );
                }
            }
        };
    }

    kernel_tests!(
        f64_kernels,
        f64,
        multiply_elements_avx_fma_f64,
        multiply_add_elements_avx_fma_f64,
        1e-9,
        1e-14
    );
    kernel_tests!(
        f32_kernels,
        f32,
        multiply_elements_avx_fma_f32,
        multiply_add_elements_avx_fma_f32,
        1e-5,
        1e-6
    );
}
