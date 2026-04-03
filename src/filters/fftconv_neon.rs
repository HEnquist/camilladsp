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

use crate::PrcFmt;
use num_complex::Complex;

// NEON SIMD kernels for complex multiply/multiply-add on aarch64.
// result.re = a.re*b.re - a.im*b.im, result.im = a.re*b.im + a.im*b.re
//
// Strategy: broadcast a.re and a.im, swap b's components, apply a sign mask
// [-1, 1] to get the correct subtract/add pattern, then use FMA:
//   result = a_re * b + a_im * b_swap_signed
//
// For multiply_add (accumulate), the kernel computes the complex product first,
// then adds the accumulator via a separate add:
//   prod = a_re * b + a_im * b_swap_signed  (via vmulq + vfmaq)
//   result = acc + prod                     (via vaddq)
//
// 4x-register unrolled main loop + 1x cleanup + scalar tail (f32 only).

// f64: each 128-bit NEON register holds 1 Complex<f64>; 4x unroll = 4 complex per iter.

#[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_elements_neon(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::aarch64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f64;
    let a_ptr = slice_a.as_ptr() as *const f64;
    let b_ptr = slice_b.as_ptr() as *const f64;

    // Sign mask for complex multiply: negate real part of cross-product.
    // [-1.0, 1.0]: after swapping b to [b.im, b.re], multiplying by this gives [-b.im, b.re].
    // SAFETY: vld1q_f64 is safe to call here; NEON is mandatory on aarch64.
    let sign: float64x2_t = unsafe {
        let arr: [f64; 2] = [-1.0, 1.0];
        vld1q_f64(arr.as_ptr())
    };

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4x main loop: 4 complex f64 per iteration ----
        let chunks_4 = len / 4;
        for i in 0..chunks_4 {
            let off = i * 8; // 4 complex * 2 f64/complex

            let a0 = vld1q_f64(a_ptr.add(off));
            let b0 = vld1q_f64(b_ptr.add(off));
            let a1 = vld1q_f64(a_ptr.add(off + 2));
            let b1 = vld1q_f64(b_ptr.add(off + 2));
            let a2 = vld1q_f64(a_ptr.add(off + 4));
            let b2 = vld1q_f64(b_ptr.add(off + 4));
            let a3 = vld1q_f64(a_ptr.add(off + 6));
            let b3 = vld1q_f64(b_ptr.add(off + 6));

            // Broadcast real and imaginary parts of a.
            let a_re0 = vdupq_laneq_f64::<0>(a0);
            let a_im0 = vdupq_laneq_f64::<1>(a0);
            let a_re1 = vdupq_laneq_f64::<0>(a1);
            let a_im1 = vdupq_laneq_f64::<1>(a1);
            let a_re2 = vdupq_laneq_f64::<0>(a2);
            let a_im2 = vdupq_laneq_f64::<1>(a2);
            let a_re3 = vdupq_laneq_f64::<0>(a3);
            let a_im3 = vdupq_laneq_f64::<1>(a3);

            // Swap real/imag of b, then apply sign mask: [-b.im, b.re].
            let b_sw0 = vmulq_f64(vextq_f64::<1>(b0, b0), sign);
            let b_sw1 = vmulq_f64(vextq_f64::<1>(b1, b1), sign);
            let b_sw2 = vmulq_f64(vextq_f64::<1>(b2, b2), sign);
            let b_sw3 = vmulq_f64(vextq_f64::<1>(b3, b3), sign);

            // result = a_re * b + a_im * b_sw_signed
            //   [a.re*b.re + a.im*(-b.im), a.re*b.im + a.im*b.re]
            //   = [a.re*b.re - a.im*b.im, a.re*b.im + a.im*b.re]
            let r0 = vfmaq_f64(vmulq_f64(a_re0, b0), a_im0, b_sw0);
            let r1 = vfmaq_f64(vmulq_f64(a_re1, b1), a_im1, b_sw1);
            let r2 = vfmaq_f64(vmulq_f64(a_re2, b2), a_im2, b_sw2);
            let r3 = vfmaq_f64(vmulq_f64(a_re3, b3), a_im3, b_sw3);

            vst1q_f64(r_ptr.add(off), r0);
            vst1q_f64(r_ptr.add(off + 2), r1);
            vst1q_f64(r_ptr.add(off + 4), r2);
            vst1q_f64(r_ptr.add(off + 6), r3);
        }

        // ---- 1x cleanup: 1 complex f64 per step ----
        let tail_start = chunks_4 * 4;
        for j in 0..(len - tail_start) {
            let off = (tail_start + j) * 2;
            let a0 = vld1q_f64(a_ptr.add(off));
            let b0 = vld1q_f64(b_ptr.add(off));
            let a_re0 = vdupq_laneq_f64::<0>(a0);
            let a_im0 = vdupq_laneq_f64::<1>(a0);
            let b_sw0 = vmulq_f64(vextq_f64::<1>(b0, b0), sign);
            vst1q_f64(
                r_ptr.add(off),
                vfmaq_f64(vmulq_f64(a_re0, b0), a_im0, b_sw0),
            );
        }
        // No scalar tail: each NEON register handles exactly 1 Complex<f64>.
    }
}

