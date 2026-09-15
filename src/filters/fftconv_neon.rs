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
//
// Both precisions are always compiled, regardless of which one `CamillaFloat` is set
// to. The unused one is dead code that the linker drops, and in exchange both
// stay under the compiler's eye and are covered by the tests below on every
// build.

// f64: each 128-bit NEON register holds 1 Complex<f64>; 4x unroll = 4 complex per iter.

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_elements_neon_f64(
    result: &mut [Complex<f64>],
    slice_a: &[Complex<f64>],
    slice_b: &[Complex<f64>],
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

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_add_elements_neon_f64(
    result: &mut [Complex<f64>],
    slice_a: &[Complex<f64>],
    slice_b: &[Complex<f64>],
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

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_elements_neon_f32(
    result: &mut [Complex<f32>],
    slice_a: &[Complex<f32>],
    slice_b: &[Complex<f32>],
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

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(super) unsafe fn multiply_add_elements_neon_f32(
    result: &mut [Complex<f32>],
    slice_a: &[Complex<f32>],
    slice_b: &[Complex<f32>],
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
#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::super::{multiply_add_elements_scalar, multiply_elements_scalar};
    use super::{
        multiply_add_elements_neon_f32, multiply_add_elements_neon_f64, multiply_elements_neon_f32,
        multiply_elements_neon_f64,
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

                // Covers every tail shape of both kernels: the f64 main loop steps by 4
                // complex, the f32 one by 8, with a 2-wide cleanup and a 1-element scalar
                // tail in the f32 case.
                const LENGTHS: [usize; 21] = [
                    0, 1, 2, 3, 4, 5, 7, 8, 9, 13, 15, 16, 17, 24, 25, 32, 33, 48, 49, 100, 1025,
                ];

                // 4097: large FFT spectrum, main loop plus remainder.
                // 8192: power of two, main loop with no remainder.
                // 8193: one past, exercises the cleanup path.
                const LARGE_LENGTHS: [usize; 3] = [4097, 8192, 8193];

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
                    for len in LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = vec![Complex::new(0.0 as $t, 0.0); len];
                        let mut result = vec![Complex::new(0.0 as $t, 0.0); len];

                        multiply_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: NEON is mandatory on all AArch64 implementations.
                        unsafe { $mul(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("multiply len={len}"));
                    }
                }

                #[test]
                fn multiply_add_matches_scalar_all_lengths() {
                    for len in LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        // Non-zero accumulator, so the accumulate path is exercised.
                        let mut expected = make_accumulator(len);
                        let mut result = expected.clone();

                        multiply_add_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: NEON is mandatory on all AArch64 implementations.
                        unsafe { $mul_add(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("multiply_add len={len}"));
                    }
                }

                #[test]
                fn multiply_large_buffers() {
                    for len in LARGE_LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = vec![Complex::new(0.0 as $t, 0.0); len];
                        let mut result = vec![Complex::new(0.0 as $t, 0.0); len];

                        multiply_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: NEON is mandatory on all AArch64 implementations.
                        unsafe { $mul(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("large multiply len={len}"));
                    }
                }

                #[test]
                fn multiply_add_large_buffers() {
                    for len in LARGE_LENGTHS {
                        let (a, b) = make_test_vectors(len);
                        let mut expected = make_accumulator(len);
                        let mut result = expected.clone();

                        multiply_add_elements_scalar(&mut expected, &a, &b);
                        // SAFETY: NEON is mandatory on all AArch64 implementations.
                        unsafe { $mul_add(&mut result, &a, &b) };

                        assert_close(&expected, &result, &format!("large multiply_add len={len}"));
                    }
                }

                // Mimics the convolution inner loop: one overwriting multiply followed by
                // repeated accumulates, so rounding differences have a chance to build up.
                #[test]
                fn multiply_add_multi_round_matches_scalar() {
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
                    // SAFETY: NEON is mandatory on all AArch64 implementations.
                    unsafe { $mul(&mut result, &segments[0].0, &segments[0].1) };

                    for (a, b) in &segments[1..] {
                        multiply_add_elements_scalar(&mut expected, a, b);
                        // SAFETY: NEON is mandatory on all AArch64 implementations.
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
        multiply_elements_neon_f64,
        multiply_add_elements_neon_f64,
        1e-9,
        1e-14
    );
    kernel_tests!(
        f32_kernels,
        f32,
        multiply_elements_neon_f32,
        multiply_add_elements_neon_f32,
        1e-5,
        1e-6
    );
}
