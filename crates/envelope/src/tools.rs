//! The editing tools of the envelope editor (envelope.md section 5).
//!
//! Each tool works on the envelope in place, like the original, which edits the slot object
//! itself. "Selection set" means the selected points if more than one point is selected, else all
//! points. Tools that the original refuses return a [`ToolError`] with the original message.

use crate::{Cursor, EPS24, Envelope, Point, Shape, curve_params, rne};
use std::fmt;

/// ln(1001), the Scale levels tension scale.
const LN_1001: f64 = 6.90875477931522;
/// ln(10001), the decimation threshold scale.
const LN_10001: f64 = 9.210440366976517;
/// ln(21), the Create sequence tension scale.
const LN_21: f64 = 3.044522437723423;
/// ln(0.01), the follower coefficient.
const LN_001: f64 = -4.605170185988091;
/// 0.001 as f32, widened (0x35462c).
const F32_0001: f64 = 0.0010000000474974513;
/// The "unbounded length" sentinel.
const UNBOUNDED: f64 = 2147483647.0;
/// The first decimation pass threshold (0x2b0f40).
const DECIMATE_EXACT: f64 = 9.5367431640625e-07;

/// A refusal of a tool, with the original title and message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolError {
    pub title: &'static str,
    pub message: String,
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.title, self.message)
    }
}

impl std::error::Error for ToolError {}

fn refuse(title: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        title,
        message: message.into(),
    }
}

fn not_continuous(tool: &str) -> ToolError {
    refuse(
        "Can't decimate",
        format!(
            "The selection must have at least two points and be continuous for the {tool} tool to work."
        ),
    )
}

/// The Smooth up dialog values (TMESmoothToolForm, envelope.md 5.10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SmoothSettings {
    pub attack: i32,
    pub release: i32,
    pub attack_slope: i32,
    pub release_slope: i32,
    pub offset: i32,
    pub decimate: i32,
    pub link: bool,
}

impl Default for SmoothSettings {
    fn default() -> Self {
        Self {
            attack: 500,
            release: 500,
            attack_slope: 500,
            release_slope: 500,
            offset: 0,
            decimate: 500,
            link: false,
        }
    }
}

/// The Filter dialog values (TMEEnvFilterToolForm, envelope.md 5.10.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterSettings {
    pub width: i32,
    pub offset: i32,
    pub decimate: i32,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            width: 500,
            offset: 0,
            decimate: 500,
        }
    }
}

/// The Scale levels dialog values (TMEScaleToolForm, envelope.md 5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaleSettings {
    pub center: i32,
    pub multiply: i32,
    pub offset: i32,
    pub tension: i32,
}

impl Default for ScaleSettings {
    fn default() -> Self {
        Self {
            center: 0,
            multiply: 8192,
            offset: 0,
            tension: 0,
        }
    }
}

/// One step of the Create sequence table (envelope.md 5.11.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeqStep {
    /// 0 Off, 1 On, 2 Hold, 3 Stick.
    pub kind: i32,
    /// Arpeggiator break (unused in Gross Beat).
    pub brk: i32,
    pub level: i32,
    pub decay_slope: i32,
    pub sustain: i32,
    pub release_slope: i32,
}

/// The default step table (0xcb160): 8 On steps, then Off.
pub fn default_steps() -> [SeqStep; 32] {
    std::array::from_fn(|i| SeqStep {
        kind: (i < 8) as i32,
        brk: 4,
        level: 65536,
        decay_slope: 32768,
        sustain: 32768,
        release_slope: 32768,
    })
}

/// The per-field metadata of the step table: (default, max).
pub const SEQ_FIELDS: [(i32, i32); 6] = [
    (1, 3),
    (4, 5),
    (65536, 65536),
    (32768, 65536),
    (32768, 65536),
    (32768, 65536),
];

/// The Create sequence dialog values (TMEArpToolForm, envelope.md 5.11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeqSettings {
    pub ping_pong: bool,
    pub time_mul: i32,
    pub swing: i32,
    pub attack: i32,
    pub decay: i32,
    pub sustain: i32,
    pub gate: i32,
}

impl Default for SeqSettings {
    fn default() -> Self {
        Self {
            ping_pong: false,
            time_mul: 2,
            swing: 0,
            attack: 0,
            decay: 500,
            sustain: 800,
            gate: 550,
        }
    }
}

/// The step lengths of TimeMulWheel (0x34cebc).
const TIME_MUL: [f32; 11] = [0.125, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0, 4.0];

/// The decimation threshold of a wheel value 0..1000 (5.4).
pub fn decimate_threshold(wheel: i32) -> f64 {
    let norm = wheel as f64 * (1.0 / 1000.0);
    let e = (((norm * 1.25) as f32) as f64 * LN_10001).exp();
    ((e - 1.0) as f32 as f64 / 80000.0) as f32 as f64
}

impl Envelope {
    /// The selection flag of the tools (0x23ecd0): more than one point selected.
    pub fn uses_selection(&self) -> bool {
        self.selected_count() > 1
    }

    /// The selected point indexes in order (0x23e1f0).
    pub fn selected_indices(&self) -> Vec<usize> {
        (0..self.points.len())
            .filter(|&i| self.points[i].selected())
            .collect()
    }

    /// True if at least two points are selected and they are consecutive (0x23e320).
    pub fn selection_continuous(&self) -> bool {
        let sel = self.selected_indices();
        sel.len() >= 2 && sel.windows(2).all(|w| w[1] - w[0] <= 1)
    }

