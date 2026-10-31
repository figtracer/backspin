//! The envelope editor (`TMEnvEditor` / `EnvVisual`, 526 x 501 at (298, 5), gfx.md sections 8.7
//! and 8.8) as Gross Beat configures it: background and grid (`FUN_000a0a50`), the overlay with
//! the watermark, axis tags and diagonal (`FUN_00061010`), the curve with its glow and the
//! points (`FUN_000a4da0`). The editor is a child window with its own bitmap and Agg2D canvas.
//! The locator and the other group's ghost are not ported.

use crate::prims::{STROKE_OUT, aa_line, darken, disc, rne, swap_rb, vspan_add};
use crate::{Font, Text};
use backspin_envelope::Point;
use backspin_gfx::dib::lighten;
use backspin_gfx::{Dib, DrawPath, Rect};

/// The editor size at scale 1.
pub const ENV_W: i32 = 526;
pub const ENV_H: i32 = 501;
/// `+0x1604`: the background as a `TColor` (#344b5b).
const BG: u32 = 0x5b4b34;
/// The grid deltas of the inverted grid: minor, major, bar.
const GRID: [i32; 3] = [-8, -18, -36];
/// The axis tag fill (Agg colour of #25313b) and text colour.
const TAG_FILL: u32 = 0xff3b3125;
const TAG_TEXT: u32 = 0x8595a0;
const DIAG: u32 = 0x2a3f4e;
/// `+0x16a4`: the glow colour is the group colour times 11/256.
const GLOW: u32 = 11;

/// The view transform of the editor (`+0x1220` pixels per beat, `+0x1248` horizontal scroll,
/// `+0x1240` baseline, `+0x1238` pixels per unit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvView {
    pub px_per_beat: f64,
    pub x_scroll: i32,
    pub y_base: f64,
    pub px_per_unit: f64,
}

impl Default for EnvView {
    /// A 4-beat slot in the 526 x 501 editor.
    fn default() -> Self {
        Self {
            px_per_beat: 525.0 / 4.0,
            x_scroll: 0,
            y_base: 500.0,
            px_per_unit: 500.0,
        }
    }
}

impl EnvView {
    /// `FUN_000a8f00`.
    fn x(&self, t: f64) -> i32 {
        rne(t * self.px_per_beat) - self.x_scroll
    }

    /// `FUN_000a9290`.
    fn y(&self, v: f64) -> i32 {
        rne(self.y_base - v * self.px_per_unit)
    }

    /// `FUN_000a8ed0`.
    fn t(&self, x: f64) -> f64 {
        (self.x_scroll as f64 + x) * (1.0 / self.px_per_beat)
    }
}

/// Paints the editor into its own bitmap. `colour` is the edited group's colour `0xRRGGBB`.
pub(crate) fn paint_env(
    dib: &mut Dib,
    view: &EnvView,
    points: &[Point],
    colour: u32,
    watermark: &str,
    text: &mut dyn Text,
) {
    background(dib, view);
    overlay(dib, view, watermark, text);
    curve(dib, view, points, colour);
}

/// `FUN_000a0a50`: the fill, the minor grid lines, then the deferred major lines.
fn background(dib: &mut Dib, view: &EnvView) {
    let Rect { x0, y0, x1, y1 } = dib.clip();
    dib.fill_rect(Rect::new(x0, y0, x1, y1), swap_rb(BG) | 0xff00_0000, 0x100);
    let cols = GRID.map(|d| swap_rb(lighten(BG, d)) | 0xff00_0000);
    let (step, v_step) = (0.125, 1.0 / 64.0);
    let (mut major_v, mut major_h) = (Vec::new(), Vec::new());
    let mut i = rne(view.t((x0 - 2) as f64) / step);
    let mut x = view.x(i as f64 * step);
    while x < x1 + 1 {
        if i.rem_euclid(8) == 0 {
            major_v.push(x);
        } else {
            dib.fill_rect(Rect::new(x, y0, x + 1, y1), cols[0], 0x100);
        }
        i += 1;
        x = view.x(i as f64 * step);
    }
    // FUN_000a90b0: the value at the bottom row.
    let mut j = rne(((view.y_base - y1 as f64) / view.px_per_unit) / v_step);
    let mut y = view.y(j as f64 * v_step);
    while y >= y0 {
        if j.rem_euclid(8) == 0 {
            major_h.push(y);
        } else {
            dib.fill_rect(Rect::new(x0, y, x1, y + 1), cols[0], 0x100);
        }
        j += 1;
        y = view.y(j as f64 * v_step);
    }
    for y in major_h {
        dib.fill_rect(Rect::new(x0, y, x1, y + 1), cols[1], 0x100);
    }
    for x in major_v {
        dib.fill_rect(Rect::new(x, y0, x + 1, y1), cols[1], 0x100);
    }
}

