//! `TCustomQuickBtn.Paint` (`0x18ad80` -> `FUN_001895e0`) for the buttons that reach the vector
//! frame renderer `FUN_001f9500` (gfx.md sections 7.5 and 7.6): `HoldBtn` (`FS_Pad`,
//! `BC_Pad_Soft` corners, `bsxSingle`, FX blend settled) up and down. The
//! `FS_SoberOutlineTransparent` buttons (`OptionsBtn`, `SlotZoom`) draw no frame when up.

use crate::prims::{STROKE_OUT, lerp_colour, rne, swap_rb};
use crate::{Font, Text};
use backspin_gfx::dib::lighten;
use backspin_gfx::{Agg2D, Dib, DrawPath, gradient_lut_ex};

const LW: f32 = 1.0;
/// `TDispMetrics +0x1c`, the integer line width.
const INSET: f32 = 1.0;
const SQRT2: f32 = std::f32::consts::SQRT_2;
const EPS: f32 = 1.5258789e-05;
/// `DAT_00354318` for `BC_Pad_Soft` (12) and the corner softness `FUN_001f7730` writes.
const CORNER: [f32; 6] = [1.0, 1.0, 0.0, 0.0, 1.0, 1.0];
const SOFT: f32 = 0.92;
/// `HoldBtn`: `Color` and the font colour (`TColor`s).
const HOLD_COLOR: u32 = 0x2be383;
const HOLD_FONT: u32 = 0xada89f;

/// `FS_Pad` luminance factor of a `TColor`.
fn lum(c: u32) -> f32 {
    let (r, g, b) = (
        (c & 0xff) as f64,
        (c >> 8 & 0xff) as f64,
        (c >> 16 & 0xff) as f64,
    );
    ((r * 0.00116796875 + g * 0.00229296875 + b * 0.0004453125) * 3.85) as f32
}

/// `FUN_0017a040` / `FUN_0017a110`: segments that skip zero-length moves.
struct Sink<'a> {
    agg: &'a mut Agg2D,
    cur: (f32, f32),
}