    /// Replaces every `dx` by the absolute position (0x23f390).
    pub fn to_absolute(&mut self) {
        let mut x = 0.0;
        for p in &mut self.points {
            x += p.dx;
            p.dx = x;
        }
    }

    /// Converts absolute positions back to distances (0x23f460).
    pub fn to_delta(&mut self) {
        let mut prev = 0.0;
        for p in &mut self.points {
            let x = p.dx;
            p.dx -= prev;
            prev = x;
        }
    }

    /// Selects every point.
    pub fn select_all(&mut self) {
        for p in &mut self.points {
            p.set_selected(true);
        }
    }

    /// Inverts the selection.
    pub fn invert_selection(&mut self) {
        for p in &mut self.points {
            p.flags ^= 1;
        }
    }

    /// Marks redundant points and deletes them (0x23bdb0): a point is redundant if its value
    /// equals the previous one and the next one, or the previous one when both it and the next
    /// point are Hold.
    pub fn remove_redundant(&mut self) {
        let n = self.points.len();
        for i in 1..n.saturating_sub(1) {
            let near_arp = (i - 1..=i + 1).any(|k| self.points[k].arp != 0);
            if near_arp {
                continue;
            }
            let (prev, cur, next) = (self.points[i - 1], self.points[i], self.points[i + 1]);
            if prev.value == cur.value
                && cur.arp == 0
                && (next.value == cur.value
                    || (next.shape == Shape::HOLD && cur.shape == Shape::HOLD))
            {
                self.points[i].shape = 0xff;
            }
        }
        self.delete_marked();
    }

    /// Deletes the points marked with shape 0xff and gives their `dx` to the next kept point
    /// (0x23bf70).
    fn delete_marked(&mut self) {
        let mut carry = 0.0;
        let mut out = Vec::with_capacity(self.points.len());
        for p in &self.points {
            if p.shape == 0xff {
                carry += p.dx;
            } else {
                let mut q = *p;
                q.dx += carry;
                out.push(q);
                carry = 0.0;
            }
        }
        self.points = out;
        self.prepare();
    }

    /// The index of the point at absolute position `x` within `2^-24` (0x243400).
    pub fn find(&self, x: f64) -> Option<usize> {
        let mut s = 0.0;
        for (i, p) in self.points.iter().enumerate() {
            s += p.dx;
            if (s - x).abs() < EPS24 {
                return Some(i);
            }
        }
        None
    }

    /// The point at `x`, inserted with the current value there if missing (0x23ca30).
    pub fn find_or_insert(&mut self, x: f64) -> usize {
        match self.find(x) {
            Some(i) => i,
            None => {
                let v = self.value_at(x) as f32;
                self.insert_point(x, v, 0.0, true, false)
            }
        }
    }

    /// True if point `i` can be deleted: not the first or the last (TMMap virtual 19, 0x244740).
    pub fn can_delete(&self, i: usize) -> bool {
        1 <= i && i < self.hi()
    }

    /// Deletes point `i` and gives its `dx` to the next point (0x23cab0).
    pub fn delete_point(&mut self, i: usize) {
        if !self.can_delete(i) {
            return;
        }
        self.deselect_all();
        if i < self.hi() {
            let d = self.points[i].dx;
            self.points[i + 1].dx += d;
        }
        self.points.remove(i);
        self.prepare();
    }

    /// Pins the first point at 0 and the last at the length, and piles later points at the
    /// length (0xaaf40, fixed-length editing).
    pub fn fix_len_clamp(&mut self) {
        self.to_absolute();
        for p in &mut self.points {
            if self.length <= p.dx {
                p.dx = self.length;
            }
        }
        if let Some(p) = self.points.first_mut() {
            p.dx = 0.0;
        }
        let hi = self.hi();
        self.points[hi].dx = self.length;
        self.to_delta();
    }

    /// Cuts the envelope after the point at its length (0x23c1c0).
    pub fn cut_at_length(&mut self) {
        if self.length == UNBOUNDED {
            return;
        }
        let i = self.find_or_insert(self.length);
        if i < self.hi() {
            self.points.truncate(i + 1);
            self.remove_redundant();
        }
    }

    /// Appends a point with raw `dx` (AddPoint 0x23cda0).
    fn append_point(&mut self, dx: f64, value: f32, tension: f32) {
        self.deselect_all();
        self.points.push(Point::new(dx, value, tension, 0));
    }

    /// Appends a point at absolute position `x` (0x23c9b0).
    pub fn add_point(&mut self, x: f64, value: f32, tension: f32) {
        let last = if self.points.is_empty() {
            0.0
        } else {
            self.abs_pos(self.hi())
        };
        let dx = x - last;
        self.append_point(if dx <= 0.0 { 0.0 } else { dx }, value, tension);
    }

    /// Reset (TMMap.Reset 0x244930): two linear points at the default value.
    pub fn reset(&mut self) {
        self.flags &= !0xff;
        self.marker = -1;
        self.points.clear();
        let v = self.default_value() as f32;
        self.append_point(0.0, v, 0.0);
        self.append_point(self.length, v, 0.0);
    }

    /// True if the envelope is the Reset envelope (the Reset item's check mark, virtual 29).
    pub fn is_default(&self) -> bool {
        self.is_empty()
    }