/// `FUN_00061010` (not compact): the watermark, the seven axis tags and the diagonal.
fn overlay(dib: &mut Dib, view: &EnvView, watermark: &str, text: &mut dyn Text) {
    let Rect { x0, y0, x1, y1 } = dib.clip();
    text.draw(
        dib,
        Font::Watermark,
        watermark,
        Rect::new(x0 + 10, y0 + 5, x1 - 10, y1 - 5),
        0x11,
        0x000000,
        0x32,
    );
    if x0 < 22 {
        for i in 1..8 {
            let t = rne(view.y_base - (i as f64 * (1.0 / 8.0) + 0.0) * view.px_per_unit);
            let (l, r, b) = (0, 22, t + 12);
            // FUN_00061650: the polygon (l, t) (r, t) (0.6 r, b) (l, b).
            let a = dib.agg();
            a.fill_color(TAG_FILL);
            a.reset_path();
            a.move_to(l as f64, t as f64);
            a.line_to(r as f64, t as f64);
            a.line_to(r as f64 * 0.6, b as f64);
            a.line_to(l as f64, b as f64);
            a.line_to(l as f64, t as f64);
            a.draw_path(DrawPath::Fill);
            let inset = (r as f64 * 0.1) as i32;
            text.draw(
                dib,
                Font::ComboLabel,
                &format!("-{}", 8 - i),
                Rect::new(l + inset, t + inset, r, b),
                0x18,
                TAG_TEXT,
                0x100,
            );
        }
    }
    let yd = rne(view.y_base - (4.0 * (1.0 / 8.0) + 0.0) * view.px_per_unit);
    aa_line(dib, 0.0, 0.0, (x1 - x0) as f32, yd as f32, DIAG);
}

/// The segment shapes of `FUN_000a2680` (double arithmetic): `kind` is the shape byte, `sign`
/// the byte `+0x13` (2 = linear), `p` the coefficients. Shapes the model does not treat (4, 6,
/// 8, 12) and Hold are linear here.
fn shape(t: f64, kind: u8, sign: i8, p: [f32; 3]) -> f64 {
    if sign == 2 {
        return t;
    }
    let (a, b, c) = (p[0] as f64, p[1] as f64, p[2] as f64);
    let bits = p[0].to_bits();
    let curve = |u: f64| {
        if bits == 0 {
            u
        } else if bits & 0x8000_0000 == 0 {
            1.0 - (((1.0 - u) * c).exp() - 1.0) * b
        } else {
            ((u * c).exp() - 1.0) * b
        }
    };
    match kind {
        0 => curve(t),
        1 => {
            if t < 0.5 {
                curve(t * 2.0) * 0.5
            } else {
                1.0 - curve((1.0 - t) * 2.0) * 0.5
            }
        }
        3 => (rne(t * a - c) as f32 * p[1]) as f64,
        5 => (rne(t * a - c) & 1) as f64,
        7 => {
            let u = if sign > 0 { 1.0 - t } else { t };
            let u3 = u * u * u;
            t - (u3 * u3 - u) * a
        }
        9 => {
            let u = if sign < 1 {
                1.0 - (t * 1.5707963705062866).sin()
            } else {
                (t * 1.5707963705062866).cos()
            };
            t - (t - u) * a
        }
        10 => {
            let t2 = t * t;
            let tm = t - 1.0;
            t + (((t2 * 3.0 - t2 * 2.0 * t) + t2 * tm * a + t * tm * tm * b) - t) * c
        }
        11 => {
            if sign < 1 {
                b / (1.0 - a * t) - c
            } else {
                b / (1.0 - a * (1.0 - t)) - c
            }
        }
        _ => t,
    }
}

/// `FUN_001776b0(a, b, margin 0, state, flags 2)`: `h = [prev_lo, lo, prev_hi, hi, last_lo,
/// last_hi]`.
fn smooth(h: &mut [f32; 6], a: f32, b: f32) {
    let (lo, hi) = (a.min(b), a.max(b));
    h[0] = h[1];
    h[2] = h[3];
    h[1] = lo;
    h[3] = hi;
    h[4] = lo;
    h[5] = hi;
}

