//! The plugin's own raster primitives that the controls use beyond the `TDIB` fills (gfx.md
//! section 3.2, `prims.py`): the gradient rect, the disc, ring and ring-sector rasterizers, the
//! anti-aliased line, the glow column, the colour helpers, and the vector maths they take from
//! `dsp_ippv2` and Accelerate (`vvsqrtf` is not the IEEE square root).
//!
//! Arithmetic is float32 where the compiled code is; coordinates are local to the `Dib` origin
//! and every primitive clips to the `Dib` clip.

use backspin_gfx::{Dib, Rect};

const HALF: f32 = 0.5;
/// The outer offset of the plugin's custom strokes (`lineWidthEx(w, w + 0.70710678118)`).
#[allow(clippy::approx_constant)]
pub(crate) const STROKE_OUT: f64 = 0.70710678118;
const TWO_PI: f32 = 6.283_185_5;
const PI_F: f32 = std::f32::consts::PI;

/// FPC `Round`: half to even.
pub(crate) fn rne(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// `fcvtzs`: truncation toward zero.
pub(crate) fn trunc32(x: f64) -> i32 {
    x as i32
}

/// ARMv8 `FRSQRTE` for a float32 (`RecipSqrtEstimate`: an 8-bit estimate from the exponent parity
/// and the top fraction bits).
fn frsqrte(x: f32) -> f32 {
    let b = x.to_bits();
    let e = (b >> 23) & 0xff;
    let m = (b & 0x7fffff) >> 15;
    let mut a = if e & 1 == 1 { 128 + (m >> 1) } else { 256 + m };
    if a < 256 {
        a = a * 2 + 1;
    } else {
        a = ((a >> 1) << 1) + 1;
        a *= 2;
    }
    let mut r = 512u64;
    while (a as u64) * (r + 1) * (r + 1) < 1 << 28 {
        r += 1;
    }
    let est = r.div_ceil(2) as u32 & 0xff;
    f32::from_bits((((380 - e as i32) >> 1) as u32) << 23 | est << 15)
}

/// ARMv8 `FRSQRTS` (fused): `(3 - a*b) / 2` with one rounding.
fn frsqrts(a: f32, b: f32) -> f32 {
    (3.0 - a as f64 * b as f64) as f32 * 0.5
}

/// Accelerate `vvsqrtf` (`ippsSqrt_32f_A11/A21`): `FRSQRTE`, two `FRSQRTS` steps, times `x`.
pub(crate) fn vvsqrtf(x: f32) -> f32 {
    if x == 0.0 || x.is_infinite() {
        return x;
    }
    let mut y = frsqrte(x);
    y *= frsqrts(x * y, y);
    y *= frsqrts(x * y, y);
    x * y
}

/// `ippsVectorSlope_32f`: `fma(i, slope, start)`.
fn slope(start: f32, step: f32, i: usize) -> f32 {
    (start as f64 + i as f64 * step as f64) as f32
}

/// `FUN_00170ea0` (mode 0, 32 bpp): per lane `d += ((c - d) * a) >> 8`, with `a` taken as an
/// i16 (`((c - d) << 4) * (a << 4)`, wrapping like the hardware). Clipped to the bitmap only.
pub(crate) fn blend_pixel(dib: &mut Dib, x: i32, y: i32, colour: u32, a: i32) {
    let (ox, oy) = dib.origin();
    let (x, y, w, h) = (x + ox, y + oy, dib.width(), dib.height());
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let m = (((a & 0xffff) << 4) & 0xffff) as i16 as i32;
    let p = &mut dib.pixels_mut()[(y * w + x) as usize];
    let d = *p;
    let mut out = 0;
    for s in [0, 8, 16, 24] {
        let dc = ((d >> s) & 0xff) as i32;
        let cc = ((colour >> s) & 0xff) as i32;
        let v = dc + ((((cc - dc) << 4) * m) >> 16);
        out |= (v.clamp(0, 255) as u32) << s;
    }
    *p = out;
}

/// `FUN_00174b00`: stores the u32 (alpha byte included).
fn set_pixel(dib: &mut Dib, x: i32, y: i32, v: u32) {
    let (ox, oy) = dib.origin();
    let (x, y, w, h) = (x + ox, y + oy, dib.width(), dib.height());
    if x >= 0 && y >= 0 && x < w && y < h {
        dib.pixels_mut()[(y * w + x) as usize] = v;
    }
}

/// The coverage dispatch of the rasterizers: `a >= 0x100` stores, `0 < a < 0x100` blends.
fn plot(dib: &mut Dib, x: i32, y: i32, colour: u32, a: i32) {
    if a >= 0x100 {
        set_pixel(dib, x, y, colour);
    } else if a > 0 {
        blend_pixel(dib, x, y, colour, a);
    }
}

/// `FUN_00174e50` (blend mode 1): `d += (a * c) div 255` per colour lane, saturated.
fn add_pixel(dib: &mut Dib, x: i32, y: i32, colour: u32, a: i32) {
    let (ox, oy) = dib.origin();
    let (x, y, w, h) = (x + ox, y + oy, dib.width(), dib.height());
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let p = &mut dib.pixels_mut()[(y * w + x) as usize];
    let d = *p;
    let mut out = d & 0xff00_0000;
    for s in [0, 8, 16] {
        let v = ((d >> s) & 0xff) as i32 + (a * ((colour >> s) & 0xff) as i32) / 0xff;
        out |= (v.min(0xff) as u32) << s;
    }
    *p = out;
}

/// Delphi `0x00BBGGRR` <-> `0x00RRGGBB`, keeping the top byte.
pub(crate) fn swap_rb(c: u32) -> u32 {
    ((c & 0xff00ff) >> 16)
        .wrapping_add((c & 0xff00ff) << 16)
        .wrapping_add(c & 0xff00ff00)
}

/// `FUN_00006c04(c0, c1, t)`: per lane (alpha included) `c1 + floor((c0 - c1) * t / 256)`.
pub(crate) fn lerp_colour(c0: u32, c1: u32, t: i32) -> u32 {
    let mut out = 0;
    for s in [0, 8, 16, 24] {
        let (a, b) = (((c0 >> s) & 0xff) as i32, ((c1 >> s) & 0xff) as i32);
        let v = b + ((((a - b) << 4) * (t << 4)) >> 16);
        out |= (v.clamp(0, 255) as u32) << s;
    }
    out
}

/// `FUN_00006ba4(c, a)`: `(c * a) >> 8` per lane, saturated.
pub(crate) fn darken(c: u32, a: u32) -> u32 {
    let mut out = 0;
    for s in [0, 8, 16, 24] {
        out |= ((((c >> s) & 0xff) * a) >> 8).min(255) << s;
    }
    out
}

/// `FUN_00186040` on a local rect: the rect clipped, or `None`.
fn clip(dib: &Dib, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Rect> {
    let r = Rect::new(x0, y0, x1, y1).intersect(dib.clip());
    (!r.is_empty()).then_some(r)
}

/// The rasterizers' bounding box: `(Round(cx - r') + 1, Round(cy - r') + 1, Round(cx + r'),
/// Round(cy + r'))` with `r' = r + 0.5`.
fn bbox(dib: &Dib, cx: f32, cy: f32, r: f32) -> Option<Rect> {
    let fv = r + HALF;
    clip(
        dib,
        rne((cx - fv) as f64) + 1,
        rne((cy - fv) as f64) + 1,
        rne((cx + fv) as f64),
        rne((cy + fv) as f64),
    )
}

const DITHER: [f32; 4] = [0.25, 0.0, 0.75, 0.5];

/// `FUN_00180250(dib, rect, c0, c1, steps, flags = 0)`: a vertical gradient from `c0` (top row)
/// to `c1` (bottom row) with the 4 x 4 ordered dither; colours in pixel lane order.
pub(crate) fn gradient_rect(dib: &mut Dib, r: Rect, c0: u32, c1: u32, steps: i32) {
    let (dox, doy) = (r.x0, r.y0);
    let n = ((r.y1 - r.y0) - 1).max(1);
    let Some(c) = clip(dib, r.x0, r.y0, r.x1, r.y1) else {
        return;
    };
    let lanes0 = [0, 8, 16, 24].map(|s| ((c0 >> s) & 0xff) as f32);
    let lanes1 = [0, 8, 16, 24].map(|s| ((c1 >> s) & 0xff) as f32);
    let delta: [f32; 4] = std::array::from_fn(|k| lanes1[k] - lanes0[k]);
    let scale = steps as f32 * 0.00390625;
    let inv_n = (1.0 / n as f64) as f32;
    for y in c.y0..c.y1 {
        let t = (y - r.y0) as f32 * inv_n;
        let lane: [f32; 4] = std::array::from_fn(|k| delta[k] * t + lanes0[k]);
        let mut pat = [0u32; 4];
        for (px, p) in pat.iter_mut().enumerate() {
            for (k, l) in lane.iter().enumerate() {
                let u1 = ((k as i32 + (y - doy)) & 3) as usize;
                let d = DITHER[(px as i32 + (u1 as i32 * 2 + (c.x0 - dox))) as usize & 3];
                let off = (u1 as f64 * 0.0625 + (-0.49999997f32) as f64) as f32;
                let val = (l * scale + off) + d;
                *p |= (rne(val as f64).clamp(0, 255) as u32) << (8 * k);
            }
        }
        for x in c.x0..c.x1 {
            let p = pat[((x - c.x0) & 3) as usize];
            if steps >= 0x100 {
                set_pixel(dib, x, y, p);
            } else {
                let (ox, oy) = dib.origin();
                let d = dib.pixel(x + ox, y + oy);
                let mut out = 0;
                for s in [0, 8, 16, 24] {
                    let v = (((p >> s) & 0xff) as i32 * steps
                        + ((d >> s) & 0xff) as i32 * (256 - steps))
                        >> 8;
                    out |= (v.min(255) as u32) << s;
                }
                set_pixel(dib, x, y, out);
            }
        }
    }
}

/// `FUN_0017b7e0(cx, cy, r_in, r_out, alpha, dib, colour)`: a disc (`r_in == 0`) or a ring. The
/// branch to the scanline disc compares the raw float bits of the radii.
pub(crate) fn disc(
    dib: &mut Dib,
    cx: f32,
    cy: f32,
    r_in: f32,
    r_out: f32,
    alpha: f32,
    colour: u32,
) {
    let bits = r_out.to_bits();
    if (bits > 0x10 && r_in == 0.0) || (bits > 0x18 && r_out < r_in * 1.25) {
        disc_big(dib, cx, cy, r_in, r_out, alpha, colour);
        return;
    }
    let Some(b) = bbox(dib, cx, cy, r_out) else {
        return;
    };
    let w = (b.x1 - b.x0) as usize;
    let n = (w + 7) & !7;
    let half_r = HALF / r_out;
    let mut c: Vec<f32> = (0..n)
        .map(|i| {
            let v = slope(b.x0 as f32 - cx, 1.0, i);
            v * v
        })
        .collect();
    let a256 = alpha * 256.0;
    let ring = r_in > 1.0;
    let (d, half_ri, ri) = if ring {
        let ri = r_in - 1.0;
        let ri = if ri <= 0.0009765625 { 0.0009765625 } else { ri };
        let half_ri = HALF / ri;
        (
            c.iter().map(|&v| v * half_ri).collect::<Vec<_>>(),
            half_ri,
            ri,
        )
    } else {
        (Vec::new(), 0.0, 0.0)
    };
    for v in &mut c {
        *v *= half_r;
    }
    for y in b.y0..b.y1 {
        let dy = y as f32 - cy;
        let dy2 = dy * dy;
        let base = (r_out * r_out - dy2) * half_r;
        let bterm = (dy2 - ri * ri) * half_ri;
        for x in b.x0..b.x1 {
            let i = (x - b.x0) as usize;
            let mut a = (base - c[i]).min(1.0);
            if ring {
                a *= (d[i] + bterm).min(1.0);
            }
            let cov = trunc32((a.max(0.0) * a256) as f64);
            blend_pixel(dib, x, y, colour, cov);
        }
    }
}

/// `FUN_0017be00`: the scanline disc or ring. Per row the outer and inner half chords give the
/// anti-aliased spans; a disc's interior is filled at `Round(alpha * 256)`.
fn disc_big(dib: &mut Dib, cx: f32, cy: f32, r_in: f32, r_out: f32, alpha: f32, colour: u32) {
    let Some(b) = bbox(dib, cx, cy, r_out) else {
        return;
    };
    let a256 = alpha * 256.0;
    let ring = r_in > 1.0;
    let mid = rne(cx as f64).clamp(b.x0, b.x1);
    let (w, h) = ((b.x1 - b.x0) as usize, (b.y1 - b.y0) as usize);
    let dx2: Vec<f32> = (0..w)
        .map(|i| {
            let v = slope(b.x0 as f32 - cx, 1.0, i);
            v * v
        })
        .collect();
    let r2 = r_out * r_out;
    let (rm1, fill_alpha) = if ring {
        let v = r_in - 1.0;
        (if v <= 0.0009765625 { 0.0009765625 } else { v }, 0)
    } else {
        (r_out - 1.0, rne(a256 as f64))
    };
    let half_r = HALF / r_out;
    let ri1 = {
        let v = r_in - 1.0;
        if v <= 0.0009765625 { 0.0009765625 } else { v }
    };
    let ri2 = ri1 * ri1;
    let half_ri = HALF / ri1;
    let mut prev_end = 0;
    for j in 0..h {
        let y = b.y0 + j as i32;
        let dyv = slope(b.y0 as f32 - cy, 1.0, j);
        let dy2 = dyv * dyv;
        let outer = vvsqrtf(r2 - dy2) + HALF;
        let inner = vvsqrtf((rm1 * rm1 - dy2).max(0.0)) + -0.5;
        for side in 0..2 {
            let (xs, xe) = if side == 0 {
                (
                    rne((cx - outer) as f64).max(b.x0),
                    (rne((cx - inner) as f64) + 1).min(mid),
                )
            } else {
                let xs = rne((cx + inner) as f64).max(mid);
                if !ring {
                    dib.fill_rect(Rect::new(prev_end, y, xs, y + 1), colour, fill_alpha);
                }
                (xs, (rne((cx + outer) as f64) + 1).min(b.x1))
            };
            prev_end = xe;
            if xe <= xs {
                continue;
            }
            let dy = y as f32 - cy;
            for x in xs..xe {
                let seg = dx2[(x - b.x0) as usize];
                let a = if !ring {
                    ((r2 - dy * dy - seg) * (half_r * a256)).min(a256)
                } else {
                    let v16 = dy * dy - ri2;
                    let a = ((r2 - dy * dy - seg) * half_r).min(1.0);
                    let bb = ((seg + v16) * half_ri).min(1.0);
                    (a * bb) * a256
                };
                blend_pixel(dib, x, y, colour, rne(a.max(0.0) as f64));
            }
        }
    }
}

/// `FUN_0017e750(cx, cy, r_in, r_out, alpha, dib, colour, flag)`: the soft ring, coverage
/// `min(1, r_out - d) * ((d - r_in) / (r_out - r_in))^(4 if flag else 1) * alpha * 256`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ring_soft(
    dib: &mut Dib,
    cx: f32,
    cy: f32,
    r_in: f32,
    r_out: f32,
    alpha: f32,
    colour: u32,
    flag: bool,
) {
    let Some(b) = bbox(dib, cx, cy, r_out) else {
        return;
    };
    if r_out - r_in <= 0.0 || (r_out - r_in).is_nan() {
        return;
    }
    let inv = 1.0 / (r_out - r_in);
    let w = (b.x1 - b.x0) as usize;
    let sq: Vec<f32> = (0..w)
        .map(|i| {
            let v = slope(b.x0 as f32 - cx, 1.0, i);
            v * v
        })
        .collect();
    let k = 1.0 - (r_in * inv + 1.0);
    let a256 = alpha * 256.0;
    let mut dy0 = 0.0f32;
    for y in b.y0..b.y1 {
        let dyy = (y as f32 - cy) * (y as f32 - cy);
        if y == b.y0 {
            dy0 = dyy;
        }
        for x in b.x0..b.x1 {
            let i = (x - b.x0) as usize;
            // The squared distances of later rows are built as (dx^2 + dy0^2) + (dy^2 - dy0^2).
            let first = sq[i] + dy0;
            let d2 = if y == b.y0 {
                first
            } else {
                first + (dyy - dy0)
            };
            let dist = vvsqrtf(d2);
            let c = (-dist + r_out).min(1.0);
            let mut bb = dist * inv + k;
            if flag {
                bb *= bb;
                bb *= bb;
            }
            plot(dib, x, y, colour, rne(((bb * c) * a256) as f64));
        }
    }
}

/// `FUN_0017ecb0`: a ring sector. Radial coverage `min(1, r_out - d) * min(1, d - (r_in - 1))`,
/// angular coverage of `atan2 + 3pi/2` wrapped against `[a0, a1]`; `alpha` in 0..1.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arc_ring(
    dib: &mut Dib,
    c1: (f32, f32),
    r_in: f32,
    asp_in: f32,
    c2: (f32, f32),
    r_out: f32,
    asp_out: f32,
    colour: u32,
    a0: f32,
    a1: f32,
    ac: (f32, f32),
    alpha: f32,
) {
    let fv = r_out * asp_out + HALF;
    let fh = r_out + HALF;
    let Some(b) = clip(
        dib,
        rne((c2.0 - fh) as f64) + 1,
        rne((c2.1 - fv) as f64) + 1,
        rne((c2.0 + fh) as f64),
        rne((c2.1 + fv) as f64),
    ) else {
        return;
    };
    let inv_in = 1.0 / asp_in;
    let inv_out = 1.0 / asp_out;
    let a256 = alpha * 256.0;
    let same = c1 == c2 && inv_in == inv_out;
    let mid = (a0 + a1) * HALF;
    let da0 = a0 - mid;
    let da1 = mid - a1;
    let rin1 = r_in - 1.0;
    let fsqrt = |v: f32| (v as f64).sqrt() as f32;
    for y in b.y0..b.y1 {
        let dy_in = (y as f32 - c1.1) * inv_in;
        let dy_out = (y as f32 - c2.1) * inv_out;
        for x in b.x0..b.x1 {
            let dxo = x as f32 - c2.0;
            let d_out = fsqrt(dxo * dxo + dy_out * dy_out);
            let cov_o = (r_out - d_out).min(1.0);
            if cov_o <= 0.0 || cov_o.is_nan() {
                continue;
            }
            let d_in = if same {
                d_out
            } else {
                let dxi = x as f32 - c1.0;
                fsqrt(dxi * dxi + dy_in * dy_in)
            };
            let cov_i = (d_in - rin1).min(1.0);
            let prod = cov_o * cov_i;
            if prod <= 0.0 || prod.is_nan() {
                continue;
            }
            let mut ang = ((x as f32 - ac.0) as f64).atan2((y as f32 - ac.1) as f64) as f32;
            ang += 4.712389;
            if ang > TWO_PI {
                ang -= TWO_PI;
            }
            ang = mid - ang;
            if ang <= PI_F {
                if ang < -PI_F {
                    ang += TWO_PI;
                }
            } else {
                ang -= TWO_PI;
            }
            let cov_a = if ang <= 0.0 {
                (da1 - ang) * d_in + HALF
            } else {
                (ang + da0) * d_in + HALF
            }
            .min(1.0);
            plot(dib, x, y, colour, rne(((prod * cov_a) * a256) as f64));
        }
    }
}