#[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_add_elements_neon(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::aarch64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f64;
    let a_ptr = slice_a.as_ptr() as *const f64;
    let b_ptr = slice_b.as_ptr() as *const f64;

    // SAFETY: vld1q_f64 is safe to call here; NEON is mandatory on aarch64.
    let sign: float64x2_t = unsafe {
        let arr: [f64; 2] = [-1.0, 1.0];
        vld1q_f64(arr.as_ptr())
    };

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4x main loop: 4 complex f64 per iteration ----
        let chunks_4 = len / 4;
        for i in 0..chunks_4 {
            let off = i * 8;

            let acc0 = vld1q_f64(r_ptr.add(off));
            let acc1 = vld1q_f64(r_ptr.add(off + 2));
            let acc2 = vld1q_f64(r_ptr.add(off + 4));
            let acc3 = vld1q_f64(r_ptr.add(off + 6));

            let a0 = vld1q_f64(a_ptr.add(off));
            let b0 = vld1q_f64(b_ptr.add(off));
            let a1 = vld1q_f64(a_ptr.add(off + 2));
            let b1 = vld1q_f64(b_ptr.add(off + 2));
            let a2 = vld1q_f64(a_ptr.add(off + 4));
            let b2 = vld1q_f64(b_ptr.add(off + 4));
            let a3 = vld1q_f64(a_ptr.add(off + 6));
            let b3 = vld1q_f64(b_ptr.add(off + 6));

            let a_re0 = vdupq_laneq_f64::<0>(a0);
            let a_im0 = vdupq_laneq_f64::<1>(a0);
            let a_re1 = vdupq_laneq_f64::<0>(a1);
            let a_im1 = vdupq_laneq_f64::<1>(a1);
            let a_re2 = vdupq_laneq_f64::<0>(a2);
            let a_im2 = vdupq_laneq_f64::<1>(a2);
            let a_re3 = vdupq_laneq_f64::<0>(a3);
            let a_im3 = vdupq_laneq_f64::<1>(a3);

            let b_sw0 = vmulq_f64(vextq_f64::<1>(b0, b0), sign);
            let b_sw1 = vmulq_f64(vextq_f64::<1>(b1, b1), sign);
            let b_sw2 = vmulq_f64(vextq_f64::<1>(b2, b2), sign);
            let b_sw3 = vmulq_f64(vextq_f64::<1>(b3, b3), sign);

            // Compute product a*b, then add accumulator.
            let prod0 = vfmaq_f64(vmulq_f64(a_re0, b0), a_im0, b_sw0);
            let prod1 = vfmaq_f64(vmulq_f64(a_re1, b1), a_im1, b_sw1);
            let prod2 = vfmaq_f64(vmulq_f64(a_re2, b2), a_im2, b_sw2);
            let prod3 = vfmaq_f64(vmulq_f64(a_re3, b3), a_im3, b_sw3);

            vst1q_f64(r_ptr.add(off), vaddq_f64(acc0, prod0));
            vst1q_f64(r_ptr.add(off + 2), vaddq_f64(acc1, prod1));
            vst1q_f64(r_ptr.add(off + 4), vaddq_f64(acc2, prod2));
            vst1q_f64(r_ptr.add(off + 6), vaddq_f64(acc3, prod3));
        }

        // ---- 1x cleanup: 1 complex f64 per step ----
        let tail_start = chunks_4 * 4;
        for j in 0..(len - tail_start) {
            let off = (tail_start + j) * 2;
            let acc0 = vld1q_f64(r_ptr.add(off));
            let a0 = vld1q_f64(a_ptr.add(off));
            let b0 = vld1q_f64(b_ptr.add(off));
            let a_re0 = vdupq_laneq_f64::<0>(a0);
            let a_im0 = vdupq_laneq_f64::<1>(a0);
            let b_sw0 = vmulq_f64(vextq_f64::<1>(b0, b0), sign);
            let prod0 = vfmaq_f64(vmulq_f64(a_re0, b0), a_im0, b_sw0);
            vst1q_f64(r_ptr.add(off), vaddq_f64(acc0, prod0));
        }
        // No scalar tail: each NEON register handles exactly 1 Complex<f64>.
    }
}

