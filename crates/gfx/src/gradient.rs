//! The gradient LUT builder `FUN_00182ab0` (gfx.md section 4.3).

use backspin_agg2d::Agg2D;

/// `FUN_00182ab0(top, bottom, curve = 0, dib, colour, flags = 0, slot)`: fills the 256-entry
/// Agg2D colour buffer of `slot` with a ramp from `colour + top` to `colour + bottom` in 1/128
/// units. `colour` is `0xAABBGGRR`; lanes are its bytes in memory order (R, G, B, A).
pub fn gradient_lut(agg: &mut Agg2D, top: f32, bottom: f32, colour: u32, slot: u32) {
    let lut = agg.gradient_color_buffer(slot);
    let mut base = [0, 8, 16, 24].map(|s| ((colour >> s) & 0xff) as f32 * 128.0);
    for b in &mut base[..3] {
        *b += top * 128.0;
    }
    let d = (bottom - top) * 128.0;
    let delta = [d, d, d, 0.0];
    let slope = 1.0f32 / 255.0;
    for (i, out) in lut.iter_mut().enumerate() {
        // ippsVectorSlope_32f(offset 0, slope 1/255), then FUN_00006160: fused base + delta * t,
        // round to nearest even, saturate to i16.
        let t = 0.0f32 + slope * i as f32;
        for k in 0..4 {
            let v = base[k] + delta[k] * t;
            out[k] = v.round_ties_even().clamp(-32768.0, 32767.0) as i16;
        }
    }
}