    /// Flip vertically (0xbaba0 → 0x23b3d0): `y = 1 - y` for the selection set.
    pub fn flip(&mut self) {
        let sel = self.uses_selection();
        for p in &mut self.points {
            if p.selected() || !sel {
                p.value = 1.0 - p.value;
            }
        }
        self.prepare();
    }

    /// Normalize levels (0xc7cc0 → 0x23b480 with 0 and 1).
    pub fn normalize(&mut self) {
        let (lo, hi) = (0.0, 1.0);
        let sel = self.uses_selection();
        let (mut mn, mut mx) = (100.0f64, -100.0f64);
        for p in &self.points {
            if p.selected() || !sel {
                let y = p.value as f64;
                if y < mn {
                    mn = y;
                }
                if mx < y {
                    mx = y;
                }
            }
        }
        if 0.0 < mx - mn {
            for p in &mut self.points {
                if p.selected() || !sel {
                    p.value = (lo + (p.value as f64 - mn) * ((hi - lo) / (mx - mn))) as f32;
                }
            }
            self.prepare();
        }
    }

    /// Reloads the envelope from its own record, as every tool dialog does before it applies
    /// (the reload runs the length fix).
    fn reload(&mut self) {
        let copy = self.clone();
        self.assign(&copy);
    }

    /// Scale levels (TMEScaleToolForm apply 0x261960) on the selection set.
    pub fn scale_levels(&mut self, s: &ScaleSettings) {
        let sel = self.uses_selection();
        self.reload();
        let c = s.center as f64 * 0.0001220703125;
        let mut r = 1.0 - c;
        if r < c {
            r = c;
        }
        let e = ((s.tension as f32 * 0.00012207031f32).abs() as f64 * LN_1001).exp();
        let mut k = [0f32; 3];
        curve_params((e - 1.0) as f32 * s.tension.signum() as f32, &mut k);
        for p in &mut self.points {
            if sel && !p.selected() {
                continue;
            }
            let d = p.value as f64 - c;
            let df = d as f32;
            let sign = if df.abs() == 0.0 {
                0.0
            } else if df.to_bits() >> 31 != 0 {
                -1.0
            } else {
                1.0
            };
            let mut a = d.abs() * (1.0 / r);
            if k[0].to_bits() != 0 {
                a = if k[0].to_bits() & 0x8000_0000 == 0 {
                    1.0 - (((1.0 - a) * k[2] as f64).exp() - 1.0) * k[1] as f64
                } else {
                    ((a * k[2] as f64).exp() - 1.0) * k[1] as f64
                };
            }
            let mut y = sign * a * (s.multiply as f64 / ((1.0 / r) * 8192.0))
                + s.offset as f64 * 0.0001220703125
                + c;
            if 0.0 <= y {
                if 1.0 < y {
                    y = 1.0;
                }
            } else {
                y = 0.0;
            }
            p.value = y as f32;
        }
        self.prepare();
    }

    /// Decimate points (0x239320): removes redundant points, then points the line between their
    /// neighbours already gives within `thr`. Returns false if no segment is compatible
    /// (curve shapes other than linear), which the dialog reports.
    pub fn decimate(&mut self, thr: f64, sel: bool) -> bool {
        if self.points.len() < 2 {
            return false;
        }
        self.remove_redundant();
        let mut possible = false;
        self.decimate_pass(DECIMATE_EXACT, 0.0, sel, &mut possible);
        if DECIMATE_EXACT < thr {
            self.decimate_pass(thr, 200.0, sel, &mut possible);
        }
        possible
    }

    /// One decimation pass (0x2394f0).
    fn decimate_pass(&mut self, thr: f64, weight: f64, sel: bool, possible: &mut bool) {
        self.to_absolute();
        let orig = self.points.clone();
        let n = self.points.len();
        let blk = ((n as f64).sqrt().round_ties_even() as i64).max(16) as usize;
        let sub = (blk as f64).sqrt().round_ties_even() as i64 as usize;
        let mut anchor = 0;
        let mut s = 1;
        let mut end = self.hi();
        if sel {
            let idx = self.selected_indices();
            if idx.len() < 2 {
                return;
            }
            s = idx[0];
            end = *idx.last().unwrap();
        }
        while s < end {
            let limit = self.decimate_limit(s, s + blk - 1);
            if self.try_remove(&orig, s, s + blk - 1, limit, anchor, thr, weight, possible) {
                s += blk;
            } else if self.try_remove(&orig, s, s + sub - 1, limit, anchor, thr, weight, possible) {
                s += sub;
            } else {
                if !self.try_remove(&orig, s, s, limit, anchor, thr, weight, possible) {
                    anchor = s;
                }
                s += 1;
            }
        }
        self.to_delta();
        self.delete_marked();
    }

    /// The last index from `s` (at most `e` and `hi - 1`) whose points can be removed: inside the
    /// range, no arp byte, and the next segment linear (0x239b20).
    fn decimate_limit(&self, s: usize, e: usize) -> isize {
        let mut limit = (e as isize).min(self.hi() as isize - 1);
        let mut i = s as isize;
        while i <= limit {
            let p = &self.points[i as usize];
            let x = p.dx;
            let next = &self.points[i as usize + 1];
            let curve = matches!(next.shape, 0 | 1 | 7 | 8 | 9 | 11 | 12);
            if x < 0.0 || UNBOUNDED < x || p.arp != 0 || !curve || next.tension != 0.0 {
                limit = i - 1;
                break;
            }
            i += 1;
        }
        limit
    }

