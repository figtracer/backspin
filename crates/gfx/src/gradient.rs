//! The gradient LUT builder `FUN_00182ab0` (gfx.md section 4.3) with the ramp of `FUN_001c3700`.

use backspin_agg2d::Agg2D;

/// `FUN_00182ab0(top, bottom, curve = 0, dib, colour, flags = 0, slot)`.
pub fn gradient_lut(agg: &mut Agg2D, top: f32, bottom: f32, colour: u32, slot: u32) {
    gradient_lut_ex(agg, top, bottom, 0.0, colour, 0, slot);
}

/// `FUN_00182ab0(top, bottom, curve, dib, colour, flags, slot)`: fills the 256-entry Agg2D colour
/// buffer of `slot` with a ramp from `colour + top` to `colour + bottom` in 1/128 units. `colour`
/// is `0xAABBGGRR`; lanes are its bytes in memory order (R, G, B, A). `curve` shapes the ramp
/// (`FUN_000064c4`); flags bit 0 maps it through `x^2 (x - 2)^2` (`FUN_000045bc`). The
/// exponential ramp (flags bit 1) is not ported.
pub fn gradient_lut_ex(
    agg: &mut Agg2D,
    top: f32,
    bottom: f32,
    curve: f32,
    colour: u32,
    flags: u32,
    slot: u32,
) {
    let mut base = [0, 8, 16, 24].map(|s| ((colour >> s) & 0xff) as f32 * 128.0);
    let t128 = top * 128.0;
    for b in &mut base[..3] {
        *b += t128;
    }
    let d = (bottom - top) * 128.0;
    let delta = [d, d, d, 0.0];
    let ramp = ramp(curve, flags);
    for (out, r) in agg.gradient_color_buffer(slot).iter_mut().zip(ramp) {
        // FUN_00006160: fused base + delta * r, round to nearest even, saturate to i16.
        for k in 0..4 {
            let v = (base[k] as f64 + delta[k] as f64 * r as f64) as f32;
            out[k] = v.round_ties_even().clamp(-32768.0, 32767.0) as i16;
        }
    }
}

/// `FUN_001c3700` for 256 entries: the 0..1 ramp (`ippsVectorSlope_32f` with slope 1/255).
fn ramp(curve: f32, flags: u32) -> [f32; 256] {
    const STEP: f32 = 1.0 / 255.0;
    let shape = |r: f32, c: f32| r + (r * r * r - r) * c;
    let mut out = [0.0f32; 256];
    for (i, o) in out.iter_mut().enumerate() {
        let up = (STEP as f64 * i as f64) as f32;
        *o = if curve == 0.0 {
            up
        } else if curve < 0.0 {
            let down = (1.0 - STEP as f64 * i as f64) as f32;
            1.0 - shape(down, -curve)
        } else {
            shape(up, curve)
        };
        if flags & 1 != 0 {
            let r = *o;
            *o = (r * r) * ((r + -2.0) * (r + -2.0));
        }
    }
    out
}