impl Sink<'_> {
    fn line(&mut self, x: f32, y: f32) {
        if (x, y) != self.cur {
            self.agg.line_to(x as f64, y as f64);
            self.cur = (x, y);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cubic(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        if (x, y) != self.cur {
            self.agg.cubic_curve_to(
                c1x as f64, c1y as f64, c2x as f64, c2y as f64, x as f64, y as f64,
            );
            self.cur = (x, y);
        }
    }
}

/// `FUN_001f7730` + `FUN_001795d0` with four `BC_Pad_Soft` corners: a rounded rect whose corners
/// are two cubics each.
fn corner_path(agg: &mut Agg2D, x0: f32, y0: f32, x1: f32, y1: f32) {
    let hw = (x1 - x0) * 0.5;
    let hh = (y1 - y0) * 0.5;
    let r = hw.min(hh) - 1.0;
    let t = CORNER;
    let s = r.abs() * SOFT;
    agg.reset_path();
    agg.move_to(x0 as f64, (y0 + r) as f64);
    let mut k = Sink {
        agg,
        cur: (x0, y0 + r),
    };
    // Top-left.
    let (ax, ay) = (x0 + r, y0 + r);
    let f7 = ax - s * t[0];
    let f8 = ay - s * t[1];
    let f6 = x0 - s * t[2];
    let f9 = y0 - s * t[3];
    k.cubic(f6, ay - s * t[5], f6, f8, (f6 + f7) * 0.5, (f9 + f8) * 0.5);
    k.cubic(f7, f9, ax - s * t[4], f9, ax, y0);
    // Top edge, top-right.
    let (bx, by) = (x1 - r, y0 + r);
    k.line(bx, y0);
    let f7 = bx + s * t[0];
    let f8 = by - s * t[1];
    let f6 = x1 + s * t[2];
    let f9 = y0 - s * t[3];
    k.cubic(bx + s * t[4], f9, f7, f9, (f6 + f7) * 0.5, (f9 + f8) * 0.5);
    k.cubic(f6, f8, f6, by - s * t[5], x1, by);
    // Right edge, bottom-right.
    let (cx, cy) = (x1 - r, y1 - r);
    k.line(x1, cy);
    let f7 = cx + s * t[0];
    let f8 = cy + s * t[1];
    let f6 = x1 + s * t[2];
    let f9 = y1 + s * t[3];
    k.cubic(f6, cy + s * t[5], f6, f8, (f6 + f7) * 0.5, (f9 + f8) * 0.5);
    k.cubic(f7, f9, cx + s * t[4], f9, cx, y1);
    // Bottom edge, bottom-left (FUN_00182510: when the left corners meet, it starts at the
    // top-left one).
    let dx = x0 + r;
    let mut dy = y1 - r;
    if dy - (y0 + r) == 0.0 {
        dy = y0 + r;
    }
    k.line(dx, y1);
    let f7 = dx - s * t[0];
    let f9 = dy + s * t[1];
    let f10 = x0 - s * t[2];
    let f8 = y1 + s * t[3];
    k.cubic(dx - s * t[4], f8, f7, f8, (f10 + f7) * 0.5, (f8 + f9) * 0.5);
    k.cubic(f10, f9, f10, dy + s * t[5], x0, dy);
    k.agg.close_polygon();
}

/// `FUN_001f9500` for `FS_Pad` with border style `0x21` (`bsxOpaque` clear): `base` is the shape
/// colour (`TColor`), `state` the FX amount 0..1. Returns the caption rect.
fn pad_frame(agg: &mut Agg2D, w: i32, h: i32, base: u32, state: f32) -> [f64; 4] {
    let lw108 = LW * 1.15;
    let base_c = base | 0xff00_0000;
    let l = lum(base);
    let outline = lighten(base_c, -rne((l * (state * 4.0 + 20.0)) as f64));
    let d = lw108 + INSET;
    corner_path(agg, d, d, w as f32 - d, h as f32 - d);
    let x0 = INSET + EPS;
    let y0 = INSET + EPS;
    let x1 = (w as f32 - INSET) - EPS;
    let y1 = (h as f32 - INSET) - EPS;
    agg.line_color(outline);
    agg.line_width_ex((LW + lw108) as f64, LW as f64 + STROKE_OUT);
    agg.draw_path(DrawPath::Stroke);
    agg.line_width_ex(lw108 as f64, lw108 as f64 + STROKE_OUT);
    let hw = (x1 - x0) * 0.5;
    let hh = (y1 - y0) * 0.5;
    let r = hh - lw108 * 0.5;
    let aspect = (hw - lw108 * 0.5) / r;
    let rad = r * SQRT2;
    let (cx, cy) = ((x0 + hw) as f64, (y0 + hh) as f64);
    agg.radial_gradient_ex(cx, cy, rad as f64, 0.0, aspect as f64, 0);
    agg.radial_gradient_ex(cx, cy, rad as f64, 0.0, aspect as f64, 1);
    gradient_lut_ex(
        agg,
        (state * 24.0 + 24.0) * l,
        l * -6.0,
        0.33333334,
        base_c,
        0,
        1,
    );
    let k = 0x8c as f32 + 32.0;
    gradient_lut_ex(
        agg,
        ((state * k) + 32.0) * l,
        ((state * 11.0) - 14.0) * l,
        0.0,
        base_c,
        1,
        0,
    );
    agg.draw_path(DrawPath::StrokeThenFill);
    agg.line_join(2);
    [x0 as f64, y0 as f64, x1 as f64, y1 as f64]
}

/// `HoldBtn` (63 x 30): the pad frame and the caption `HOLD` (flags `0x10005a`, alpha `0x110`).
pub(crate) fn hold_btn(dib: &mut Dib, w: i32, h: i32, down: bool, text: &mut dyn Text) {
    let state = if down { 1.0f32 } else { 0.0 };
    let t = rne((state * 256.0) as f64);
    let base = lerp_colour(HOLD_COLOR, lighten(HOLD_FONT, -0x90), t);
    let rect = pad_frame(dib.agg(), w, h, base, state);
    let colour = lerp_colour(lighten(HOLD_COLOR, -0x44), HOLD_FONT, t);
    text.draw_f(
        dib,
        Font::Hold,
        "HOLD",
        rect,
        0x10005a,
        swap_rb(colour) & 0xffffff,
        0x110,
    );
}

/// The framework's down arrow (`FUN_00200750`) of `OptionsBtn` and the combos: a filled
/// triangle with a cubic tip: `pts` are the start, the first control and end, the second
/// control and end. Drawn with `lineJoin(1)`, then `lineJoin(2)` stays.
pub(crate) fn arrow(agg: &mut Agg2D, pts: [(f64, f64); 5], colour: u32) {
    agg.reset_path();
    agg.move_to(pts[0].0, pts[0].1);
    agg.cubic_curve_to(pts[1].0, pts[1].1, pts[1].0, pts[1].1, pts[2].0, pts[2].1);
    agg.cubic_curve_to(pts[3].0, pts[3].1, pts[3].0, pts[3].1, pts[4].0, pts[4].1);
    agg.line_to(pts[0].0, pts[0].1);
    agg.close_polygon();
    agg.line_join(1);
    agg.fill_color(colour);
    agg.draw_path(DrawPath::Fill);
    agg.line_join(2);
}