    /// Tries to remove points `s..=e` (0x239890): the line from the anchor to point `e + 1` must
    /// stay within `thr` of every original point between them.
    #[allow(clippy::too_many_arguments)]
    fn try_remove(
        &mut self,
        orig: &[Point],
        s: usize,
        e: usize,
        limit: isize,
        anchor: usize,
        thr: f64,
        weight: f64,
        possible: &mut bool,
    ) -> bool {
        if e as isize > limit {
            return false;
        }
        *possible = true;
        let pts = &self.points;
        let dx = pts[e + 1].dx - pts[anchor].dx;
        if 0.0 < dx {
            let y0 = pts[anchor].value as f64;
            let slope = (pts[e + 1].value as f64 - y0) / dx;
            for i in anchor + 1..=e {
                let d = ((y0 + slope * (pts[i].dx - pts[anchor].dx)) - orig[i].value as f64).abs();
                if !(d - thr).is_sign_negative() {
                    return false;
                }
                if 0.0 < weight
                    && !((d * (orig[i + 1].dx - orig[i - 1].dx)) * weight - thr).is_sign_negative()
                {
                    return false;
                }
            }
        }
        for p in &mut self.points[s..=e] {
            p.shape = 0xff;
        }
        true
    }

    /// Decimate points (DecimateMenuClick 0xc07a0 with the dialog's amount).
    pub fn decimate_tool(&mut self, amount: i32) -> Result<(), ToolError> {
        let sel = self.uses_selection();
        if sel && !self.selection_continuous() {
            return Err(not_continuous("decimate"));
        }
        let mut probe = self.clone();
        probe.reload();
        if !probe.decimate(decimate_threshold(amount), sel) {
            return Err(refuse(
                "Can't decimate",
                "The envelope contains points with curve modes that aren't compatible with decimation.",
            ));
        }
        *self = probe;
        self.fix_len_clamp();
        self.prepare();
        Ok(())
    }

    /// Turn all points smooth (0xc8190): shape Smooth and tension 0 for the selection range.
    pub fn turn_smooth(&mut self) -> Result<(), ToolError> {
        let sel = self.uses_selection();
        if sel && !self.selection_continuous() {
            return Err(not_continuous("smoothing"));
        }
        let (a, b) = if sel {
            let idx = self.selected_indices();
            (idx[0], *idx.last().unwrap())
        } else {
            (0, self.hi())
        };
        for p in &mut self.points[a..=b] {
            p.shape = Shape::SMOOTH;
            p.tension = 0.0;
        }
        self.prepare();
        Ok(())
    }

    /// Prepare for smooth editing (0xabbc0): a Reset envelope with a Smooth point at every grid
    /// step that carries the current value. `grid` is the editor's beat grid (+0x1590), `coarse`
    /// its fallback (+0x1598). The result goes through a record save and load, like the original.
    pub fn prepare_smooth_editing(&mut self, grid: f64, coarse: f64) {
        let mut new = Envelope::with_points(self.length, Vec::new());
        new.reset();
        let mut g = grid;
        if new.length * 0.25 < g {
            g = coarse;
        }
        if new.length * 0.000244140625 < g {
            let mut x = g;
            while x < new.length {
                let v = self.value_at(x) as f32;
                new.insert_point(x, v, 0.0, true, false);
                x += g;
            }
            for p in &mut new.points[1..] {
                p.shape = Shape::SMOOTH;
            }
        }
        new.prepare();
        self.assign(&new);
    }

    /// Copies `other` into this envelope through a record save and load (0x2391f0).
    pub fn assign(&mut self, other: &Envelope) {
        let mut rec = Vec::new();
        other.save(&mut rec);
        self.load(&mut crate::Reader::new(&rec));
    }

