//! The Gross Beat envelope model (`TCustomMEnvelope` / `TMMap` / `TBRSlot`).
//!
//! An [`Envelope`] is a list of [`Point`]s. Each point stores its distance `dx` from the previous
//! point, so the absolute position of point `i` is the sum of `dx[0..=i]`. A point describes the
//! segment that ends at it: its shape, tension and derived coefficients. Slot envelopes are 4 beats
//! long and loop.
//!
//! Every computation follows `.amp/in/re/notes/envelope.md` bit for bit: the same float types,
//! the same rounding points and the same operation order.

mod record;
mod render;
mod shape;
pub mod tools;

pub use record::{Error, RECORD_VERSION, Reader, read_fnv, write_fnv};
pub use render::Cursor;
pub use shape::{SHAPE_MENU, Shape, curve_params, single_curve, step_count};

/// The record kind of `TMMap` (and `TBRSlot`), written first in every envelope record.
pub const KIND_MAP: i32 = 3;

/// The length of a slot envelope in beats (env+0x68, set by the `TBRSlot` constructor).
pub const SLOT_LENGTH: f64 = 4.0;

/// `2^-24`, the "small value" constant at 0x34cb24.
pub const EPS24: f64 = 5.960464477539063e-08;

/// One envelope point (`TMEnvPoint`, 36 bytes in the original).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Distance from the previous point, in envelope units (beats for slots). Point 0 uses it as
    /// its absolute position.
    pub dx: f64,
    /// Value, 0..=1 for slots.
    pub value: f32,
    /// Tension, -1..=1.
    pub tension: f32,
    /// Shape id of the segment that ends at this point (see [`Shape`]).
    pub shape: u8,
    /// Arpeggiator break byte (+0x11). Gross Beat clears it on load.
    pub arp: u8,
    /// Flags (+0x12). Bit 0 = selected in the editor.
    pub flags: u8,
    /// Byte +0x13: the sign of the tension, or 2 for "evaluate linear". `prepare` derives it for
    /// points 1.., point 0 keeps the loaded byte. It is saved as is.
    pub sign: i8,
    /// Derived shape coefficients (+0x14, +0x18, +0x1c).
    c: [f32; 3],
}

impl Point {
    /// Creates a point with zeroed shape, flag bytes and coefficients.
    pub fn new(dx: f64, value: f32, tension: f32, shape: u8) -> Self {
        Self {
            dx,
            value,
            tension,
            shape,
            arp: 0,
            flags: 0,
            sign: 0,
            c: [0.0; 3],
        }
    }

    /// Returns true if the point is selected in the editor.
    pub fn selected(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Selects or deselects the point.
    pub fn set_selected(&mut self, on: bool) {
        self.flags = (self.flags & !1) | on as u8;
    }
}

/// A `TMMap` envelope as Gross Beat uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    /// The points. Never empty after a load or an edit.
    pub points: Vec<Point>,
    /// Envelope flags (+0x44, saved).
    pub flags: u32,
    /// Extra marker (+0x48, saved, -1 in practice).
    pub marker: i32,
    /// Length in envelope units (+0x68). 4 beats for slots, 1 for filter kernels.
    pub length: f64,
    /// The point clipboard of this envelope (+0x30; Cut/Copy/Paste, not saved).
    pub clip: Vec<Point>,
}

impl Envelope {
    /// The constructor envelope of a slot: (0, 1.0) and (+4, 1.0), Hold, tension 0.
    pub fn slot() -> Self {
        Self::with_points(
            SLOT_LENGTH,
            vec![
                Point::new(0.0, 1.0, 0.0, Shape::HOLD),
                Point::new(SLOT_LENGTH, 1.0, 0.0, Shape::HOLD),
            ],
        )
    }

    /// Creates an envelope from points and prepares it.
    pub fn with_points(length: f64, points: Vec<Point>) -> Self {
        let mut env = Self {
            points,
            flags: 0,
            marker: -1,
            length,
            clip: Vec::new(),
        };
        env.prepare();
        env
    }