/// `FUN_00006b78(a, b, c)`: `(a * b) sdiv c`.
fn muldiv(a: i64, b: i64, c: i64) -> i64 {
    a * b / c
}

/// `FUN_00177b40(x0, y0, x1, y1, dib, mode 2, colour)`: the anti-aliased line through 20.12 fixed
/// point and `FUN_001761b0`.
pub(crate) fn aa_line(dib: &mut Dib, x0: f32, y0: f32, x1: f32, y1: f32, colour: u32) {
    let p = [x0, y0, x1, y1].map(|v| trunc32((v * 4096.0) as f64) as i64);
    aa_line_fixed(dib, p[0], p[1], p[2], p[3], colour);
}

/// `FUN_001761b0`: x-major or y-major, three pixels per step with soft edges.
fn aa_line_fixed(dib: &mut Dib, px0: i64, py0: i64, px1: i64, py1: i64, colour: u32) {
    let c = dib.clip();
    let (cx0, cy0, cx1, cy1) = (c.x0 as i64, c.y0 as i64, c.x1 as i64, c.y1 as i64);
    let (dx, dy) = (px1 - px0, py1 - py0);
    if dx == 0 && dy == 0 {
        return;
    }
    let mut fdx = dx as f32 * 0.00024414062;
    let mut fdy = dy as f32 * 0.00024414062;
    // The major axis is `u`, the minor `v`; `put(u, v, a)` plots in the right orientation.
    let x_major = dy.abs() < dx.abs();
    let (du, pu0, pv0, pu1, pv1, cu0, cu1, cv0, cv1) = if x_major {
        (dx, px0, py0, px1, py1, cx0, cx1, cy0, cy1)
    } else {
        (dy, py0, px0, py1, px1, cy0, cy1, cx0, cx1)
    };
    let dv = if x_major { dy } else { dx };
    let k = muldiv(dv, 0x1000, du);
    let (mut ua, mut va, mut ub, mut vb) = (pu0, pv0 - 0x800, pu1, pv1 - 0x800);
    if pu1 < pu0 {
        ua = pu1 + 0x1000;
        ub = pu0 + 0x1000;
        va = (pv1 - 0x800) + k;
        vb = (pv0 - 0x800) + k;
        if x_major {
            fdx = -fdx;
        } else {
            fdy = -fdy;
        }
    }
    let fmaj = if x_major { fdx } else { fdy };
    let len = ((fdx * fdx + fdy * fdy) as f64).sqrt() as f32;
    let w = (rne(((fmaj * 2730.6667) / len) as f64) as i64) & 0xffff_ffff;
    let half = w >> 1;
    let i5 = 0x1000 - half;
    let i7 = 0x1000 - ((w * 3) >> 1);
    if !(ua < cu1 * 0x1000 && cu0 * 0x1000 <= ub) {
        return;
    }
    let mut put = |u: i64, v: i64, a: i64| {
        let (x, y) = if x_major { (u, v) } else { (v, u) };
        blend_pixel(dib, x as i32, y as i32, colour, a as i32);
    };
    let three = |put: &mut dyn FnMut(i64, i64, i64), u: i64, vfix: i64, scale: i64| {
        let t = ((vfix & 0xfff) * w) >> 12;
        let vi = ((vfix + 0x80000000) >> 12) - 0x80000;
        for (v, cov) in [
            (vi, i5 - t),
            (vi + 1, 0x1000 - (half - t).abs()),
            (vi + 2, t + i7),
        ] {
            if cv0 <= v && v < cv1 {
                put(u, v, (scale * cov) >> 16);
            }
        }
    };
    let mut uend = ub >> 12;
    if uend < cu1 {
        let frac = ub & 0xfff;
        if frac != 0 {
            three(&mut put, uend, vb - k, frac);
        }
    } else {
        uend = cu1;
    }
    let us = if ua < cu0 * 0x1000 {
        va -= muldiv(ua - (cu0 << 12), k, 0x1000);
        cu0
    } else {
        let mut s = ua >> 12;
        if s != uend {
            let frac = 0x1000 - (ua & 0xfff);
            three(&mut put, s, va, frac);
            s += 1;
            va += muldiv(frac, k, 0x1000);
        }
        s
    };
    for u in us..uend {
        let t = ((va & 0xfff) * w) >> 12;
        if va < (cv0 << 12) || (cv1 * 0x1000 - 0x2000) <= va {
            let vi = ((va + 0x80000000) >> 12) - 0x80000;
            for (v, cov) in [
                (vi, i5 - t),
                (vi + 1, 0x1000 - (half - t).abs()),
                (vi + 2, t + i7),
            ] {
                if cv0 <= v && v < cv1 {
                    put(u, v, cov >> 4);
                }
            }
        } else {
            let vi = va >> 12;
            put(u, vi, (i5 - t) >> 4);
            put(u, vi + 1, (0x1000 - (half - t).abs()) >> 4);
            put(u, vi + 2, (t + i7) >> 4);
        }
        va += k;
    }
}

