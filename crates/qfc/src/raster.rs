//! The AggPas glyph pipeline of `FUN_0013fdd0` after FreeType (gfx.md section 5.3): outline
//! decomposition into the integer path storage, `conv_curve` (approximation scale 3),
//! `trans_affine`, the anisotropic `conv_contour` (`vcgen_contour` with the FPC maths) and
//! `rasterizer_scanline_aa` rendered on a gray8 buffer.

use crate::ft;
use std::collections::HashMap;

pub const CMD_STOP: u8 = 0;
pub const CMD_MOVE_TO: u8 = 1;
pub const CMD_LINE_TO: u8 = 2;
pub const CMD_CURVE3: u8 = 3;
pub const CMD_CURVE4: u8 = 4;
pub const CMD_END_POLY: u8 = 0x0f;
pub const FLAG_CLOSE: u8 = 0x40;

/// A path vertex: command and coordinates.
pub type Vertex = (u8, f64, f64);

/// One int26.6 outline coordinate as the integer path storage returns it: `dbl_to_int26p6(v / 64)
/// = trunc(v + 0.5)` moves negative coordinates up by one unit.
fn coord(v: i64) -> f64 {
    (if v < 0 { v + 1 } else { v }) as f64 / 64.0
}

/// `font_engine_freetype_base::decompose_ft_outline` (AggPas) without flip and with the identity
/// affine. `None` for a malformed outline.
pub fn decompose(o: &ft::Outline) -> Option<Vec<Vertex>> {
    let n_contours = o.n_contours.max(0) as usize;
    let n_points = o.n_points.max(0) as usize;
    if n_contours == 0 {
        return Some(Vec::new());
    }
    if o.points.is_null() || o.tags.is_null() || o.contours.is_null() {
        return None;
    }
    // SAFETY: FreeType keeps `n_points` points and tags and `n_contours` contour ends.
    let (pts, tags, contours) = unsafe {
        (
            std::slice::from_raw_parts(o.points, n_points),
            std::slice::from_raw_parts(o.tags as *const u8, n_points),
            std::slice::from_raw_parts(o.contours, n_contours),
        )
    };
    // `FT_Pos` is a C long: 32-bit on Windows.
    #[allow(clippy::useless_conversion)]
    let p = |i: isize| -> (i64, i64) {
        let v = pts[i as usize];
        (i64::from(v.x), i64::from(v.y))
    };
    let tag = |i: isize| tags[i as usize] & 3;
    let mut path = Vec::with_capacity(n_points * 2 + n_contours * 2);
    let mut first = 0isize;
    for &last in contours {
        let last = last as isize;
        if last < first || last as usize >= n_points {
            return None;
        }
        let mut limit = last;
        let mut v_start = p(first);
        let v_last = p(last);
        let mut point = first;
        let t = tag(first);
        if t == 2 {
            return None;
        }
        if t == 0 {
            if tag(last) == 1 {
                v_start = v_last;
                limit -= 1;
            } else {
                v_start = (half(v_start.0 + v_last.0), half(v_start.1 + v_last.1));
            }
            point -= 1;
        }
        path.push((CMD_MOVE_TO, coord(v_start.0), coord(v_start.1)));
        while point < limit {
            point += 1;
            match tag(point) {
                1 => {
                    let v = p(point);
                    path.push((CMD_LINE_TO, coord(v.0), coord(v.1)));
                }
                0 => {
                    let mut v_control = p(point);
                    loop {
                        if point < limit {
                            point += 1;
                            let vec = p(point);
                            match tag(point) {
                                1 => {
                                    path.push((CMD_CURVE3, coord(v_control.0), coord(v_control.1)));
                                    path.push((CMD_CURVE3, coord(vec.0), coord(vec.1)));
                                    break;
                                }
                                0 => {
                                    let mid =
                                        (half(v_control.0 + vec.0), half(v_control.1 + vec.1));
                                    path.push((CMD_CURVE3, coord(v_control.0), coord(v_control.1)));
                                    path.push((CMD_CURVE3, coord(mid.0), coord(mid.1)));
                                    v_control = vec;
                                }
                                _ => return None,
                            }
                        } else {
                            path.push((CMD_CURVE3, coord(v_control.0), coord(v_control.1)));
                            path.push((CMD_CURVE3, coord(v_start.0), coord(v_start.1)));
                            point = limit + 1;
                            break;
                        }
                    }
                }
                _ => {
                    if point + 1 > limit || tag(point + 1) != 2 {
                        return None;
                    }
                    let (v1, v2) = (p(point), p(point + 1));
                    point += 2;
                    path.push((CMD_CURVE4, coord(v1.0), coord(v1.1)));
                    path.push((CMD_CURVE4, coord(v2.0), coord(v2.1)));
                    let end = if point <= limit { p(point) } else { v_start };
                    path.push((CMD_CURVE4, coord(end.0), coord(end.1)));
                }
            }
        }
        path.push((CMD_END_POLY | FLAG_CLOSE, 0.0, 0.0));
        first = last + 1;
    }
    Some(path)
}

