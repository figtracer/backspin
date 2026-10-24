//! Envelope editing tools.

use crate::{Envelope, Point, Shape};
use serde::{Deserialize, Serialize};

/// The time lane maps the full value range to this many beats.
pub const TIME_RANGE_BEATS: f64 = 8.0;

/// LFO waveforms for [`Envelope::lfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LfoShape {
    /// Sine wave.
    Sine,
    /// Triangle wave.
    Triangle,
    /// Rising saw wave.
    Saw,
    /// Square wave.
    Square,
}

impl Envelope {
    /// Mirrors every value: `y = 1 - y`.
    pub fn flip_vertical(&mut self) {
        for p in &mut self.points {
            p.y = 1.0 - p.y;
        }
    }

    /// Scales values around `anchor` by `factor`, then clamps them.
    pub fn scale_levels(&mut self, factor: f64, anchor: f64) {
        for p in &mut self.points {
            p.y = (anchor + (p.y - anchor) * factor).clamp(0.0, 1.0);
        }
    }

    /// Stretches values so the lowest becomes 0 and the highest becomes 1.
    pub fn normalize_levels(&mut self) {
        let (lo, hi) = self.range();
        if hi - lo < 1e-9 {
            return;
        }
        for p in &mut self.points {
            p.y = (p.y - lo) / (hi - lo);
        }
    }

    /// Removes interior linear points that change the curve by less than `tolerance`.
    pub fn decimate(&mut self, tolerance: f64) {
        let mut i = 1;
        while i + 1 < self.points.len() {
            let a = self.points[i - 1];
            let p = self.points[i];
            let b = self.points[i + 1];
            let linear = |q: &Point| q.shape == Shape::SingleCurve && q.tension.abs() < 1e-9;
            if linear(&p) && linear(&b) && b.x > a.x {
                let t = (p.x - a.x) / (b.x - a.x);
                let interp = a.y + (b.y - a.y) * t;
                if (interp - p.y).abs() < tolerance {
                    self.points.remove(i);
                    continue;
                }
            }
            i += 1;
        }
    }

    /// Replaces vertical jumps with ramps `width` beats long.
    pub fn smooth_abrupt(&mut self, width: f64) {
        let mut out = Vec::with_capacity(self.points.len() * 2);
        for (i, p) in self.points.iter().enumerate() {
            let prev = if i == 0 { None } else { Some(self.points[i - 1]) };
            match prev {
                Some(a) if p.shape == Shape::Hold => {
                    let x = (p.x - width).max(a.x);
                    out.push(Point::new(x, a.y));
                    out.push(Point::new(p.x, p.y).with_shape(Shape::Smooth));
                }
                Some(a) if p.x - a.x < width * 0.5 && (p.y - a.y).abs() > 1e-9 => {
                    out.pop();
                    let x = (p.x - width).max(out.last().map_or(0.0, |q: &Point| q.x));
                    out.push(Point::new(x, a.y).with_shape(a.shape).with_tension(a.tension));
                    out.push(Point::new(p.x, p.y).with_shape(Shape::Smooth));
                }
                _ => out.push(*p),
            }
        }
        self.points = out;
        self.normalize();
    }

    /// Sets every point's shape to [`Shape::Smooth`].
    pub fn all_smooth(&mut self) {
        for p in &mut self.points {
            p.shape = Shape::Smooth;
        }
    }

    /// Replaces the envelope with a step sequence: each step is `on` or `off` for `step_beats`.
    pub fn create_sequence(&mut self, steps: &[bool], step_beats: f64, on: f64, off: f64) {
        let mut points = Vec::new();
        let level = |b: bool| if b { on } else { off };
        let mut x = 0.0;
        let mut prev = None;
        for &step in steps.iter().cycle() {
            if x >= self.length - 1e-9 {
                break;
            }
            let y = level(step);
            if prev != Some(step) {
                points.push(Point::new(x, y).with_shape(Shape::Hold));
                prev = Some(step);
            }
            x += step_beats;
        }
        if let Some(step) = prev {
            points.push(Point::new(self.length, level(step)));
        }
        if !points.is_empty() {
            self.points = points;
        }
    }

    /// Replaces the envelope with an LFO between `lo` and `hi`, `cycles` times per loop.
    pub fn lfo(&mut self, shape: LfoShape, cycles: u32, lo: f64, hi: f64) {
        let cycles = cycles.max(1) as f64;
        let period = self.length / cycles;
        let mut points = Vec::new();
        let mut x = 0.0;
        while x < self.length - 1e-9 {
            match shape {
                LfoShape::Sine => {
                    let mid = (lo + hi) / 2.0;
                    points.push(Point::new(x, mid));
                    points.push(
                        Point::new(x + period * 0.25, hi).with_shape(Shape::HalfSine).with_tension(-1.0),
                    );
                    points.push(Point::new(x + period * 0.5, mid).with_shape(Shape::HalfSine).with_tension(1.0));
                    points.push(
                        Point::new(x + period * 0.75, lo).with_shape(Shape::HalfSine).with_tension(-1.0),
                    );
                }
                LfoShape::Triangle => {
                    points.push(Point::new(x, lo));
                    points.push(Point::new(x + period * 0.5, hi));
                }
                LfoShape::Saw => {
                    points.push(Point::new(x, lo).with_shape(Shape::Hold));
                    points.push(Point::new(x + period - 1e-6, hi));
                }
                LfoShape::Square => {
                    points.push(Point::new(x, hi).with_shape(Shape::Hold));
                    points.push(Point::new(x + period * 0.5, lo).with_shape(Shape::Hold));
                }
            }
            x += period;
        }
        let end = match shape {
            LfoShape::Sine => {
                Point::new(self.length, (lo + hi) / 2.0).with_shape(Shape::HalfSine).with_tension(1.0)
            }
            LfoShape::Triangle | LfoShape::Saw => Point::new(self.length, lo).with_shape(Shape::Hold),
            LfoShape::Square => Point::new(self.length, hi).with_shape(Shape::Hold),
        };
        points.push(end);
        self.points = points;
        self.normalize();
    }