// f32: each 128-bit NEON register holds 2 Complex<f32>; 4x unroll = 8 complex per iter.

#[cfg(all(target_arch = "aarch64", feature = "32bit"))]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_elements_neon(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::aarch64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f32;
    let a_ptr = slice_a.as_ptr() as *const f32;
    let b_ptr = slice_b.as_ptr() as *const f32;

    // Sign mask: [-1.0, 1.0, -1.0, 1.0] to negate the real cross-product lanes.
    // SAFETY: vld1q_f32 is safe to call here; NEON is mandatory on aarch64.
    let sign: float32x4_t = unsafe {
        let arr: [f32; 4] = [-1.0, 1.0, -1.0, 1.0];
        vld1q_f32(arr.as_ptr())
    };

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4x main loop: 8 complex f32 per iteration ----
        let chunks_8 = len / 8;
        for i in 0..chunks_8 {
            let off = i * 16; // 8 complex * 2 f32/complex

            let a0 = vld1q_f32(a_ptr.add(off));
            let b0 = vld1q_f32(b_ptr.add(off));
            let a1 = vld1q_f32(a_ptr.add(off + 4));
            let b1 = vld1q_f32(b_ptr.add(off + 4));
            let a2 = vld1q_f32(a_ptr.add(off + 8));
            let b2 = vld1q_f32(b_ptr.add(off + 8));
            let a3 = vld1q_f32(a_ptr.add(off + 12));
            let b3 = vld1q_f32(b_ptr.add(off + 12));

            // Broadcast real/imag: trn1 duplicates even lanes, trn2 duplicates odd lanes.
            let (a_re0, a_im0) = (vtrn1q_f32(a0, a0), vtrn2q_f32(a0, a0));
            let (a_re1, a_im1) = (vtrn1q_f32(a1, a1), vtrn2q_f32(a1, a1));
            let (a_re2, a_im2) = (vtrn1q_f32(a2, a2), vtrn2q_f32(a2, a2));
            let (a_re3, a_im3) = (vtrn1q_f32(a3, a3), vtrn2q_f32(a3, a3));

            // Swap pairs within 64-bit halves, then apply sign mask.
            let b_sw0 = vmulq_f32(vrev64q_f32(b0), sign);
            let b_sw1 = vmulq_f32(vrev64q_f32(b1), sign);
            let b_sw2 = vmulq_f32(vrev64q_f32(b2), sign);
            let b_sw3 = vmulq_f32(vrev64q_f32(b3), sign);

            let r0 = vfmaq_f32(vmulq_f32(a_re0, b0), a_im0, b_sw0);
            let r1 = vfmaq_f32(vmulq_f32(a_re1, b1), a_im1, b_sw1);
            let r2 = vfmaq_f32(vmulq_f32(a_re2, b2), a_im2, b_sw2);
            let r3 = vfmaq_f32(vmulq_f32(a_re3, b3), a_im3, b_sw3);

            vst1q_f32(r_ptr.add(off), r0);
            vst1q_f32(r_ptr.add(off + 4), r1);
            vst1q_f32(r_ptr.add(off + 8), r2);
            vst1q_f32(r_ptr.add(off + 12), r3);
        }

        // ---- 1x cleanup: 2 complex f32 per step ----
        let tail_start = chunks_8 * 8;
        let remaining_pairs = (len - tail_start) / 2;
        for j in 0..remaining_pairs {
            let off = (tail_start + j * 2) * 2;
            let a0 = vld1q_f32(a_ptr.add(off));
            let b0 = vld1q_f32(b_ptr.add(off));
            let a_re0 = vtrn1q_f32(a0, a0);
            let a_im0 = vtrn2q_f32(a0, a0);
            let b_sw0 = vmulq_f32(vrev64q_f32(b0), sign);
            vst1q_f32(
                r_ptr.add(off),
                vfmaq_f32(vmulq_f32(a_re0, b0), a_im0, b_sw0),
            );
        }

        // ---- Scalar tail: 0-1 remaining Complex<f32> ----
        let simd_done = tail_start + remaining_pairs * 2;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) = *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