    /// 1 semitone up or down on the clicked point (PtPitch0MenuClick 0x657e0): the incoming
    /// slope `s` becomes `(1 + 8s) * 2^(±1/12) - 1`, read as a playback rate on the time lane.
    pub fn semitone(&mut self, i: usize, up: bool) {
        let Some(p) = self.points.get(i) else { return };
        if p.dx.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return;
        }
        let tag: f64 = if up { 1.0 } else { -1.0 };
        let y0 = self.points[i.saturating_sub(1)].value as f64;
        let y1 = p.value as f64;
        let g = 8.0 / p.dx;
        let ratio = ((tag / 12.0) * std::f64::consts::LN_2).exp();
        let y = y0 + (((y1 - y0) * g + 1.0) * ratio - 1.0) / g;
        let p = &mut self.points[i];
        p.value = y.clamp(0.0, 1.0) as f32;
        p.shape = 0;
        p.tension = 0.0;
        self.prepare();
    }

    /// Smooth up... (TMESmoothToolForm apply 0xce870). Refuses a selection that is not continuous.
    pub fn smooth_up(&mut self, s: &SmoothSettings) -> Result<(), ToolError> {
        let sel = self.uses_selection();
        if sel && !self.selection_continuous() {
            return Err(not_continuous("smoothing"));
        }
        let a = (s.attack as f32 * 0.001f32) as f64;
        let r = (s.release as f32 * 0.001f32) as f64;
        let att_slope = (s.attack_slope as f32 * 0.001f32) as f64;
        let rel_slope = (s.release_slope as f32 * 0.001f32) as f64;
        let ofs = s.offset as f64 * 0.25;
        let thr = decimate_threshold(s.decimate);
        self.reload();
        self.smooth_core(
            a * a * 1000.0,
            att_slope,
            r * r * 1000.0,
            rel_slope,
            ofs,
            thr,
            None,
            sel,
        );
        self.fix_len_clamp();
        self.prepare();
        Ok(())
    }

    /// Filter... (TMEEnvFilterToolForm apply 0xcdf50) with a kernel envelope of length 1.
    pub fn filter(&mut self, s: &FilterSettings, kernel_env: &Envelope) -> Result<(), ToolError> {
        let sel = self.uses_selection();
        if sel && !self.selection_continuous() {
            return Err(not_continuous("filter"));
        }
        let w = (s.width as f32 * 0.001f32) as f64;
        let mut width = w * w * 1000.0;
        if width <= 3.0 {
            width = 3.0;
        }
        let klen = (width.trunc() as usize + (width.fract() > 0.0) as usize) | 1;
        let mut kernel = vec![0f32; klen];
        render_kernel(
            ((klen as f64 - width) + 1.0) * 0.5,
            width,
            kernel_env,
            &mut kernel,
        );
        let ofs = (s.offset as f64 / 2000.0) * klen as f64;
        self.reload();
        self.smooth_core(
            0.0,
            0.0,
            0.0,
            0.0,
            ofs,
            decimate_threshold(s.decimate),
            Some(&mut kernel),
            sel,
        );
        self.fix_len_clamp();
        self.prepare();
        Ok(())
    }

    /// The Smooth up / Filter core (0x239c90): rasterize at 0.001 beat, filter, turn every sample
    /// into a linear point, and decimate.
    #[allow(clippy::too_many_arguments)]
    pub fn smooth_core(
        &mut self,
        mut att: f64,
        att_slope: f64,
        mut rel: f64,
        rel_slope: f64,
        ofs: f64,
        thr: f64,
        kernel: Option<&mut Vec<f32>>,
        sel: bool,
    ) {
        if self.points.len() < 2 {
            return;
        }
        // A. Selection: the selected run, rebased to x = 0.
        let (mut orig, mut orig_abs, mut sel_idx, mut sel_end_x) =
            (Vec::new(), Vec::new(), Vec::new(), -1.0);
        if sel {
            orig = self.points.clone();
            self.to_absolute();
            orig_abs = self.points.clone();
            self.to_delta();
            sel_idx = self.selected_indices();
            if sel_idx.len() < 2 {
                return;
            }
            let last = *sel_idx.last().unwrap();
            if last < self.hi() {
                sel_end_x = self.abs_pos(last);
            }
            self.points.truncate(sel_idx.len());
            self.deselect_all();
            for (j, &i) in sel_idx.iter().enumerate() {
                let p = &mut self.points[j];
                p.dx = orig[i].dx;
                p.value = orig[i].value;
                p.tension = orig[i].tension;
                p.shape = orig[i].shape;
                p.flags |= 1;
            }
            self.points[0].dx = 0.0;
            self.prepare();
        }
        // B. Raster.
        self.remove_redundant();
        let h0 = 1.0 * F32_0001;
        let mut st = Cursor::new(self);
        let len = self.abs_pos(self.hi());
        let n = rne(len / h0) as i32;
        if n < 1 {
            return;
        }
        let h = len / n as f64;
        let mut buf: Vec<f32> = Vec::with_capacity(n as usize + 1);
        for _ in 0..=n {
            buf.push(st.out as f32);
            st.step_once(self, h);
        }
        // C. Edge padding.
        if let Some(k) = &kernel {
            att = k.len() as f64;
            rel = att;
        }
        let shift = rne(ofs);
        let pre = rne(att).max(shift.abs()) as usize;
        let post = rne(rel).max(shift.abs()) as usize;
        let first = buf[0];
        let tail = if self.length == UNBOUNDED {
            0.0
        } else {
            *buf.last().unwrap()
        };
        let mut padded = Vec::with_capacity(pre + buf.len() + post);
        padded.extend(std::iter::repeat_n(first, pre));
        padded.extend_from_slice(&buf);
        padded.extend(std::iter::repeat_n(tail, post));
        let mut buf = padded;
        let mut cut = pre;
        // D. Filter.
        match kernel {
            None => {
                let (ca, cr) = follower_coefs(att * att_slope, rel * rel_slope);
                follow(&mut buf, ca, cr);
                buf.reverse();
                let (ca, cr) = follower_coefs(rel * (1.0 - rel_slope), att * (1.0 - att_slope));
                follow(&mut buf, ca, cr);
                buf.reverse();
            }
            Some(k) => {
                let mut sum = exact_sum(k) as f32;
                if sum == 0.0 {
                    k.fill(1.0);
                    sum = k.len() as f32;
                }
                let inv = 1.0f32 / sum;
                let (m, kl) = (buf.len(), k.len());
                let mut out = Vec::with_capacity(m + kl - 1);
                for j in 0..m + kl - 1 {
                    let mut acc = 0f64;
                    for q in j.saturating_sub(m - 1)..kl.min(j + 1) {
                        acc += buf[j - q] as f64 * k[q] as f64;
                    }
                    out.push(acc as f32 * inv);
                }
                buf = out;
                cut += kl / 2;
            }
        }
        buf.drain(..(cut as i64 - shift) as usize);
        // E. One linear point per remaining sample.
        let new: Vec<Point> = buf
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let mut p = Point::new(if i == 0 { 0.0 } else { h }, v, 0.0, 0);
                p.flags = sel as u8;
                p
            })
            .collect();
        if !sel {
            // F. Whole envelope.
            self.points = new;
            self.prepare();
            self.cut_at_length();
            self.prepare();
            self.decimate(thr, false);
        } else {
            // G. Selection: points before the run, the new points, points after the run.
            self.points.clear();
            let (s0, last) = (sel_idx[0], *sel_idx.last().unwrap());
            let mut carry = 0.0;
            for (i, o) in orig.iter().enumerate().take(s0) {
                let mut p =
                    Point::new(if i == 0 { 0.0 } else { o.dx }, o.value, o.tension, o.shape);
                p.arp = o.arp;
                self.points.push(p);
                carry = p.dx;
            }
            for (j, q) in new.iter().enumerate() {
                let mut p = *q;
                if j == 0 {
                    p.dx = carry;
                }
                p.flags = 1;
                self.points.push(p);
                if 0.0 <= sel_end_x && sel_end_x <= self.abs_pos(self.hi()) {
                    break;
                }
            }
            if last < orig.len() - 1 {
                self.to_absolute();
                for i in last + 1..orig.len() {
                    let mut p = Point::new(
                        orig_abs[i].dx,
                        orig_abs[i].value,
                        orig[i].tension,
                        orig[i].shape,
                    );
                    p.arp = orig[i].arp;
                    self.points.push(p);
                }
                self.to_delta();
            }
            self.prepare();
            self.cut_at_length();
            self.decimate(thr, true);
            self.points[0].dx = 0.0;
        }
    }

    /// Create sequence (0xccd60): one attack, decay and release group per step, stretched to
    /// the length.
    pub fn create_sequence(&mut self, steps: &[SeqStep; 32], s: &SeqSettings) {
        let mut count = 32;
        while count > 0 && steps[count - 1].kind == 0 {
            count -= 1;
        }
        self.reset();
        self.points.clear();
        self.flags |= 3;
        if count < 1 {
            self.reset();
            self.prepare();
            return;
        }
        let step_len = TIME_MUL[s.time_mul.clamp(0, 10) as usize] as f64;
        let swing = (step_len * s.swing as f64) * 0.00075;
        let att = (s.attack as f32 * 0.001f32) as f64;
        let dec = (s.decay as f32 * 0.001f32) as f64;
        let sus = (s.sustain as f32 * 0.001f32) as f64;
        let gate = (s.gate as f32 * 0.001f32) as f64;
        let last_it = if s.ping_pong { 2 * count } else { count };
        let (mut t, mut tied) = (0.0f64, false);
        for it in 0..=last_it {
            let si = if it < count {
                it as i64
            } else {
                2 * count as i64 - it as i64 - 1
            };
            let dur = if it % 2 == 0 {
                step_len + swing
            } else {
                step_len - swing
            };
            let st = &steps[si.clamp(0, 31) as usize];
            let lvl = if st.kind == 0 {
                0.0
            } else {
                st.level as f64 * 1.52587890625e-05
            };
            let mut d17;
            if it < last_it && tied {
                d17 = 0.0;
            } else {
                self.add_point(t, 0.0, 0.0);
                if it >= last_it {
                    break;
                }
                d17 = dur * att * lvl;
                self.add_point(t + d17, lvl as f32, 0.0);
            }
            tied = st.kind > 2;
            if !tied {
                d17 += (dur - d17) * dec;
                let mut sl = (lvl * sus) * (st.sustain as f64 * 3.0517578125e-05);
                if sl >= 1.0 {
                    sl = 1.0;
                }
                self.add_point(t + d17, sl as f32, seq_tension(st.decay_slope));
                let mut r = dur - d17;
                if st.kind < 2 {
                    r *= gate;
                }
                self.add_point(t + (d17 + r), 0.0, seq_tension(st.release_slope));
            }
            t += dur;
        }
        self.remove_redundant();
        self.fix_length();
        self.prepare();
    }

    /// Analyze audio file (core 0x23ad50): an envelope follower of the decoded file. `frames`
    /// holds mono or stereo samples per frame; `tempo` is the file tempo or 0.
    pub fn analyze(&mut self, frames: &[[f32; 2]], channels: usize, sr: f64, tempo: f64) {
        if frames.is_empty() {
            return;
        }
        let ms = frames.len() as f64 * 1000.0 / sr;
        let (att_ms, rel_ms, mode) = if ms >= 50.0 {
            (5.0, 150.0, 0)
        } else {
            (0.0, 0.0, 3)
        };
        let mut tmp = self.clone();
        tmp.reset();
        tmp.points.clear();
        let n = frames.len().min(0x40_0000);
        let mut dt = 2.0 / sr;
        if tempo > 0.0 {
            tmp.flags |= 3;
            dt = tempo / (sr * 60.0);
        }
        let rel = rel_ms * sr * 0.001;
        let att = att_ms * sr * 0.001;
        let (ca, cr) = follower_coefs(att * 0.5773502691896258, rel * 0.5773502691896258);
        let mut y = [0f64; 3];
        let mut mx = 0f64;
        for (i, f) in frames[..n].iter().enumerate() {
            let mut x = f[0];
            if channels > 1 {
                x = (x + f[1]) / channels as f32;
            }
            x = if mode & 2 == 0 {
                x.abs()
            } else {
                (x + 1.0) * 0.5
            };
            if i == 0 {
                y = [x as f64; 3];
            } else {
                for ys in &mut y {
                    let xd = x as f64;
                    *ys = if xd <= *ys {
                        xd + (*ys - xd) * cr
                    } else {
                        xd + (*ys - xd) * ca
                    };
                    x = *ys as f32;
                }
            }
            tmp.points.push(Point::new(dt, x, 0.0, 0));
            let fm = mx as f32;
            mx = (if fm <= x { x } else { fm }) as f64;
        }
        if 0.0 < mx && (mode & 1 == 0 || 1.0 < mx) {
            let s = (1.0 / mx) as f32;
            for p in &mut tmp.points {
                p.value *= s;
            }
        }
        tmp.decimate(0.001953125, false);
        tmp.fix_length();
        tmp.prepare();
        self.assign(&tmp);
    }

    /// Cut (`cut = true`) or Copy the selected points into the clipboard (0x23e430).
    pub fn copy_points(&mut self, cut: bool) {
        let sel = self.selected_indices();
        if sel.is_empty() {
            return;
        }
        let continuous = sel.windows(2).all(|w| w[1] - w[0] <= 1);
        if cut && (sel.len() as i64 > self.points.len() as i64 - 2 || !continuous) {
            return;
        }
        self.to_absolute();
        let base = self.points[sel[0]].dx;
        self.clip = sel
            .iter()
            .map(|&i| {
                let mut p = self.points[i];
                p.dx -= base;
                p
            })
            .collect();
        self.to_delta();
        if cut {
            self.deselect_all();
            for &i in sel.iter().rev() {
                self.delete_point(i);
            }
            if sel[0] > 0 && sel[0] < self.points.len() {
                self.points[sel[0]].dx = self.points[sel[0] - 1].dx;
            }
            self.points[0].dx = 0.0;
        }
        self.prepare();
    }

    /// Paste (insert) or Paste (replace) from the clipboard (0x23e6f0).
    pub fn paste_points(&mut self, replace: bool) -> Result<(), ToolError> {
        if self.clip.is_empty() {
            return Err(refuse(
                "Paste",
                "There are currently no points in the clipboard to paste.",
            ));
        }
        let sel = self.selected_indices();
        if replace && !self.selection_continuous() {
            return Err(refuse(
                "Paste",
                "Cannot paste and replace on a non-continuous selection.",
            ));
        }
        if sel.is_empty() {
            return Ok(());
        }
        let sel_last = *sel.last().unwrap();
        self.to_absolute();
        let a = self.points.clone();
        let span = self.clip.last().unwrap().dx;
        self.deselect_all();
        self.points.clear();
        let (keep, shift, base, anchor_v) = if !replace {
            (sel_last + 1, 0.0, a[sel_last].dx, a[sel_last].value)
        } else {
            (sel[0], a[sel_last].dx - a[sel[0]].dx, a[sel[0]].dx, -1.0f32)
        };
        self.points.extend_from_slice(&a[..keep]);
        for (k, c) in self.clip.iter().enumerate() {
            if k == 0 && c.dx == 0.0 && c.value == anchor_v {
                continue;
            }
            let mut p = *c;
            p.dx = c.dx + base;
            p.flags |= 1;
            self.points.push(p);
        }
        let last = *self.points.last().unwrap();
        for (i, q) in a.iter().enumerate().skip(sel_last + 1) {
            if i == sel_last + 1 && q.dx + span == last.dx && q.value == last.value {
                continue;
            }
            let mut p = *q;
            p.dx = (q.dx + span) - shift;
            p.flags &= !1;
            self.points.push(p);
        }
        self.to_delta();
        self.prepare();
        self.fix_len_clamp();
        Ok(())
    }

    /// Duplicate the selection after itself (0xab120).
    pub fn duplicate(&mut self) -> Result<(), ToolError> {
        let sel = self.selected_indices();
        if sel.len() < 2 {
            return Err(refuse(
                "Duplicate",
                "You must select at least 2 points to duplicate a selection.",
            ));
        }
        self.deselect_all();
        let sel_last = *sel.last().unwrap();
        let a = self.abs_pos(sel[0]);
        let b = self.abs_pos(sel_last);
        let v_last = self.points[sel_last].value;
        if sel_last < self.hi() {
            self.points[sel_last + 1].dx += b - a;
        }
        for (k, &i) in sel.iter().enumerate() {
            let x = self.abs_pos(i);
            if k == 0 && x - a == 0.0 && self.points[sel[0]].value == v_last {
                self.points[sel_last].flags |= 1;
            } else {
                let (v, t, shape) = (
                    self.points[i].value,
                    self.points[i].tension,
                    self.points[i].shape,
                );
                let j = self.insert_point(b + (x - a), v, t, false, false);
                self.points[j].shape = shape;
                self.points[j].flags |= 1;
            }
        }
        self.remove_redundant();
        self.fix_len_clamp();
        self.prepare();
        Ok(())
    }

    /// Delete (point menu 0xb6c10, also Delete selection 0xab8d0): deletes the selected points;
    /// the first and the last point stay.
    pub fn delete_selected(&mut self) -> Result<(), ToolError> {
        let min = 2;
        if self.points.len() <= min {
            return Err(refuse(
                "Delete",
                format!("The automation envelope must have at least {min} point(s)."),
            ));
        }
        let sel = self.selected_indices();
        if sel == [0] {
            return Err(refuse(
                "Delete",
                "The first point of the envelope can't be removed.",
            ));
        }
        for &i in sel.iter().rev() {
            if !self.points.is_empty() {
                self.delete_point(i);
            }
        }
        Ok(())
    }

    /// Paste value (0xb6f60): every selected point gets the value.
    pub fn set_selected_values(&mut self, v: f64) {
        for p in &mut self.points {
            if p.selected() {
                p.value = (v as f32).clamp(0.0, 1.0);
            }
        }
        self.prepare();
    }

    /// The "Copy state" clipboard payload: `FLST`, 1, `.ENV`, then the record.
    pub fn state_clipboard(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0x5453_4c46u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0x564e_452eu32.to_le_bytes());
        self.save(&mut out);
        out
    }

    /// True if `data` is a "Copy state" payload this envelope accepts (the Paste state check).
    pub fn accepts_state_clipboard(data: &[u8]) -> bool {
        data.len() >= 20
            && data[..4] == 0x5453_4c46u32.to_le_bytes()
            && data[4..8] == 1u32.to_le_bytes()
            && data[8..12] == 0x564e_452eu32.to_le_bytes()
            && i32::from_le_bytes(data[12..16].try_into().unwrap()) == crate::KIND_MAP
            && i32::from_le_bytes(data[16..20].try_into().unwrap()) == crate::RECORD_VERSION
    }

    /// Paste state (0xb9b50).
    pub fn paste_state_clipboard(&mut self, data: &[u8]) -> bool {
        if !Self::accepts_state_clipboard(data) {
            return false;
        }
        self.load(&mut crate::Reader::new(&data[12..]));
        true
    }
}

