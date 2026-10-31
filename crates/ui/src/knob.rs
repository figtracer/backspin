//! `TVectorWheel.Paint` (`0x7c650`, gfx.md section 7.7) for the Gross Beat knobs: rail
//! `VWR_Gauge`, cap `VWC_SoftBump`, no gear, dot pointer; not pressed, not hovered.

use crate::prims::{STROKE_OUT, disc, rne, swap_rb};
use backspin_gfx::dib::lighten;
use backspin_gfx::{Dib, DrawPath, gradient_lut_ex};

/// `RailColor` = `CapColor` #3f484d as a `TColor`.
const RAIL: u32 = 0x4d483f;
const CAP: u32 = 0x4d483f;
/// `FillColor`: time #86bf69, volume #ffb870 (`TColor`s).
const FILL_TIME: u32 = 0x69bf86;
const FILL_VOL: u32 = 0x70b8ff;
/// `StartAngle` / `EndAngle` (float32): 1.75 pi, 2.25 pi for the Mix knobs; 1.25 pi, 1.75 pi
/// for ATT, REL and TNS.
const MIX: (f32, f32) = (5.497787, 7.0685835);
const SMALL: (f32, f32) = (3.9269907, 5.497787);
const TWO_PI: f32 = 6.283_185_5;
const PI_F: f32 = std::f32::consts::PI;
/// `TDispMetrics +0x24` at scale 1.
const LW: f32 = 1.0;

/// One knob of the form: geometry, angles, fill and its parameter (index and raw range).
pub(crate) struct Knob {
    pub x: i32,
    pub y: i32,
    pub size: i32,
    angles: (f32, f32),
    centered: bool,
    fill: u32,
    pub param: usize,
    lo: i32,
    hi: i32,
}

impl Knob {
    /// The knob value in 0..1.
    pub fn value(&self, params: &[i32; 9]) -> f64 {
        (params[self.param] - self.lo) as f64 / (self.hi - self.lo) as f64
    }
}

/// The knobs at form height 535.
pub(crate) const TIME_MIX: Knob = Knob {
    x: 7,
    y: 5,
    size: 48,
    angles: MIX,
    centered: false,
    fill: FILL_TIME,
    param: 2,
    lo: 0,
    hi: 256,
};
pub(crate) const VOL_MIX: Knob = Knob {
    x: 7,
    y: 480,
    size: 48,
    angles: MIX,
    centered: false,
    fill: FILL_VOL,
    param: 3,
    lo: 0,
    hi: 256,
};
pub(crate) const VOL_ATT: Knob = Knob {
    x: 56,
    y: 422,
    size: 42,
    angles: SMALL,
    centered: false,
    fill: FILL_VOL,
    param: 4,
    lo: 0,
    hi: 10000,
};
pub(crate) const VOL_REL: Knob = Knob {
    x: 106,
    y: 422,
    size: 42,
    angles: SMALL,
    centered: false,
    fill: FILL_VOL,
    param: 5,
    lo: 0,
    hi: 10000,
};
pub(crate) const VOL_TNS: Knob = Knob {
    x: 159,
    y: 418,
    size: 48,
    angles: SMALL,
    centered: true,
    fill: FILL_VOL,
    param: 6,
    lo: -256,
    hi: 256,
};

/// `thunk_FUN_00006b78(B*0x1d2f + G*0x9646 + R*0x4c8b, factor, 0x1000000)`: the luma weighted
/// delta (truncated toward zero).
fn luma_dark(colour: u32, factor: i64) -> i32 {
    let (r, g, b) = (
        (colour & 0xff) as i64,
        (colour >> 8 & 0xff) as i64,
        (colour >> 16 & 0xff) as i64,
    );
    ((b * 0x1d2f + g * 0x9646 + r * 0x4c8b) * factor / 0x1000000) as i32
}

/// `FUN_0008aec0(angle, radius, centre)`: `(cx + cos(a) r, cy - sin(a) r)`, libm on the widened
/// angle, products in float32.
pub(crate) fn polar(angle: f32, radius: f32, cx: f32, cy: f32) -> (f32, f32) {
    let a = angle as f64;
    let (s, c) = (a.sin() as f32, a.cos() as f32);
    (cx + c * radius, cy - s * radius)
}