/// Profile table 1 of `DAT_003b7dc8`: `Round((i/256)^(5/6) * 256)`.
fn profile(i: usize) -> i32 {
    rne((i as f64 / 256.0).powf(5.0 / 6.0) * 256.0)
}

/// `FUN_001777d0(y0, y1, y2, y3, dib, x, colour, table 1, alpha)` in blend mode 1: one pixel
/// column from `Round(min(y0, y1) - 0.5)` to `Round(max(y2, y3) + 0.5)` with the coverage ramping
/// up over `[y0, y1]` and down over `[y2, y3]`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn vspan_add(
    dib: &mut Dib,
    y0: f32,
    y1: f32,
    y2: f32,
    y3: f32,
    x: i32,
    colour: u32,
    alpha: i32,
) {
    let c = dib.clip();
    if !(c.x0 <= x && x < c.x1) {
        return;
    }
    let (lo_a, hi_a) = if y1 < y0 { (y1, y0) } else { (y0, y1) };
    let (hi_b, lo_b) = if y3 > y2 { (y3, y2) } else { (y2, y3) };
    let mut r0 = rne((lo_a + -0.49999997) as f64);
    let mut r1 = rne((hi_b + 0.49999997) as f64);
    if !(r0 < c.y1 && c.y0 <= r1) {
        return;
    }
    r0 = r0.max(c.y0);
    r1 = r1.min(c.y1 - 1);
    let step_up = 256.0f32 / ((hi_a - lo_a) + 1.0);
    let step_dn = 1.0f32 / ((lo_b - hi_b) - 1.0);
    let mut pu = ((r0 as f32 - lo_a) + 1.0) * step_up;
    let mut pd = ((r0 as f32 - lo_b) * step_dn) + 1.0;
    for y in r0..=r1 {
        let idx = trunc32((pu.clamp(0.0, 256.0) * pd.clamp(0.0, 1.0)) as f64);
        let a = (profile(idx.clamp(0, 256) as usize) * alpha) >> 8;
        add_pixel(dib, x, y, colour, a);
        pu += step_up;
        pd += step_dn;
    }
}