/// C integer division by 2 (truncation toward zero).
fn half(v: i64) -> i64 {
    v / 2
}

const CURVE_RECURSION_LIMIT: u32 = 32;
const CURVE_COLLINEARITY_EPS: f64 = 1e-30;

/// `curve3_div` / `curve4_div` of AggPas with angle tolerance 0 and cusp limit 0 (so every flat
/// enough subdivision adds its midpoint; the collinear case uses the AGG 2.3 manhattan rule).
struct CurveDiv {
    tol: f64,
    tol_manhattan: f64,
}

impl CurveDiv {
    fn new(scale: f64) -> Self {
        let t = 0.5 / scale;
        Self {
            tol: t * t,
            tol_manhattan: 4.0 / scale,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn curve3(
        &self,
        out: &mut Vec<Vertex>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x3: f64,
        y3: f64,
        level: u32,
    ) {
        if level > CURVE_RECURSION_LIMIT {
            return;
        }
        let (x12, y12) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let (x23, y23) = ((x2 + x3) / 2.0, (y2 + y3) / 2.0);
        let (x123, y123) = ((x12 + x23) / 2.0, (y12 + y23) / 2.0);
        let (dx, dy) = (x3 - x1, y3 - y1);
        let d = ((x2 - x3) * dy - (y2 - y3) * dx).abs();
        if d > CURVE_COLLINEARITY_EPS {
            if d * d <= self.tol * (dx * dx + dy * dy) {
                out.push((CMD_LINE_TO, x123, y123));
                return;
            }
        } else if (x1 + x3 - x2 - x2).abs() + (y1 + y3 - y2 - y2).abs() <= self.tol_manhattan {
            out.push((CMD_LINE_TO, x123, y123));
            return;
        }
        self.curve3(out, x1, y1, x12, y12, x123, y123, level + 1);
        self.curve3(out, x123, y123, x23, y23, x3, y3, level + 1);
    }

    #[allow(clippy::too_many_arguments)]
    fn curve4(
        &self,
        out: &mut Vec<Vertex>,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x3: f64,
        y3: f64,
        x4: f64,
        y4: f64,
        level: u32,
    ) {
        if level > CURVE_RECURSION_LIMIT {
            return;
        }
        let (x12, y12) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
        let (x23, y23) = ((x2 + x3) / 2.0, (y2 + y3) / 2.0);
        let (x34, y34) = ((x3 + x4) / 2.0, (y3 + y4) / 2.0);
        let (x123, y123) = ((x12 + x23) / 2.0, (y12 + y23) / 2.0);
        let (x234, y234) = ((x23 + x34) / 2.0, (y23 + y34) / 2.0);
        let (x1234, y1234) = ((x123 + x234) / 2.0, (y123 + y234) / 2.0);
        let (dx, dy) = (x4 - x1, y4 - y1);
        let d2 = ((x2 - x4) * dy - (y2 - y4) * dx).abs();
        let d3 = ((x3 - x4) * dy - (y3 - y4) * dx).abs();
        let case = (d2 > CURVE_COLLINEARITY_EPS) as u8 * 2 + (d3 > CURVE_COLLINEARITY_EPS) as u8;
        let len2 = dx * dx + dy * dy;
        match case {
            0 => {
                if (x1 + x3 - x2 - x2).abs()
                    + (y1 + y3 - y2 - y2).abs()
                    + (x2 + x4 - x3 - x3).abs()
                    + (y2 + y4 - y3 - y3).abs()
                    <= self.tol_manhattan
                {
                    out.push((CMD_LINE_TO, x1234, y1234));
                    return;
                }
            }
            1 => {
                if d3 * d3 <= self.tol * len2 {
                    out.push((CMD_LINE_TO, x23, y23));
                    return;
                }
            }
            2 => {
                if d2 * d2 <= self.tol * len2 {
                    out.push((CMD_LINE_TO, x23, y23));
                    return;
                }
            }
            _ => {
                if (d2 + d3) * (d2 + d3) <= self.tol * len2 {
                    out.push((CMD_LINE_TO, x23, y23));
                    return;
                }
            }
        }
        self.curve4(out, x1, y1, x12, y12, x123, y123, x1234, y1234, level + 1);
        self.curve4(out, x1234, y1234, x234, y234, x34, y34, x4, y4, level + 1);
    }
}

/// `conv_curve`: curve commands become line vertices of the subdivided curves.
pub fn conv_curve(path: &[Vertex], scale: f64) -> Vec<Vertex> {
    let div = CurveDiv::new(scale);
    let mut out = Vec::with_capacity(path.len() * 4);
    let mut last = (0.0, 0.0);
    let mut i = 0;
    while i < path.len() {
        let (cmd, x, y) = path[i];
        match cmd {
            CMD_CURVE3 if i + 1 < path.len() => {
                let (_, ex, ey) = path[i + 1];
                div.curve3(&mut out, last.0, last.1, x, y, ex, ey, 0);
                out.push((CMD_LINE_TO, ex, ey));
                last = (ex, ey);
                i += 2;
            }
            CMD_CURVE4 if i + 2 < path.len() => {
                let (_, c2x, c2y) = path[i + 1];
                let (_, ex, ey) = path[i + 2];
                div.curve4(&mut out, last.0, last.1, x, y, c2x, c2y, ex, ey, 0);
                out.push((CMD_LINE_TO, ex, ey));
                last = (ex, ey);
                i += 3;
            }
            _ => {
                out.push((cmd, x, y));
                if cmd == CMD_MOVE_TO || cmd == CMD_LINE_TO {
                    last = (x, y);
                }
                i += 1;
            }
        }
    }
    out
}

/// `trans_affine`.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub sx: f64,
    pub shy: f64,
    pub shx: f64,
    pub sy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(sx: f64, shy: f64, shx: f64, sy: f64, tx: f64, ty: f64) -> Self {
        Self {
            sx,
            shy,
            shx,
            sy,
            tx,
            ty,
        }
    }

    pub const fn scaling(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    pub fn skewing(sx: f64, sy: f64) -> Self {
        Self::new(1.0, sy.tan(), sx.tan(), 1.0, 0.0, 0.0)
    }

    pub const fn translation(tx: f64, ty: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    /// `trans_affine::multiply`: `self` first, then `m`.
    pub fn multiply(mut self, m: &Affine) -> Self {
        let t0 = self.sx * m.sx + self.shy * m.shx;
        let t2 = self.shx * m.sx + self.sy * m.shx;
        let t4 = self.tx * m.sx + self.ty * m.shx + m.tx;
        self.shy = self.sx * m.shy + self.shy * m.sy;
        self.sy = self.shx * m.shy + self.sy * m.sy;
        self.ty = self.tx * m.shy + self.ty * m.sy + m.ty;
        self.sx = t0;
        self.shx = t2;
        self.tx = t4;
        self
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x * self.sx + y * self.shx + self.tx,
            x * self.shy + y * self.sy + self.ty,
        )
    }
}

/// Transforms the vertex commands of `path` in place.
pub fn transform(path: &mut [Vertex], m: &Affine) {
    for v in path {
        let c = v.0 & 0x0f;
        if c != CMD_END_POLY && c != CMD_STOP {
            let (x, y) = m.apply(v.1, v.2);
            v.1 = x;
            v.2 = y;
        }
    }
}

/// `FUN_0014d710`: min/max over the vertex commands.
pub fn bounding_rect(path: &[Vertex]) -> Option<(f64, f64, f64, f64)> {
    let mut bb: Option<(f64, f64, f64, f64)> = None;
    for &(cmd, x, y) in path {
        if (1..=4).contains(&(cmd & 0x0f)) {
            bb = Some(match bb {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    bb
}

/// `FUN_0014dc20`: the path scaled by `(1, 1/a)` with `a = max(aspect, 0.0001)`, contoured by
/// `vcgen_contour(width, join)` and scaled back.
pub fn contour(path: &[Vertex], width: f64, aspect: f64, join: u8) -> Vec<Vertex> {
    let a = if aspect >= 0.0001 { aspect } else { 0.0001 };
    let mut p = path.to_vec();
    transform(&mut p, &Affine::scaling(1.0, 1.0 / a));
    let mut out = vcgen_contour(&p, width, join);
    transform(&mut out, &Affine::scaling(1.0, a));
    out
}

// FPC RTL maths of the stroker: FUN_0008b250 hypot, FUN_0008b150 arctan2, FUN_0008b0c0 arccos.

fn fpc_hypot(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    if x <= y {
        if x > 0.0 {
            y * ((x / y) * (x / y) + 1.0).sqrt()
        } else {
            y
        }
    } else {
        x * ((y / x) * (y / x) + 1.0).sqrt()
    }
}

fn fpc_atan2(y: f64, x: f64) -> f64 {
    let mut r = if x == 0.0 {
        if y == 0.0 {
            0.0
        } else if y > 0.0 {
            std::f64::consts::FRAC_PI_2
        } else {
            -std::f64::consts::FRAC_PI_2
        }
    } else {
        (y / x).atan()
    };
    if x < 0.0 {
        r += std::f64::consts::PI;
    }
    if r > std::f64::consts::PI {
        r -= std::f64::consts::TAU;
    }
    r
}

fn fpc_acos(x: f64) -> f64 {
    if x.abs() == 1.0 {
        return if x >= 0.0 { 0.0 } else { std::f64::consts::PI };
    }
    fpc_atan2(((1.0 - x) * (x + 1.0)).sqrt(), x)
}

const VERTEX_DIST_EPS: f64 = 1e-14;
const INTERSECTION_EPS: f64 = 1e-30;
const MITER_LIMIT: f64 = 4.0;
const INNER_MITER_LIMIT: f64 = 1.015625;
const JOIN_MITER_REVERT: u8 = 1;
const JOIN_MITER_ROUND: u8 = 2;
const JOIN_ROUND: u8 = 3;

/// A `vertex_dist`: x, y and the distance to the next vertex.
#[derive(Clone, Copy)]
struct VDist {
    x: f64,
    y: f64,
    d: f64,
}

/// The `vertex_dist` functor: sets `a.d` and returns false (with `d = 1e14`) if too close.
fn vdist(a: &mut VDist, b: VDist) -> bool {
    a.d = fpc_hypot(b.x - a.x, b.y - a.y);
    let ok = a.d > VERTEX_DIST_EPS;
    if !ok {
        a.d = 1.0 / VERTEX_DIST_EPS;
    }
    ok
}

/// `vertex_sequence<vertex_dist>`.
#[derive(Default)]
struct VertexSequence(Vec<VDist>);

impl VertexSequence {
    fn add(&mut self, x: f64, y: f64) {
        let n = self.0.len();
        if n > 1 {
            let b = self.0[n - 1];
            if !vdist(&mut self.0[n - 2], b) {
                self.0.pop();
            }
        }
        self.0.push(VDist { x, y, d: 0.0 });
    }

    fn modify_last(&mut self, x: f64, y: f64) {
        self.0.pop();
        self.add(x, y);
    }

    fn close(&mut self, closed: bool) {
        while self.0.len() > 1 {
            let n = self.0.len();
            let b = self.0[n - 1];
            if vdist(&mut self.0[n - 2], b) {
                break;
            }
            let t = self.0.pop().unwrap();
            self.modify_last(t.x, t.y);
        }
        if closed {
            while self.0.len() > 1 {
                let n = self.0.len();
                let b = self.0[0];
                if vdist(&mut self.0[n - 1], b) {
                    break;
                }
                self.0.pop();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn calc_intersection(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    cx: f64,
    cy: f64,
    dx: f64,
    dy: f64,
) -> Option<(f64, f64)> {
    let den = (bx - ax) * (dy - cy) - (by - ay) * (dx - cx);
    if den.abs() < INTERSECTION_EPS {
        return None;
    }
    let r = ((ay - cy) * (dx - cx) - (ax - cx) * (dy - cy)) / den;
    Some((ax + r * (bx - ax), ay + r * (by - ay)))
}

#[allow(clippy::too_many_arguments)]
fn calc_arc(
    out: &mut Vec<(f64, f64)>,
    x: f64,
    y: f64,
    dx1: f64,
    dy1: f64,
    dx2: f64,
    dy2: f64,
    width: f64,
    approx: f64,
) {
    let mut a1 = fpc_atan2(dy1, dx1);
    let mut a2 = fpc_atan2(dy2, dx2);
    let ccw = a1 - a2 > 0.0 && a1 - a2 < std::f64::consts::PI;
    let width = width.abs();
    let approx = if approx == 0.0 { 1e-05 } else { approx };
    let da = fpc_acos(width / (width + 0.125 / approx)) * 2.0;
    out.push((x + dx1, y + dy1));
    if !ccw {
        if a1 > a2 {
            a2 += std::f64::consts::TAU;
        }
        a1 += da;
        while a1 < a2 - da * 0.25 {
            out.push((x + a1.cos() * width, y + a1.sin() * width));
            a1 += da;
        }
    } else {
        if a1 < a2 {
            a2 -= std::f64::consts::TAU;
        }
        a1 -= da;
        while a1 > a2 + da * 0.25 {
            out.push((x + a1.cos() * width, y + a1.sin() * width));
            a1 -= da;
        }
    }
    out.push((x + dx2, y + dy2));
}

#[allow(clippy::too_many_arguments)]
fn calc_miter(
    out: &mut Vec<(f64, f64)>,
    v0: VDist,
    v1: VDist,
    v2: VDist,
    dx1: f64,
    dy1: f64,
    dx2: f64,
    dy2: f64,
    width: f64,
    miter_limit: f64,
    approx: f64,
    join: u8,
) {
    let mut exceeded = true;
    match calc_intersection(
        v0.x + dx1,
        v0.y - dy1,
        v1.x + dx1,
        v1.y - dy1,
        v1.x + dx2,
        v1.y - dy2,
        v2.x + dx2,
        v2.y - dy2,
    ) {
        None => {
            let x2 = v1.x + dx1;
            let y2 = v1.y - dy1;
            if ((x2 - v0.x) * dy1 - (v0.y - y2) * dx1 < 0.0)
                != ((x2 - v2.x) * dy1 - (v2.y - y2) * dx1 < 0.0)
            {
                out.push((v1.x + dx1, v1.y - dy1));
                exceeded = false;
            }
        }
        Some(xi) => {
            if fpc_hypot(xi.0 - v1.x, xi.1 - v1.y) <= width * miter_limit {
                out.push(xi);
                exceeded = false;
            }
        }
    }
    if !exceeded {
        return;
    }
    match join {
        JOIN_MITER_REVERT => {
            out.push((v1.x + dx1, v1.y - dy1));
            out.push((v1.x + dx2, v1.y - dy2));
        }
        JOIN_MITER_ROUND => calc_arc(out, v1.x, v1.y, dx1, -dy1, dx2, -dy2, width, approx),
        _ => {
            out.push((
                v1.x + dx1 + dy1 * miter_limit,
                v1.y - dy1 + dx1 * miter_limit,
            ));
            out.push((
                v1.x + dx2 - dy2 * miter_limit,
                v1.y - dy2 - dx2 * miter_limit,
            ));
        }
    }
}

/// `stroke_calc_join` with the inner miter join, miter limit 4, inner miter limit 1.015625 and
/// approximation scale 1.
fn stroke_calc_join(
    out: &mut Vec<(f64, f64)>,
    v0: VDist,
    v1: VDist,
    v2: VDist,
    width: f64,
    join: u8,
) {
    let (len1, len2) = (v0.d, v1.d);
    let dx1 = width * (v1.y - v0.y) / len1;
    let dy1 = width * (v1.x - v0.x) / len1;
    let dx2 = width * (v2.y - v1.y) / len2;
    let dy2 = width * (v2.x - v1.x) / len2;
    out.clear();
    let cp = (v2.x - v1.x) * (v1.y - v0.y) - (v2.y - v1.y) * (v1.x - v0.x);
    if cp <= 0.0 {
        if join < JOIN_ROUND {
            calc_miter(
                out,
                v0,
                v1,
                v2,
                dx1,
                dy1,
                dx2,
                dy2,
                width,
                MITER_LIMIT,
                1.0,
                join,
            );
        } else if join != JOIN_ROUND {
            out.push((v1.x + dx1, v1.y - dy1));
            out.push((v1.x + dx2, v1.y - dy2));
        } else {
            calc_arc(out, v1.x, v1.y, dx1, -dy1, dx2, -dy2, width, 1.0);
        }
        return;
    }
    calc_miter(
        out,
        v0,
        v1,
        v2,
        dx1,
        dy1,
        dx2,
        dy2,
        width,
        INNER_MITER_LIMIT,
        1.0,
        JOIN_MITER_REVERT,
    );
}

/// `conv_contour`: every closed polygon becomes the contour polygon of `vcgen_contour` at
/// `width / 2` (orientation none). Polygons with fewer than three distinct vertices produce
/// nothing.
fn vcgen_contour(path: &[Vertex], width: f64, join: u8) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(path.len() * 2);
    let half = width * 0.5;
    let mut joined = Vec::new();
    let mut i = 0;
    let n = path.len();
    while i < n {
        let (cmd, x, y) = path[i];
        if cmd & 0x0f != CMD_MOVE_TO {
            i += 1;
            continue;
        }
        let mut seq = VertexSequence::default();
        seq.modify_last(x, y);
        let mut closed = false;
        i += 1;
        while i < n {
            let (cmd, x, y) = path[i];
            let c = cmd & 0x0f;
            if c == CMD_MOVE_TO {
                break;
            }
            i += 1;
            if c == CMD_LINE_TO || c == CMD_CURVE3 || c == CMD_CURVE4 {
                seq.add(x, y);
            } else if c == CMD_END_POLY {
                closed = cmd & FLAG_CLOSE != 0;
                break;
            } else if c == CMD_STOP {
                break;
            }
        }
        seq.close(true);
        let v = &seq.0;
        if v.len() < 2 + closed as usize {
            continue;
        }
        let mut first = true;
        let len = v.len();
        for k in 0..len {
            let prev = v[(len + k - 1) % len];
            let curr = v[k];
            let next = v[(k + 1) % len];
            stroke_calc_join(&mut joined, prev, curr, next, half, join);
            for &(px, py) in &joined {
                out.push((if first { CMD_MOVE_TO } else { CMD_LINE_TO }, px, py));
                first = false;
            }
        }
        if closed {
            out.push((CMD_END_POLY | FLAG_CLOSE | 0x10, 0.0, 0.0));
        }
    }
    out
}

const POLY_SHIFT: i32 = 8;
const POLY_SCALE: i64 = 1 << POLY_SHIFT;
const POLY_MASK: i64 = POLY_SCALE - 1;

/// `poly_coord` (`FUN_0014a960`): truncation toward zero.
fn poly_coord(c: f64) -> i64 {
    (c * 256.0) as i64
}

/// `rasterizer_scanline_aa` (AGG 2.4 cells, no clipping, cells merged by position, non-zero
/// rule, identity gamma).
#[derive(Default)]
pub struct Rasterizer {
    cells: HashMap<(i64, i64), (i64, i64)>,
    cur: (i64, i64),
    start: (i64, i64),
    has_start: bool,
}

impl Rasterizer {
    fn cell(&mut self, ex: i64, ey: i64, cover: i64, area: i64) {
        let c = self.cells.entry((ex, ey)).or_insert((0, 0));
        c.0 += cover;
        c.1 += area;
    }

    fn move_to(&mut self, x: i64, y: i64) {
        if self.has_start {
            self.close_polygon();
        }
        self.start = (x, y);
        self.cur = (x, y);
        self.has_start = true;
    }

    fn line_to(&mut self, x: i64, y: i64) {
        self.render_line(self.cur.0, self.cur.1, x, y);
        self.cur = (x, y);
    }

    fn close_polygon(&mut self) {
        if self.has_start {
            self.render_line(self.cur.0, self.cur.1, self.start.0, self.start.1);
            self.cur = self.start;
            self.has_start = false;
        }
    }

    pub fn add_path(&mut self, path: &[Vertex]) {
        for &(cmd, x, y) in path {
            match cmd & 0x0f {
                CMD_MOVE_TO => self.move_to(poly_coord(x), poly_coord(y)),
                CMD_LINE_TO => self.line_to(poly_coord(x), poly_coord(y)),
                CMD_END_POLY => self.close_polygon(),
                _ => {}
            }
        }
        self.close_polygon();
    }

    fn render_hline(&mut self, ey: i64, x1: i64, y1: i64, x2: i64, y2: i64) {
        let mut ex1 = x1 >> POLY_SHIFT;
        let ex2 = x2 >> POLY_SHIFT;
        let fx1 = x1 & POLY_MASK;
        let fx2 = x2 & POLY_MASK;
        if y1 == y2 {
            return;
        }
        if ex1 == ex2 {
            let delta = y2 - y1;
            self.cell(ex1, ey, delta, (fx1 + fx2) * delta);
            return;
        }
        let mut p = (POLY_SCALE - fx1) * (y2 - y1);
        let mut first = POLY_SCALE;
        let mut incr = 1;
        let mut dx = x2 - x1;
        if dx < 0 {
            p = fx1 * (y2 - y1);
            first = 0;
            incr = -1;
            dx = -dx;
        }
        let mut delta = div_trunc(p, dx);
        let mut m = p - delta * dx;
        if m < 0 {
            delta -= 1;
            m += dx;
        }
        self.cell(ex1, ey, delta, (fx1 + first) * delta);
        ex1 += incr;
        let mut y1 = y1 + delta;
        if ex1 != ex2 {
            let p = POLY_SCALE * (y2 - y1 + delta);
            let mut lift = div_trunc(p, dx);
            let mut rem = p - lift * dx;
            if rem < 0 {
                lift -= 1;
                rem += dx;
            }
            m -= dx;
            while ex1 != ex2 {
                delta = lift;
                m += rem;
                if m >= 0 {
                    m -= dx;
                    delta += 1;
                }
                self.cell(ex1, ey, delta, POLY_SCALE * delta);
                y1 += delta;
                ex1 += incr;
            }
        }
        let delta = y2 - y1;
        self.cell(ex1, ey, delta, (fx2 + POLY_SCALE - first) * delta);
    }

    fn render_line(&mut self, x1: i64, y1: i64, x2: i64, y2: i64) {
        let mut ey1 = y1 >> POLY_SHIFT;
        let ey2 = y2 >> POLY_SHIFT;
        let fy1 = y1 & POLY_MASK;
        let fy2 = y2 & POLY_MASK;
        if ey1 == ey2 {
            self.render_hline(ey1, x1, fy1, x2, fy2);
            return;
        }
        let dx = x2 - x1;
        let mut dy = y2 - y1;
        let mut incr = 1;
        if dx == 0 {
            let ex = x1 >> POLY_SHIFT;
            let two_fx = (x1 - (ex << POLY_SHIFT)) << 1;
            let mut first = POLY_SCALE;
            if dy < 0 {
                first = 0;
                incr = -1;
            }
            let delta = first - fy1;
            self.cell(ex, ey1, delta, two_fx * delta);
            ey1 += incr;
            let delta = first + first - POLY_SCALE;
            let area = two_fx * delta;
            while ey1 != ey2 {
                self.cell(ex, ey1, delta, area);
                ey1 += incr;
            }
            let delta = fy2 - POLY_SCALE + first;
            self.cell(ex, ey1, delta, two_fx * delta);
            return;
        }
        let mut p = (POLY_SCALE - fy1) * dx;
        let mut first = POLY_SCALE;
        if dy < 0 {
            p = fy1 * dx;
            first = 0;
            incr = -1;
            dy = -dy;
        }
        let mut delta = div_trunc(p, dy);
        let mut m = p - delta * dy;
        if m < 0 {
            delta -= 1;
            m += dy;
        }
        let mut x_from = x1 + delta;
        self.render_hline(ey1, x1, fy1, x_from, first);
        ey1 += incr;
        if ey1 != ey2 {
            let p = POLY_SCALE * dx;
            let mut lift = div_trunc(p, dy);
            let mut rem = p - lift * dy;
            if rem < 0 {
                lift -= 1;
                rem += dy;
            }
            m -= dy;
            while ey1 != ey2 {
                delta = lift;
                m += rem;
                if m >= 0 {
                    m -= dy;
                    delta += 1;
                }
                let x_to = x_from + delta;
                self.render_hline(ey1, x_from, POLY_SCALE - first, x_to, first);
                x_from = x_to;
                ey1 += incr;
            }
        }
        self.render_hline(ey1, x_from, POLY_SCALE - first, x2, fy2);
    }

    /// `FUN_0014cf30`: `|area div 0x200|` (C truncation) clamped to 255.
    fn alpha(area: i64) -> i64 {
        div_trunc(area, 1 << (POLY_SHIFT * 2 + 1 - 8))
            .abs()
            .min(255)
    }

    /// Sweeps the cells and renders them on a `w` x `h` gray8 buffer with
    /// `renderer_scanline_aa_solid` (colour 255, alpha 255).
    pub fn render_gray8(&self, w: usize, h: usize) -> Vec<u8> {
        let mut bmp = vec![0u8; w * h];
        let mut cells: Vec<(i64, i64, i64, i64)> = self
            .cells
            .iter()
            .filter(|(_, c)| c.0 != 0 || c.1 != 0)
            .map(|(&(x, y), &(c, a))| (y, x, c, a))
            .collect();
        cells.sort_unstable_by_key(|c| (c.0, c.1));
        let mut plot = |x: i64, y: i64, len: i64, cover: i64| {
            if y < 0 || y >= h as i64 || cover == 0 {
                return;
            }
            let alpha = (255 * (cover + 1)) >> 8;
            let row = y as usize * w;
            for px in x..x + len {
                if px < 0 || px >= w as i64 {
                    continue;
                }
                let p = &mut bmp[row + px as usize];
                *p = if alpha == 255 {
                    255
                } else {
                    (((255 - *p as i64) * alpha + ((*p as i64) << 8)) >> 8) as u8
                };
            }
        };
        let mut i = 0;
        while i < cells.len() {
            let ey = cells[i].0;
            let mut cover = 0;
            while i < cells.len() && cells[i].0 == ey {
                let mut x = cells[i].1;
                let area = cells[i].3;
                cover += cells[i].2;
                i += 1;
                if area != 0 {
                    plot(x, ey, 1, Self::alpha((cover << (POLY_SHIFT + 1)) - area));
                    x += 1;
                }
                if i < cells.len() && cells[i].0 == ey && cells[i].1 > x {
                    plot(
                        x,
                        ey,
                        cells[i].1 - x,
                        Self::alpha(cover << (POLY_SHIFT + 1)),
                    );
                }
            }
        }
        bmp
    }
}

fn div_trunc(a: i64, b: i64) -> i64 {
    a / b
}