/// Paints `k` at value `v` (0..1) in control coordinates.
pub(crate) fn paint_knob(dib: &mut Dib, k: &Knob, v: f64) {
    let (w, h) = (k.size, k.size);
    let r = w.min(h) as f32 * 0.5;
    let cx = w as f32 * 0.5;
    let cy = r;
    let (dcx, dcy) = (cx - 0.5, cy - 0.5);
    let rail_c = RAIL | 0xff00_0000;
    let dark = lighten(rail_c, luma_dark(rail_c, -0x50));
    let fill_c = k.fill | 0xff00_0000;
    let (start, end) = k.angles;
    // FUN_0007c310: the value angles.
    let v = v as f32;
    let vspan = (TWO_PI - end) + start;
    let a_to = start - v * vspan;
    let a_from = if k.centered {
        start - vspan * 0.5
    } else {
        start
    };
    let rr = r - LW;
    let gear = rne((rr * 0.6) as f64);
    let span = end - start;
    let a = dib.agg();
    // The rail.
    a.reset_path();
    let (sx, sy) = polar(start, rr, cx, cy);
    let (ex, ey) = polar(end, rr, cx, cy);
    a.move_to(sx as f64, sy as f64);
    a.arc_to(
        rr as f64,
        rr as f64,
        0.0,
        span <= PI_F,
        true,
        ex as f64,
        ey as f64,
    );
    a.line_to(cx as f64, cy as f64);
    a.line_to(sx as f64, sy as f64);
    a.close_polygon();
    a.line_width_ex(LW as f64, LW as f64 + STROKE_OUT);
    a.line_color(dark);
    a.draw_path(DrawPath::Stroke);
    a.radial_gradient_ex(cx as f64, (cy + rr) as f64, (rr * 2.0) as f64, 0.0, 1.5, 1);
    gradient_lut_ex(a, 0.0, -12.0, 0.0, rail_c, 1, 1);
    a.draw_path(DrawPath::Fill);
    // The value arc.
    a.fill_color(fill_c);
    a.reset_path();
    if a_from < a_to + TWO_PI {
        let (fx, fy) = polar(a_from, rr, cx, cy);
        let (tx, ty) = polar(a_to, rr, cx, cy);
        a.move_to(fx as f64, fy as f64);
        a.arc_to(
            rr as f64,
            rr as f64,
            0.0,
            a_from - a_to > PI_F,
            a_to < a_from,
            tx as f64,
            ty as f64,
        );
        a.line_to(cx as f64, cy as f64);
        a.line_to(fx as f64, fy as f64);
        a.close_polygon();
    } else {
        a.add_ellipse(cx as f64, cy as f64, rr as f64, rr as f64, false);
    }
    a.draw_path(DrawPath::Fill);
    // No gear: the cap size and the dark ring (dent 0 when not hovered).
    let gear = (gear as f32).min(rr - LW * 2.0);
    let r160 = rne(gear as f64) as f32;
    let d0 = ((r160 + 0.125) - 0.16666667) - LW;
    let kk = (LW * 0.9).max(1.4);
    let ring_out = (r160 + 0.125) + 0.33333334;
    let ring_in = d0 - kk * 0.5;
    disc(dib, dcx, dcy, ring_in, ring_out, 1.0, swap_rb(dark));
    // The cap shadow and the soft bump cap.
    let cap_c = CAP | 0xff00_0000;
    let sr = r160 - LW;
    let a = dib.agg();
    a.fill_color(0x4800_0000);
    a.reset_path();
    a.move_to((cx - sr) as f64, cy as f64);
    a.arc_to(
        sr as f64,
        (sr * (0.2f32 + 1.0)) as f64,
        0.0,
        false,
        false,
        (cx + sr) as f64,
        cy as f64,
    );
    a.hor_line_to((cx - sr) as f64);
    a.close_polygon();
    a.draw_path(DrawPath::Fill);
    let d4 = ring_in - kk * 0.5;
    a.linear_gradient(cx as f64, (cy - r160) as f64, cx as f64, cy as f64, 0);
    gradient_lut_ex(a, 60.0, -8.0, 0.0, cap_c, 0, 0);
    a.radial_gradient(cx as f64, (cy - r160) as f64, (r160 * 2.0) as f64, 1);
    gradient_lut_ex(a, 24.0, -28.0, 0.05, cap_c, 0, 1);
    // FUN_00182c80(cx, cy, d4, k).
    let (fcx, fcy) = (cx as f64, cy as f64);
    let outer = d4 as f64 + kk as f64;
    let inner = d4 as f64 - STROKE_OUT;
    a.reset_path();
    a.add_ellipse(fcx, fcy, outer, outer, true);
    a.add_ellipse(fcx, fcy, inner, inner, false);
    a.draw_path(DrawPath::FillWithLineColor);
    a.reset_path();
    a.add_ellipse(fcx, fcy, d4 as f64, d4 as f64, true);
    a.draw_path(DrawPath::Fill);
    // The dot pointer in the fill colour.
    let c8 = (ring_in - 0.0).max(0.0);
    let mut dot = LW * 1.0;
    if dot <= c8 * 0.0625 {
        dot = (c8 * 0.0625).min(LW * 3.0);
    }
    let dr = ((c8 * 0.975) - LW * 1.5) - dot;
    let (px, py) = polar(a_to, dr, cx, cy);
    disc(
        dib,
        px - 0.5,
        py - 0.5,
        0.0,
        dot + 0.5,
        1.0,
        swap_rb(fill_c),
    );
}
