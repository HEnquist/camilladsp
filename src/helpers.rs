use crate::PrcFmt;
use num_complex::Complex;

// element-wise product, result = slice_a * slice_b
pub fn multiply_elements(
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
pub fn multiply_add_elements(
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

// Inplace recalculation of values positive values 0..1 to dB.
pub fn linear_to_db(values: &mut [f32]) {
    values.iter_mut().for_each(|val| {
        if *val == 0.0 {
            *val = -1000.0;
        } else {
            *val = 20.0 * val.log10();
        }
    });
}

// A simple PI controller for rate adjustments
pub struct PIRateController {
    target_level: f64,
    interval: f64,
    k_p: f64,
    k_i: f64,
    frames_per_interval: f64,
    accumulated: f64,
}

impl PIRateController {
    /// Create a new controller with default gains
    pub fn new_with_default_gains(fs: usize, interval: f64, target_level: usize) -> Self {
        let k_p = 0.2;
        let k_i = 0.004;
        Self::new(fs, interval, target_level, k_p, k_i)
    }

    pub fn new(fs: usize, interval: f64, target_level: usize, k_p: f64, k_i: f64) -> Self {
        let frames_per_interval = interval * fs as f64;
        Self {
            target_level: target_level as f64,
            interval,
            k_p,
            k_i,
            frames_per_interval,
            accumulated: 0.0,
        }
    }

    /// Calculate the control output for the next measured value
    pub fn next(&mut self, level: f64) -> f64 {
        let err = level - self.target_level;
        let rel_diff = err / self.frames_per_interval;
        self.accumulated += rel_diff * self.interval;
        let proportional = self.k_p * rel_diff;
        let integral = self.k_i * self.accumulated;
        let mut rate_diff = proportional + integral;
        rate_diff = rate_diff.clamp(-0.005, 0.005);
        1.0 - rate_diff
    }
}