    /// The default value of a `TMMap` (+0xb0 = `1 - offset`, the offset is 0).
    pub const fn default_value(&self) -> f64 {
        1.0
    }

    /// The value of point `i` as the evaluator reads it (0x23ecf0): the f32 value widened, minus
    /// the value offset (0), or 0 if the index does not exist.
    pub fn value(&self, i: usize) -> f64 {
        self.points.get(i).map_or(0.0, |p| p.value as f64)
    }

    /// The index of the last point.
    pub fn hi(&self) -> usize {
        self.points.len().saturating_sub(1)
    }

    /// The absolute position of point `i` (0x23edf0): the sum of `dx[0..=i]`.
    pub fn abs_pos(&self, i: usize) -> f64 {
        let mut x = 0.0;
        for p in &self.points[..=i.min(self.hi())] {
            x += p.dx;
        }
        x
    }

    /// Returns true if the envelope is "empty" (TMMap.v29 0x244ba0): every point has the same
    /// value and that value is the default level 1.0.
    pub fn is_empty(&self) -> bool {
        let Some(first) = self.points.first() else {
            return false;
        };
        self.points.iter().all(|p| p.value == first.value)
            && first.value as f64 == self.default_value()
    }

    /// Returns the number of selected points.
    pub fn selected_count(&self) -> usize {
        self.points.iter().filter(|p| p.selected()).count()
    }

    /// Deselects every point.
    pub fn deselect_all(&mut self) {
        for p in &mut self.points {
            p.set_selected(false);
        }
    }

    /// Inserts a point at absolute position `x` (0x23c6b0) and returns its index.
    ///
    /// `x` is clamped to `[0, length]`. The point goes before the first point `i >= 1` whose
    /// absolute position is past `x` (compared in 1/1000 units when `quantized`), or at the end.
    /// Its `dx` is `max(f32(x - abs_pos(i - 1)), 0)`, and the next point's `dx` shrinks by the same
    /// amount. The new point has zero shape and flag bytes; like the original, it keeps the derived
    /// bytes of the point it was shifted over. Then the envelope is prepared.
    pub fn insert_point(
        &mut self,
        x: f64,
        value: f32,
        tension: f32,
        deselect: bool,
        quantized: bool,
    ) -> usize {
        if deselect {
            self.deselect_all();
        }
        let x = if 0.0 <= x {
            if self.length < x { self.length } else { x }
        } else {
            0.0
        };
        let n = self.points.len();
        let (mut idx, mut s) = (n, 0.0);
        if n >= 1 {
            s = self.points[0].dx;
            let mut prev = s;
            for i in 1..n {
                let next = prev + self.points[i].dx;
                let hit = if quantized {
                    rne(x * 1000.0) <= rne(next * 1000.0)
                } else {
                    x < next
                };
                if hit {
                    idx = i;
                    s = prev;
                    break;
                }
                prev = next;
                s = next;
            }
        }
        let mut p = self
            .points
            .get(idx)
            .copied()
            .unwrap_or(Point::new(0.0, 0.0, 0.0, 0));
        p.shape = 0;
        p.arp = 0;
        p.flags = 0;
        let dx = (x - s) as f32;
        p.dx = if dx <= 0.0 { 0.0 } else { dx as f64 };
        p.value = value;
        p.tension = tension;
        self.points.insert(idx, p);
        if idx < self.hi() {
            self.points[idx + 1].dx -= self.points[idx].dx;
        }
        self.prepare();
        idx
    }
}

/// FPC `Round` (`frintx` + `fcvtzs`): round half to even, then convert.
pub fn rne(x: f64) -> i64 {
    x.round_ties_even() as i64
}

impl Default for Envelope {
    fn default() -> Self {
        Self::slot()
    }
}
