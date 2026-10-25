//! Width (WidthShaperModulation::process 0x52e7b4, shapers_a.md section 3).

use super::Wave;

/// The side signal scaled by `2w` (mid unchanged), then the gain `2^((24 gain - 12) / 6)`.
/// `gain` is the plain state value (not smoothed).
pub fn process(buf: &mut [[f32; 2]], w: Wave, gain: f32) {
    if !w.is_constant(0.5) {
        for (i, f) in buf.iter_mut().enumerate() {
            let k = w.at(i).min(1.0) - 0.5;
            let d = (f[0] - f[1]) * k;
            f[0] += d;
            f[1] -= d;
        }
    }
    let g = ((gain * 2.0 - 1.0) * 12.0 / 6.0).exp2();
    if g == 0.0 {
        buf.fill([0.0; 2]);
    } else if g != 1.0 {
        for f in buf.iter_mut() {
            f[0] *= g;
            f[1] *= g;
        }
    }
}