    /// For a time envelope: changes the playback speed of the segment that ends at point `i` by
    /// `semitones`, by moving point `i` vertically.
    ///
    /// Speed is `1 + 8 * dy / dx`, because `y` spans 8 beats of offset.
    pub fn semitone_slope(&mut self, i: usize, semitones: f64) {
        if i == 0 || i >= self.points.len() {
            return;
        }
        let a = self.points[i - 1];
        let b = self.points[i];
        let dx = b.x - a.x;
        if dx <= 0.0 {
            return;
        }
        let speed = 1.0 + TIME_RANGE_BEATS * (b.y - a.y) / dx;
        let new_speed = speed * 2f64.powf(semitones / 12.0);
        let new_dy = (new_speed - 1.0) * dx / TIME_RANGE_BEATS;
        self.points[i].y = (a.y + new_dy).clamp(0.0, 1.0);
    }

    /// Step editing: replaces points between `x0` and `x1` with flat steps at `y`, one per `step`.
    /// With `pulse`, each step rises to `y` and falls back to 0 halfway through the step.
    pub fn step_draw(&mut self, x0: f64, x1: f64, y: f64, step: f64, pulse: bool) {
        let step = step.max(1e-3);
        let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        let start = (lo / step).floor() * step;
        let end = ((hi / step).floor() + 1.0) * step;
        let end = end.min(self.length);
        let before = self.value_at(start - 1e-9);
        let after = self.value_at(end);
        self.points.retain(|p| p.x < start - 1e-9 || p.x > end + 1e-9);
        let mut x = start;
        while x < end - 1e-9 {
            self.add_point(Point::new(x, y).with_shape(Shape::Hold));
            if pulse {
                self.add_point(Point::new(x + step * 0.5, 0.0).with_shape(Shape::Hold));
            }
            x += step;
        }
        if start > 0.0 {
            let i = self.points.partition_point(|p| p.x < start - 1e-9);
            self.points.insert(i, Point::new(start - 1e-6, before));
        }
        if end < self.length {
            self.add_point(Point::new(end, after).with_shape(Shape::Hold));
        } else {
            self.add_point(Point::new(end, y).with_shape(Shape::Hold));
        }
        self.normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_and_normalize() {
        let mut env = Envelope::from_points(vec![Point::new(0.0, 0.25), Point::new(4.0, 0.5)]);
        env.flip_vertical();
        assert_eq!(env.points[0].y, 0.75);
        env.normalize_levels();
        assert_eq!(env.points[0].y, 1.0);
        assert_eq!(env.points[1].y, 0.0);
    }

    #[test]
    fn decimate_removes_collinear_points() {
        let mut env = Envelope::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.25),
            Point::new(2.0, 0.5),
            Point::new(4.0, 1.0),
        ]);
        env.decimate(1e-6);
        assert_eq!(env.points.len(), 2);
    }

    #[test]
    fn sequence_gates() {
        let mut env = Envelope::flat(1.0);
        env.create_sequence(&[true, false], 0.5, 1.0, 0.0);
        assert_eq!(env.value_at(0.25), 1.0);
        assert_eq!(env.value_at(0.75), 0.0);
        assert_eq!(env.value_at(3.75), 0.0);
    }

    #[test]
    fn semitone_slope_changes_speed() {
        // Flat line: speed 1. One octave up gives speed 2: dy = dx / 8.
        let mut env = Envelope::from_points(vec![Point::new(0.0, 0.5), Point::new(1.0, 0.5)]);
        env.semitone_slope(1, 12.0);
        assert!((env.points[1].y - (0.5 + 1.0 / 8.0)).abs() < 1e-12);
    }

    #[test]
    fn lfo_stays_in_range() {
        for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Saw, LfoShape::Square] {
            let mut env = Envelope::flat(1.0);
            env.lfo(shape, 4, 0.2, 0.8);
            for i in 0..400 {
                let v = env.value_at(i as f64 * 0.01);
                assert!((0.2 - 1e-9..=0.8 + 1e-9).contains(&v), "{shape:?} {v}");
            }
        }
    }

    #[test]
    fn step_draw_fills_steps() {
        let mut env = Envelope::flat(1.0);
        env.step_draw(1.0, 1.9, 0.0, 0.5, false);
        assert_eq!(env.value_at(0.5), 1.0);
        assert_eq!(env.value_at(1.25), 0.0);
        assert_eq!(env.value_at(1.75), 0.0);
        assert_eq!(env.value_at(2.5), 1.0);
    }

    #[test]
    fn smooth_abrupt_ramps_holds() {
        let mut env = Envelope::from_points(vec![
            Point::new(0.0, 1.0),
            Point::new(2.0, 0.0).with_shape(Shape::Hold),
            Point::new(4.0, 0.0),
        ]);
        env.smooth_abrupt(0.25);
        let v = env.value_at(1.9);
        assert!(v > 0.0 && v < 1.0, "{v}");
    }
}
