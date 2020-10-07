use num_complex::Complex;
use PrcFmt;

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

    while res.len() >= 8 {
        res[0] = val_a[0] * val_b[0];
        res[1] = val_a[1] * val_b[1];
        res[2] = val_a[2] * val_b[2];
        res[3] = val_a[3] * val_b[3];
        res[4] = val_a[4] * val_b[4];
        res[5] = val_a[5] * val_b[5];
        res[6] = val_a[6] * val_b[6];
        res[7] = val_a[7] * val_b[7];
        res = &mut res[8..];
        val_a = &val_a[8..];
        val_b = &val_b[8..];
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

    while res.len() >= 8 {
        res[0] += val_a[0] * val_b[0];
        res[1] += val_a[1] * val_b[1];
        res[2] += val_a[2] * val_b[2];
        res[3] += val_a[3] * val_b[3];
        res[4] += val_a[4] * val_b[4];
        res[5] += val_a[5] * val_b[5];
        res[6] += val_a[6] * val_b[6];
        res[7] += val_a[7] * val_b[7];
        res = &mut res[8..];
        val_a = &val_a[8..];
        val_b = &val_b[8..];
    }
    for (r, val) in res
        .iter_mut()
        .zip(val_a.iter().zip(val_b.iter()).map(|(a, b)| *a * *b))
    {
        *r += val;
    }
}
