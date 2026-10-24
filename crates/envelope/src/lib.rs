//! Looping multi-point envelopes.
//!
//! An [`Envelope`] is a list of points sorted by `x` (beats). The shape and tension of a point
//! describe the segment that ends at that point. The envelope loops every `length` beats.

mod shape;
mod tools;

pub use shape::{count, Shape};
pub use tools::LfoShape;

use serde::{Deserialize, Serialize};

/// The default envelope length, in beats (one 4/4 bar).
pub const DEFAULT_LENGTH: f64 = 4.0;

/// One envelope point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Position in beats, 0..=length.
    pub x: f64,
    /// Value, 0..=1.
    pub y: f64,
    /// Shape of the segment that ends at this point.
    pub shape: Shape,
    /// Tension of the segment that ends at this point, -1..=1.
    pub tension: f64,
}

impl Point {
    /// Creates a point with the default shape and zero tension.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y, shape: Shape::default(), tension: 0.0 }
    }

    /// Returns the point with a different shape.
    pub fn with_shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    /// Returns the point with a different tension.
    pub fn with_tension(mut self, tension: f64) -> Self {
        self.tension = tension;
        self
    }
}

/// A looping multi-point envelope.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Loop length in beats.
    pub length: f64,
    /// Points, sorted by `x`. Never empty.
    pub points: Vec<Point>,
}

impl Default for Envelope {
    fn default() -> Self {
        Self::flat(1.0)
    }
}

impl Envelope {
    /// Creates a flat envelope at `y`.
    pub fn flat(y: f64) -> Self {
        Self::from_points(vec![Point::new(0.0, y), Point::new(DEFAULT_LENGTH, y)])
    }

    /// Creates an envelope from points. Sorts and clamps them.
    pub fn from_points(points: Vec<Point>) -> Self {
        let mut env = Self { length: DEFAULT_LENGTH, points };
        env.normalize();
        env
    }

    /// Sorts the points, clamps coordinates, and makes sure at least one point exists.
    pub fn normalize(&mut self) {
        if self.points.is_empty() {
            self.points.push(Point::new(0.0, 1.0));
        }
        for p in &mut self.points {
            p.x = p.x.clamp(0.0, self.length);
            p.y = p.y.clamp(0.0, 1.0);
            p.tension = p.tension.clamp(-1.0, 1.0);
        }
        self.points.sort_by(|a, b| a.x.total_cmp(&b.x));
    }

    /// Returns the value at `x` beats. `x` wraps around the loop length.
    pub fn value_at(&self, x: f64) -> f64 {
        let x = x.rem_euclid(self.length);
        let i = self.points.partition_point(|p| p.x <= x);
        self.segment_value(i, x)
    }

    /// Returns the value at `x` (already wrapped) where `i` is the index of the first point with
    /// `point.x > x`.
    fn segment_value(&self, i: usize, x: f64) -> f64 {
        let points = &self.points;
        if i == 0 {
            return points[0].y;
        }
        if i >= points.len() {
            return points[points.len() - 1].y;
        }
        let a = points[i - 1];
        let b = points[i];
        let w = b.x - a.x;
        if w <= 0.0 {
            return b.y;
        }
        let t = (x - a.x) / w;
        a.y + (b.y - a.y) * b.shape.eval(t, b.tension)
    }

    /// Fills `out` with values starting at `x0`, advancing `dx` beats per sample.
    pub fn render(&self, x0: f64, dx: f64, out: &mut [f32]) {
        let mut cursor = Cursor::default();
        for (n, o) in out.iter_mut().enumerate() {
            *o = cursor.value(self, x0 + dx * n as f64) as f32;
        }
    }

    /// Adds a point and returns its index.
    pub fn add_point(&mut self, point: Point) -> usize {
        let mut point = point;
        point.x = point.x.clamp(0.0, self.length);
        point.y = point.y.clamp(0.0, 1.0);
        let i = self.points.partition_point(|p| p.x <= point.x);
        self.points.insert(i, point);
        i
    }

    /// Removes a point. Keeps at least one point.
    pub fn remove_point(&mut self, i: usize) {
        if self.points.len() > 1 && i < self.points.len() {
            self.points.remove(i);
        }
    }

