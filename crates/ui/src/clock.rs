//! `TScratchWheel.Paint` (`0x60690`, gfx.md section 8.10): the scratch clock, 72 x 72 at scale 1.
//! The live position ring drawn while playing is not ported.

use crate::prims::{arc_ring, disc, gradient_rect, ring_soft, rne};
use backspin_gfx::{Dib, Rect};

const PI_F: f32 = std::f32::consts::PI;
const TWO_PI: f32 = 6.283_185_5;
/// `DAT_003547f8`: float32 pi/180.
const DEG: f32 = 0.017453292;
const GRAD_TOP: u32 = 0x7c8790;
const FRAME: u32 = 0x4e5962;
const ARC: u32 = 0x848f96;
const DOTS: [u32; 2] = [0xffffff, 0x99a5ad];
/// The four arcs start at `k * pi` and span 1.75 pi.
const ARCS: [f32; 4] = [0.25, 1.25, 0.75, 1.75];

/// Paints the clock in control coordinates. `value` is the scratch parameter (0..65536, 65536
/// draws no hand ring) and `colour` the group colour `0xRRGGBB`.
pub(crate) fn paint_clock(dib: &mut Dib, w: i32, h: i32, value: i32, colour: u32) {
    let s = 1.0f32;
    let (c36, r25, r31, r32) = (s * 36.0, s * 25.0, s * 31.0, s * 32.0);
    let half = w as f32 * 0.5;
    dib.frame_rect(Rect::new(0, 0, w, h), 0x000000, 0x100, 1);
    dib.frame_rect(Rect::new(1, 1, w - 1, h - 1), FRAME, 0x100, 1);
    gradient_rect(dib, Rect::new(2, 2, w - 2, h - 2), GRAD_TOP, FRAME, 0x100);
    let size = (w - 4).min(h - 4);
    ring_soft(dib, c36, c36, size as f32 * 0.05, r31, 0.2, FRAME, true);
    let c = (c36, c36);
    for k in ARCS {
        let a0 = k * PI_F;
        let a1 = a0 + 1.75 * PI_F;
        arc_ring(dib, c, 0.0, 1.0, c, r31, 1.0, ARC, a0, a1, c, 0.5);
    }
    let rdot = rne((s * 2.0 + -0.49999997) as f64) as f32;
    for i in 0..16 {
        let a = ((i as f32 * 22.5) * DEG) as f64;
        let x = (half as f64 + a.cos() * r32 as f64) as f32;
        let y = (half as f64 + a.sin() * r32 as f64) as f32;
        disc(dib, x, y, 0.0, rdot, 1.0, DOTS[i & 1]);
    }
    // FUN_00060e40(value * 2^-16): the hand at angle pi - v * 2pi.
    let v = value as f32 * 1.5258789e-05;
    let a = (PI_F - v * TWO_PI) as f64;
    let hx = (c36 as f64 + a.sin() * r25 as f64) as f32;
    let hy = (c36 as f64 + a.cos() * r25 as f64) as f32;
    if value != 0x10000 {
        arc_ring(
            dib,
            c,
            r25 - 0.25,
            1.0,
            c,
            r25 + 1.25,
            1.0,
            colour,
            0.0,
            0.0,
            c,
            1.0,
        );
    }
    disc(dib, hx, hy, 0.0, s * 2.4 + 0.5, 1.0, colour);
}
