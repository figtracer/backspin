//! The transcendental vector routines of the original's IPP wrapper (dsp.md section 0).
//!
//! On macOS they call Accelerate vForce exactly as `dsp_ippv2.dylib` does, which is the only way
//! to be bit-identical. Elsewhere they fall back to libm, which differs by at most one ulp.

#[cfg(target_os = "macos")]
#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vvsin(y: *mut f64, x: *const f64, n: *const i32);
    fn vvcos(y: *mut f64, x: *const f64, n: *const i32);
    fn vvexpf(y: *mut f32, x: *const f32, n: *const i32);
}

/// `ippsSin_64f_A50`: `y[i] = sin(x[i])`.
pub fn sin(x: &[f64], y: &mut [f64]) {
    assert_eq!(x.len(), y.len());
    #[cfg(target_os = "macos")]
    if !x.is_empty() {
        let n = x.len() as i32;
        // SAFETY: both slices hold `n` elements.
        unsafe { vvsin(y.as_mut_ptr(), x.as_ptr(), &n) };
    }
    #[cfg(not(target_os = "macos"))]
    for (y, x) in y.iter_mut().zip(x) {
        *y = x.sin();
    }
}

/// `ippsCos_64f_A53`: `y[i] = cos(x[i])`.
pub fn cos(x: &[f64], y: &mut [f64]) {
    assert_eq!(x.len(), y.len());
    #[cfg(target_os = "macos")]
    if !x.is_empty() {
        let n = x.len() as i32;
        // SAFETY: both slices hold `n` elements.
        unsafe { vvcos(y.as_mut_ptr(), x.as_ptr(), &n) };
    }
    #[cfg(not(target_os = "macos"))]
    for (y, x) in y.iter_mut().zip(x) {
        *y = x.cos();
    }
}

/// `ippsExp_32f_A11`: `y[i] = exp(x[i])`.
pub fn expf(x: &[f32], y: &mut [f32]) {
    assert_eq!(x.len(), y.len());
    #[cfg(target_os = "macos")]
    if !x.is_empty() {
        let n = x.len() as i32;
        // SAFETY: both slices hold `n` elements.
        unsafe { vvexpf(y.as_mut_ptr(), x.as_ptr(), &n) };
    }
    #[cfg(not(target_os = "macos"))]
    for (y, x) in y.iter_mut().zip(x) {
        *y = (*x as f64).exp() as f32;
    }
}