    /// Moves a point, keeping it between its neighbors. With `slide`, every following point moves
    /// by the same `x` delta.
    pub fn move_point(&mut self, i: usize, x: f64, y: f64, slide: bool) {
        let Some(p) = self.points.get(i).copied() else { return };
        let lo = if i == 0 { 0.0 } else { self.points[i - 1].x };
        let hi = if slide || i + 1 >= self.points.len() {
            self.length
        } else {
            self.points[i + 1].x
        };
        let mut nx = x.clamp(lo, hi);
        if slide {
            let last = self.points.last().map_or(p.x, |l| l.x);
            let max_dx = self.length - last;
            nx = nx.min(p.x + max_dx);
            let dx = nx - p.x;
            for q in &mut self.points[i + 1..] {
                q.x += dx;
            }
        }
        let point = &mut self.points[i];
        point.x = nx;
        point.y = y.clamp(0.0, 1.0);
    }

    /// Returns the index of the point nearest to `(x, y)` within the given distances.
    pub fn hit_point(&self, x: f64, y: f64, max_dx: f64, max_dy: f64) -> Option<usize> {
        self.points
            .iter()
            .enumerate()
            .filter(|(_, p)| (p.x - x).abs() <= max_dx && (p.y - y).abs() <= max_dy)
            .min_by(|(_, a), (_, b)| {
                let da = ((a.x - x) / max_dx).powi(2) + ((a.y - y) / max_dy).powi(2);
                let db = ((b.x - x) / max_dx).powi(2) + ((b.y - y) / max_dy).powi(2);
                da.total_cmp(&db)
            })
            .map(|(i, _)| i)
    }

    /// Returns the minimum and maximum point values.
    pub fn range(&self) -> (f64, f64) {
        self.points
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)))
    }
}

/// Speeds up sequential [`Envelope`] lookups by remembering the last segment.
#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    index: usize,
}

impl Cursor {
    /// Returns the value at `x` beats. Fast when `x` moves forward in small steps.
    pub fn value(&mut self, env: &Envelope, x: f64) -> f64 {
        let x = x.rem_euclid(env.length);
        let points = &env.points;
        let n = points.len();
        let mut i = self.index.min(n);
        if i > 0 && points[i - 1].x > x {
            i = points.partition_point(|p| p.x <= x);
        } else {
            while i < n && points[i].x <= x {
                i += 1;
            }
        }
        self.index = i;
        env.segment_value(i, x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_envelope() {
        let env = Envelope::flat(0.25);
        for i in 0..20 {
            assert_eq!(env.value_at(i as f64 * 0.37), 0.25);
        }
    }

    #[test]
    fn linear_ramp_and_wrap() {
        let env = Envelope::from_points(vec![Point::new(0.0, 0.0), Point::new(4.0, 1.0)]);
        assert!((env.value_at(1.0) - 0.25).abs() < 1e-12);
        assert!((env.value_at(5.0) - 0.25).abs() < 1e-12);
        assert!((env.value_at(-1.0) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn hold_steps_at_end_point() {
        let env = Envelope::from_points(vec![
            Point::new(0.0, 1.0),
            Point::new(2.0, 0.5).with_shape(Shape::Hold),
            Point::new(4.0, 0.5),
        ]);
        assert_eq!(env.value_at(1.999), 1.0);
        assert_eq!(env.value_at(2.0), 0.5);
    }

    #[test]
    fn cursor_matches_value_at() {
        let env = Envelope::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0).with_shape(Shape::DoubleCurve).with_tension(0.6),
            Point::new(2.5, 0.2).with_shape(Shape::Stairs).with_tension(0.1),
            Point::new(4.0, 0.9),
        ]);
        let mut cursor = Cursor::default();
        for i in 0..2000 {
            let x = i as f64 * 0.0137;
            assert_eq!(cursor.value(&env, x), env.value_at(x));
        }
        assert_eq!(cursor.value(&env, 0.5), env.value_at(0.5));
    }

    #[test]
    fn move_point_respects_neighbors_and_slide() {
        let mut env = Envelope::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.0),
        ]);
        env.move_point(1, 2.5, 0.5, false);
        assert_eq!(env.points[1].x, 2.0);
        env.move_point(1, 1.5, 0.5, true);
        assert_eq!(env.points[2].x, 1.5);
        assert_eq!(env.points[3].x, 2.5);
    }

    #[test]
    fn serde_roundtrip() {
        let env = Envelope::from_points(vec![Point::new(0.0, 0.3), Point::new(4.0, 0.7)]);
        let json = serde_json::to_string(&env).unwrap();
        assert_eq!(serde_json::from_str::<Envelope>(&json).unwrap(), env);
    }
}