#[cfg(all(target_arch = "aarch64", feature = "32bit"))]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_add_elements_neon(
    result: &mut [Complex<PrcFmt>],
    slice_a: &[Complex<PrcFmt>],
    slice_b: &[Complex<PrcFmt>],
) {
    use std::arch::aarch64::*;

    let len = result.len();
    assert!(slice_a.len() >= len && slice_b.len() >= len);
    let r_ptr = result.as_mut_ptr() as *mut f32;
    let a_ptr = slice_a.as_ptr() as *const f32;
    let b_ptr = slice_b.as_ptr() as *const f32;

    // SAFETY: vld1q_f32 is safe to call here; NEON is mandatory on aarch64.
    let sign: float32x4_t = unsafe {
        let arr: [f32; 4] = [-1.0, 1.0, -1.0, 1.0];
        vld1q_f32(arr.as_ptr())
    };

    // SAFETY: slice_a and slice_b are at least `len` elements long (asserted above);
    // all pointer offsets stay within slice bounds.
    unsafe {
        // ---- 4x main loop: 8 complex f32 per iteration ----
        let chunks_8 = len / 8;
        for i in 0..chunks_8 {
            let off = i * 16;

            let acc0 = vld1q_f32(r_ptr.add(off));
            let acc1 = vld1q_f32(r_ptr.add(off + 4));
            let acc2 = vld1q_f32(r_ptr.add(off + 8));
            let acc3 = vld1q_f32(r_ptr.add(off + 12));

            let a0 = vld1q_f32(a_ptr.add(off));
            let b0 = vld1q_f32(b_ptr.add(off));
            let a1 = vld1q_f32(a_ptr.add(off + 4));
            let b1 = vld1q_f32(b_ptr.add(off + 4));
            let a2 = vld1q_f32(a_ptr.add(off + 8));
            let b2 = vld1q_f32(b_ptr.add(off + 8));
            let a3 = vld1q_f32(a_ptr.add(off + 12));
            let b3 = vld1q_f32(b_ptr.add(off + 12));

            let (a_re0, a_im0) = (vtrn1q_f32(a0, a0), vtrn2q_f32(a0, a0));
            let (a_re1, a_im1) = (vtrn1q_f32(a1, a1), vtrn2q_f32(a1, a1));
            let (a_re2, a_im2) = (vtrn1q_f32(a2, a2), vtrn2q_f32(a2, a2));
            let (a_re3, a_im3) = (vtrn1q_f32(a3, a3), vtrn2q_f32(a3, a3));

            let b_sw0 = vmulq_f32(vrev64q_f32(b0), sign);
            let b_sw1 = vmulq_f32(vrev64q_f32(b1), sign);
            let b_sw2 = vmulq_f32(vrev64q_f32(b2), sign);
            let b_sw3 = vmulq_f32(vrev64q_f32(b3), sign);

            // Compute product a*b, then add accumulator.
            let prod0 = vfmaq_f32(vmulq_f32(a_re0, b0), a_im0, b_sw0);
            let prod1 = vfmaq_f32(vmulq_f32(a_re1, b1), a_im1, b_sw1);
            let prod2 = vfmaq_f32(vmulq_f32(a_re2, b2), a_im2, b_sw2);
            let prod3 = vfmaq_f32(vmulq_f32(a_re3, b3), a_im3, b_sw3);

            vst1q_f32(r_ptr.add(off), vaddq_f32(acc0, prod0));
            vst1q_f32(r_ptr.add(off + 4), vaddq_f32(acc1, prod1));
            vst1q_f32(r_ptr.add(off + 8), vaddq_f32(acc2, prod2));
            vst1q_f32(r_ptr.add(off + 12), vaddq_f32(acc3, prod3));
        }

        // ---- 1x cleanup: 2 complex f32 per step ----
        let tail_start = chunks_8 * 8;
        let remaining_pairs = (len - tail_start) / 2;
        for j in 0..remaining_pairs {
            let off = (tail_start + j * 2) * 2;
            let acc0 = vld1q_f32(r_ptr.add(off));
            let a0 = vld1q_f32(a_ptr.add(off));
            let b0 = vld1q_f32(b_ptr.add(off));
            let a_re0 = vtrn1q_f32(a0, a0);
            let a_im0 = vtrn2q_f32(a0, a0);
            let b_sw0 = vmulq_f32(vrev64q_f32(b0), sign);
            let prod0 = vfmaq_f32(vmulq_f32(a_re0, b0), a_im0, b_sw0);
            vst1q_f32(r_ptr.add(off), vaddq_f32(acc0, prod0));
        }

        // ---- Scalar tail: 0-1 remaining Complex<f32> ----
        let simd_done = tail_start + remaining_pairs * 2;
        for i in simd_done..len {
            *result.get_unchecked_mut(i) += *slice_a.get_unchecked(i) * *slice_b.get_unchecked(i);
        }
    }
}

