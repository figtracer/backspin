//! Volume (VolumeShaperModulation::process 0x5393ec, shapers_a.md section 2).

use super::Wave;
use crate::smoother::Smoother;

/// `out = in * w * 2^((48 gain - 24) / 6)`: the wave is a linear amplitude.
pub fn process(buf: &mut [[f32; 2]], w: Wave, gain: &Smoother) {
    for (i, f) in buf.iter_mut().enumerate() {
        let g = ((gain.at(i) * 48.0 - 24.0) / 6.0).exp2();
        let k = w.at(i) * g;
        f[0] *= k;
        f[1] *= k;
    }
}