/// The Create sequence tension of a slope value: `sign(f) (21^|f| - 1) 0.05` with
/// `f = 1 - raw / 32768`.
pub fn seq_tension(raw: i32) -> f32 {
    let f = (1.0 - raw as f64 * 3.0517578125e-05) as f32;
    let sgn = if f.abs() == 0.0 {
        0.0f32
    } else if f < 0.0 {
        -1.0
    } else {
        1.0
    };
    let e = ((f.abs() as f64 * LN_21).exp() - 1.0) as f32;
    ((e * sgn) as f64 * 0.05) as f32
}

/// The follower coefficients (0x1dac40): close 99 % of a step in `a` (rising) or `r` samples.
fn follower_coefs(a: f64, r: f64) -> (f64, f64) {
    let c = |x: f64| {
        if 0.0 < x {
            (LN_001 / if x <= 1.0 { 1.0 } else { x }).exp()
        } else {
            0.0
        }
    };
    (c(a), c(r))
}

/// The follower pass (0x1dae40) with the state starting at the first sample.
fn follow(buf: &mut [f32], ca: f64, cr: f64) {
    let Some(&first) = buf.first() else { return };
    let mut y = first as f64;
    for v in buf {
        let x = *v as f64;
        y = if x <= y {
            x + (y - x) * cr
        } else {
            x + (y - x) * ca
        };
        *v = y as f32;
    }
}