// NEON is architecturally mandatory on all AArch64 implementations (ARM
// Architecture Reference Manual, section A1.1), so no runtime detection is
// needed. The function is kept to match the dispatch pattern of the AVX path,
// where runtime detection is genuinely required.
#[cfg(target_arch = "aarch64")]
#[inline]
pub(super) fn has_neon() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use crate::PrcFmt;
    use num_complex::Complex;

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

    // Relative tolerance helper: for large-magnitude values the absolute FMA
    // rounding difference can exceed SIMD_TOL, so we use
    //   max(SIMD_TOL, SIMD_REL_TOL * max(|expected|, |got|))
    // to scale the tolerance with the magnitude of the operands.
    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    const SIMD_REL_TOL: PrcFmt = 1e-14;
    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    const SIMD_REL_TOL: PrcFmt = 1e-6;

    #[cfg(target_arch = "aarch64")]
    fn simd_tol_for(a: PrcFmt, b: PrcFmt) -> PrcFmt {
        let mag = a.abs().max(b.abs());
        SIMD_TOL.max(SIMD_REL_TOL * mag)
    }

    // ---- f64 tests ----

    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    #[test]
    fn multiply_elements_neon_matches_scalar_all_lengths() {
        use super::super::multiply_elements_scalar;
        use super::multiply_elements_neon;
        // Cover all f64 tail lengths: mod 4 (main loop) and individual cleanup.
        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 13, 15, 16, 17, 24, 25, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_elements_neon_matches_scalar_all_lengths() {
        use super::super::multiply_add_elements_scalar;
        use super::multiply_add_elements_neon;
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
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_add_elements_neon(&mut result, &a, &b) };

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

    // ---- f32 tests ----

    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    #[test]
    fn multiply_elements_neon_f32_matches_scalar_all_lengths() {
        use super::super::multiply_elements_scalar;
        use super::multiply_elements_neon;
        // Cover all f32 tail lengths: mod 2 (cleanup) and mod 8 (main loop).
        for len in [
            0usize, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 32, 33, 48, 49, 100, 1025,
        ] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    #[test]
    fn multiply_add_elements_neon_f32_matches_scalar_all_lengths() {
        use super::super::multiply_add_elements_scalar;
        use super::multiply_add_elements_neon;
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
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_add_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_multi_round_neon_matches_scalar() {
        use super::super::{multiply_add_elements_scalar, multiply_elements_scalar};
        use super::{multiply_add_elements_neon, multiply_elements_neon};
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
        unsafe { multiply_elements_neon(&mut result, &segments[0].0, &segments[0].1) };

        // Remaining segments: accumulate.
        for (a, b) in &segments[1..] {
            multiply_add_elements_scalar(&mut expected, a, b);
            unsafe { multiply_add_elements_neon(&mut result, a, b) };
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

    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    #[test]
    fn multiply_elements_neon_large_buffers() {
        use super::super::multiply_elements_scalar;
        use super::multiply_elements_neon;
        // 4097: typical large FFT spectrum, exercises 4x body + remainder.
        // 8192: power-of-two, exercises 4x body with no remainder.
        // 8193: one past power-of-two, exercises cleanup (1 element).
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            unsafe { multiply_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", not(feature = "32bit")))]
    #[test]
    fn multiply_add_elements_neon_large_buffers() {
        use super::super::multiply_add_elements_scalar;
        use super::multiply_add_elements_neon;
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            unsafe { multiply_add_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    #[test]
    fn multiply_add_multi_round_neon_f32_matches_scalar() {
        use super::super::{multiply_add_elements_scalar, multiply_elements_scalar};
        use super::{multiply_add_elements_neon, multiply_elements_neon};
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
        // SAFETY: NEON is mandatory on all AArch64 implementations.
        unsafe { multiply_elements_neon(&mut result, &segments[0].0, &segments[0].1) };

        // Remaining segments: accumulate.
        for (a, b) in &segments[1..] {
            multiply_add_elements_scalar(&mut expected, a, b);
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_add_elements_neon(&mut result, a, b) };
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

    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    #[test]
    fn multiply_elements_neon_f32_large_buffers() {
        use super::super::multiply_elements_scalar;
        use super::multiply_elements_neon;
        // 4097: exercises 4x body + remainder.
        // 8192: power-of-two, no remainder.
        // 8193: one past power-of-two, exercises scalar tail (1 element).
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let mut expected = vec![Complex::new(0.0 as PrcFmt, 0.0); len];
            let mut result = vec![Complex::new(0.0 as PrcFmt, 0.0); len];

            multiply_elements_scalar(&mut expected, &a, &b);
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_elements_neon(&mut result, &a, &b) };

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

    #[cfg(all(target_arch = "aarch64", feature = "32bit"))]
    #[test]
    fn multiply_add_elements_neon_f32_large_buffers() {
        use super::super::multiply_add_elements_scalar;
        use super::multiply_add_elements_neon;
        for len in [4097usize, 8192, 8193] {
            let (a, b) = make_test_vectors(len);
            let init: Vec<Complex<PrcFmt>> = (0..len)
                .map(|i| Complex::new(i as PrcFmt * 0.1, -(i as PrcFmt) * 0.2))
                .collect();

            let mut expected = init.clone();
            let mut result = init.clone();

            multiply_add_elements_scalar(&mut expected, &a, &b);
            // SAFETY: NEON is mandatory on all AArch64 implementations.
            unsafe { multiply_add_elements_neon(&mut result, &a, &b) };

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