/// `FUN_000a4da0` without loop span, handles or hover: the curve (line width 1.5) with the glow
/// column under every pixel, then the points.
fn curve(dib: &mut Dib, view: &EnvView, points: &[Point], colour: u32) {
    let Rect { x0, x1, .. } = dib.clip();
    let n = points.len();
    let mut px = Vec::with_capacity(n);
    let mut py = Vec::with_capacity(n);
    let mut acc = 0.0;
    for p in points {
        acc += p.dx;
        px.push((rne(acc * view.px_per_beat) - view.x_scroll) as f64);
        py.push(view.y_base - p.value as f64 * view.px_per_unit);
    }
    // Crowded points fade.
    let alpha: Vec<f32> = (0..n)
        .map(|i| {
            let mut cnt = 0;
            for j in i + 1..n.min(i + 32) {
                if px[j] > px[i] + 3.0 {
                    break;
                }
                cnt += 1;
            }
            for j in (i.saturating_sub(32)..i).rev() {
                if px[j] < px[i] - 3.0 {
                    break;
                }
                cnt += 1;
            }
            ((1.0 - (cnt - 4) as f64 * 0.03571428571428571) as f32).clamp(0.0, 1.0)
        })
        .collect();
    let glow = darken(colour & 0xffffff, GLOW);
    let line = swap_rb(colour) | 0xff00_0000;
    let base_y = view.y(0.0) as f32;
    let mut hist = [0.0f32; 6];
    dib.agg().line_width(1.5);
    dib.agg().reset_path();
    for i in 1..n {
        let x_prev = rne(px[i - 1]) as f64;
        let y_prev = py[i - 1] as f32;
        let x_i = rne(px[i]) as f64;
        let y_i = py[i] as f32;
        let dx = x_i - x_prev;
        if x_i < -32.0 || (x1 + 32) as f64 <= x_prev || dx == 0.0 {
            continue;
        }
        let inv = 1.0 / dx;
        let p = &points[i];
        let start = if x_prev >= (x0 - 3) as f64 {
            x_prev
        } else {
            (x0 - 3) as f64
        };
        let end = if x_i <= (x1 + 3) as f64 {
            x_i
        } else {
            (x1 + 3) as f64
        };
        let xk = x_i - 5.960464477539063e-08;
        let tbase = xk * inv;
        let mut xx = start;
        let mut first = true;
        while xx <= end {
            xx += 1.0;
            let v = if xx < xk {
                let tt = xx * inv + (1.0 - tbase);
                shape(tt, p.shape, p.sign, p.coefficients()) as f32
            } else {
                1.0
            };
            let e4 = y_i - y_prev;
            smooth(&mut hist, y_prev + v * e4, y_prev + v * e4);
            let xi = rne(xx - 1.0);
            let top = hist[0].max(hist[1]);
            let bot = hist[2].min(hist[3]);
            if base_y < top {
                if base_y <= bot {
                    let ya = (bot - 0.5).max(base_y);
                    vspan_add(dib, base_y, base_y, ya, ya, xi, glow, 0x100);
                }
            } else {
                let yt = (top + 0.5).min(base_y);
                vspan_add(dib, yt, yt, base_y, base_y, xi, glow, 0x100);
            }
            let a = dib.agg();
            a.line_color(line);
            if first && i == 1 {
                a.move_to(xi as f64, ((hist[2] + hist[0]) * 0.5) as f64);
            } else {
                a.line_to((xi + 1) as f64, ((top + bot) * 0.5) as f64);
            }
            first = false;
        }
    }
    let a = dib.agg();
    a.draw_path(DrawPath::Stroke);
    a.line_width(1.0 + STROKE_OUT);
    // FUN_000a2050: the points in the group colour lightened by 0x20.
    let pc = lighten(colour & 0xffffff, 0x20);
    for (i, p) in points.iter().enumerate() {
        if !(-32.0 <= px[i] && px[i] < (x1 + 32) as f64) {
            continue;
        }
        let (x, y) = (rne(px[i]) as f32, py[i] as f32);
        let mut c = pc;
        if p.selected() {
            disc(dib, x, y, 1.0, 5.0, 1.0, 0xa8e44a);
            c = 0xa8e44a;
        }
        if alpha[i] > 0.0 {
            disc(
                dib,
                x,
                y,
                0.0,
                (2.65f64 + 0.5) as f32 + 0.5,
                alpha[i],
                darken(c, 0x20),
            );
            disc(dib, x, y, 0.0, (2.65f64 - 0.5) as f32 + 0.5, alpha[i], c);
        }
    }
}