/// Samples the kernel envelope (0x2432f0): sample `i` reads `x = (i + 1 - start) * L / W`.
pub fn render_kernel(start: f64, width: f64, env: &Envelope, data: &mut [f32]) {
    let dx = env.length / width;
    let mut st = Cursor::new(env);
    let i0 = (start as i64).min(data.len() as i64).max(0) as usize;
    for v in &mut data[..i0] {
        *v = st.out as f32;
    }
    st.step_once(env, (1.0 - (start - i0 as f64)) * dx);
    for v in &mut data[i0..] {
        *v = st.out as f32;
        st.step_once(env, dx);
    }
}

/// The exactly rounded sum of f32 values (the original's ippsSum_32f with the accurate hint is
/// modeled as an exact sum rounded once).
fn exact_sum(v: &[f32]) -> f64 {
    // Shewchuk's partials, as Python's math.fsum.
    let mut partials: Vec<f64> = Vec::new();
    for &x in v {
        let mut x = x as f64;
        let mut i = 0;
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let lo = y - (hi - x);
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        partials.push(x);
    }
    partials.iter().sum()
}

/// Parses a typed or pasted value (0x1ceba0): `"0.5"`, `"50"`, `"50%"` or `"1/2"`. With `pct`, a
/// plain number above 1 is a percentage.
pub fn parse_value(s: &str, pct: bool) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s.len() > 63 {
        return None;
    }
    let num = |t: &str| t.trim().parse::<f64>().ok();
    let v = if let Some((a, b)) = s.split_once('/') {
        let n = num(a)?;
        match num(b) {
            Some(d) if d != 0.0 => n / d,
            _ => n,
        }
    } else {
        let (t, sign) = match s.strip_suffix('%') {
            Some(t) => (t, true),
            None => (s, false),
        };
        let v = num(t)?;
        if sign || (v > 1.0 && pct) {
            v / 100.0
        } else {
            v
        }
    };
    Some(v)
}

/// The text of Copy value (`FloatToStrF(v, ffGeneral, 15, 0)`).
pub fn value_text(v: f32) -> String {
    let v = v as f64;
    let s = format!("{v:.15}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    // ffGeneral with 15 digits: at most 15 significant digits.
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
    if digits <= 15 {
        return s.to_string();
    }
    let mut out = format!("{:.*e}", 14, v);
    if let Some((m, e)) = out.split_once('e') {
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        if (-5..15).contains(&e) {
            let p = (14 - e).max(0) as usize;
            let t = format!("{v:.p$}");
            out = t.trim_end_matches('0').trim_end_matches('.').to_string();
        } else {
            out = format!("{m}E{e}");
        }
    }
    out
}
