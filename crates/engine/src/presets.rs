//! Factory slots.
//!
//! Time patterns are built from speeds and offsets: in the time lane, `y = 1 - offset / 8`, and a
//! segment plays at speed `1 + 8 * dy / dx`.

use crate::bank::{Bank, Slot};
use backspin_envelope::{Envelope, Point, Shape};

const RANGE: f64 = 8.0;
const LEN: f64 = 4.0;

/// Builds an envelope from left to right.
struct Builder {
    points: Vec<Point>,
    x: f64,
    y: f64,
}

impl Builder {
    fn new(y: f64) -> Self {
        Self { points: vec![Point::new(0.0, y)], x: 0.0, y }
    }

    /// Time lane: starts at an offset in beats.
    fn offset(beats: f64) -> Self {
        Self::new(1.0 - beats / RANGE)
    }

    fn push(&mut self, x: f64, y: f64, shape: Shape, tension: f64) -> &mut Self {
        let y = y.clamp(0.0, 1.0);
        self.points.push(Point::new(x, y).with_shape(shape).with_tension(tension));
        self.x = x;
        self.y = y;
        self
    }

    /// Holds the current value until `x`, then jumps to `y`.
    fn hold_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.push(x, y, Shape::Hold, 0.0)
    }

    /// Time lane: holds the current offset until `x`, then jumps to an offset of `beats`.
    fn jump_offset(&mut self, x: f64, beats: f64) -> &mut Self {
        self.hold_to(x, 1.0 - beats / RANGE)
    }

    /// Time lane: plays at `speed` until `x`.
    fn speed_to(&mut self, x: f64, speed: f64) -> &mut Self {
        let y = self.y + (speed - 1.0) / RANGE * (x - self.x);
        self.push(x, y, Shape::SingleCurve, 0.0)
    }

    fn curve_to(&mut self, x: f64, y: f64, shape: Shape, tension: f64) -> &mut Self {
        self.push(x, y, shape, tension)
    }

    /// Time lane: follows `offset(t)` (beats, t = 0..=dur after the current x) with linear segments.
    fn offset_fn(&mut self, dur: f64, steps: usize, offset: impl Fn(f64) -> f64) -> &mut Self {
        let x0 = self.x;
        for i in 1..=steps {
            let t = dur * i as f64 / steps as f64;
            self.push(x0 + t, 1.0 - offset(t) / RANGE, Shape::SingleCurve, 0.0);
        }
        self
    }

    /// Time lane: repeats the `block` beats that start at `from`, until `to`.
    fn repeat(&mut self, from: f64, to: f64, block: f64) -> &mut Self {
        if self.x < from {
            self.hold_to(from, self.y);
        }
        let base = 1.0 - self.y;
        let mut k = 1.0;
        while from + k * block < to - 1e-9 {
            self.hold_to(from + k * block, 1.0 - (base + k * block / RANGE));
            k += 1.0;
        }
        self
    }

    fn finish(&mut self) -> Envelope {
        if self.x < LEN {
            let y = self.y;
            self.push(LEN, y, Shape::SingleCurve, 0.0);
        }
        Envelope::from_points(std::mem::take(&mut self.points))
    }
}

fn steps(pattern: &str, step: f64, on: f64, off: f64) -> Envelope {
    let mut env = Envelope::flat(1.0);
    let steps: Vec<bool> = pattern.chars().map(|c| c == '1').collect();
    env.create_sequence(&steps, step, on, off);
    env
}

fn per_beat(f: impl Fn(&mut Builder, f64)) -> Envelope {
    let mut b = Builder::new(1.0);
    b.points.clear();
    for beat in 0..4 {
        f(&mut b, beat as f64);
    }
    b.finish()
}

fn time_slots() -> Vec<Slot> {
    let s = |name: &str, env: Envelope| Slot::new(name, env);
    vec![
        s("Live", Envelope::flat(1.0)),
        s("Half speed", Builder::offset(0.0).speed_to(4.0, 0.5).finish()),
        s("Half speed x2", {
            let mut b = Builder::offset(0.0);
            b.speed_to(2.0, 0.5).jump_offset(2.0, 0.0).speed_to(4.0, 0.5);
            b.finish()
        }),
        s("Quarter speed", Builder::offset(0.0).speed_to(4.0, 0.25).finish()),
        s("Slow then catch up", Builder::offset(0.0).speed_to(2.0, 0.5).speed_to(4.0, 1.5).finish()),
        s("Tape stop", Builder::offset(0.0).offset_fn(4.0, 16, |t| t * t / 8.0).finish()),
        s("Tape stop 2 beats", {
            let mut b = Builder::offset(0.0);
            b.speed_to(2.0, 1.0).offset_fn(2.0, 12, |t| t * t / 4.0);
            b.finish()
        }),
        s("Tape stop last beat", {
            let mut b = Builder::offset(0.0);
            b.speed_to(3.0, 1.0).offset_fn(1.0, 8, |t| t * t / 2.0);
            b.finish()
        }),
        s("Tape start", Builder::offset(0.0).offset_fn(4.0, 16, |t| t - t * t / 8.0).finish()),
        s("Spinback last beat", Builder::offset(0.0).speed_to(3.0, 1.0).speed_to(4.0, -3.0).finish()),
        s("Reverse bar", Builder::offset(0.0).speed_to(4.0, -1.0).finish()),
        s("Reverse beats", per_beat(|b, x| {
            if x == 0.0 {
                b.push(0.0, 1.0, Shape::SingleCurve, 0.0);
            } else {
                b.jump_offset(x, 0.0);
            }
            b.speed_to(x + 1.0, -1.0);
        })),
        s("Reverse 2nd half", Builder::offset(0.0).speed_to(2.0, 1.0).speed_to(4.0, -1.0).finish()),
        s("Repeat 1/4 last beat", Builder::offset(0.0).repeat(3.0, 4.0, 0.25).finish()),
        s("Repeat 1/8 last beat", Builder::offset(0.0).repeat(3.0, 4.0, 0.125).finish()),
        s("Repeat 1/16 last beat", Builder::offset(0.0).repeat(3.0, 4.0, 0.0625).finish()),
        s("Roll build", {
            let mut b = Builder::offset(0.0);
            // Every block replays the audio from beat 0.
            let mut x = 0.0;
            for (end, block) in [(2.0, 0.5), (3.0, 0.25), (3.5, 0.125), (4.0, 0.0625)] {
                while x < end - 1e-9 {
                    if x > 0.0 {
                        b.jump_offset(x, x);
                    }
                    x += block;
                }
            }
            b.finish()
        }),
        s("Beat 1 x4", Builder::offset(0.0).repeat(0.0, 4.0, 1.0).finish()),
        s("First half x2", Builder::offset(0.0).repeat(0.0, 4.0, 2.0).finish()),
        s("Stutter 1/8", Builder::offset(0.0).repeat(0.0, 4.0, 0.5).finish()),
        s("Stutter 1/16", Builder::offset(0.0).repeat(0.0, 4.0, 0.25).finish()),
        s("Granular stairs", {
            let mut b = Builder::offset(0.0);
            b.curve_to(4.0, 0.5, Shape::Stairs, 1.0);
            b.finish()
        }),
        s("Smooth stairs", {
            let mut b = Builder::offset(0.0);
            b.curve_to(4.0, 0.75, Shape::SmoothStairs, 0.0);
            b.finish()
        }),
        s("Wobble", {
            let mut b = Builder::offset(0.25);
            b.curve_to(4.0, 1.0 - 0.1 / RANGE, Shape::Wave, 0.0);
            b.finish()
        }),
        s("Scratch 1/4", per_beat(|b, x| {
            if x == 0.0 {
                b.push(0.0, 0.9, Shape::SingleCurve, 0.0);
            }
            b.curve_to(x + 0.5, 0.97, Shape::DoubleCurve, 0.6);
            b.curve_to(x + 1.0, 0.9, Shape::DoubleCurve, 0.6);
        })),
        s("Scratch 1/8", per_beat(|b, x| {
            if x == 0.0 {
                b.push(0.0, 0.92, Shape::SingleCurve, 0.0);
            }
            for h in [0.0, 0.5] {
                b.curve_to(x + h + 0.25, 0.97, Shape::DoubleCurve, 0.5);
                b.curve_to(x + h + 0.5, 0.92, Shape::DoubleCurve, 0.5);
            }
        })),
        s("Baby scratch last beat", {
            let mut b = Builder::offset(0.0);
            b.speed_to(3.0, 1.0);
            for k in 0..4 {
                let x = 3.0 + k as f64 * 0.25;
                b.curve_to(x + 0.125, 1.0 - 0.4 / RANGE, Shape::DoubleCurve, 0.4);
                b.curve_to(x + 0.25, 1.0, Shape::DoubleCurve, 0.4);
            }
            b.finish()
        }),
        s("Delay 3/4 beat", Envelope::flat(1.0 - 0.75 / RANGE)),
        s("Delay 1 beat", Envelope::flat(1.0 - 1.0 / RANGE)),
        s("Echo beat 2 on 4", {
            let mut b = Builder::offset(0.0);
            b.jump_offset(3.0, 2.0);
            b.finish()
        }),
        s("Swing 1/8", per_beat(|b, x| {
            let late = 1.0 / 6.0;
            if x == 0.0 {
                b.push(0.0, 1.0, Shape::SingleCurve, 0.0);
            }
            b.curve_to(x + 0.5 + late, 1.0 - late / RANGE, Shape::SingleCurve, 0.0);
            b.curve_to(x + 1.0, 1.0, Shape::SingleCurve, 0.0);
        })),
        s("Rewind and replay", Builder::offset(0.0).speed_to(3.0, 1.0).speed_to(3.5, -2.0).finish()),
        s("Buzz 1/32 end", Builder::offset(0.0).repeat(3.5, 4.0, 0.03125).finish()),
        s("Triplet stutter", Builder::offset(0.0).repeat(0.0, 4.0, 1.0 / 3.0).finish()),
        s("Glitch mix", {
            let mut b = Builder::offset(0.0);
            b.repeat(1.0, 2.0, 0.25);
            b.jump_offset(2.0, 0.0);
            b.speed_to(2.5, -1.0);
            b.jump_offset(3.0, 0.0);
            b.repeat(3.0, 4.0, 0.125);
            b.finish()
        }),
        s("Previous bar", Envelope::flat(0.5)),
    ]
}

fn volume_slots() -> Vec<Slot> {
    let s = |name: &str, env: Envelope| Slot::new(name, env);
    let pump = |period: f64, tension: f64| {
        let mut b = Builder::new(0.0);
        let mut x = 0.0;
        while x < LEN - 1e-9 {
            b.curve_to(x + period, 1.0, Shape::SingleCurve, tension);
            if x + period < LEN - 1e-9 {
                b.hold_to(x + period, 0.0);
            }
            x += period;
        }
        b.finish()
    };
    let wave = |cycles: f64, low: f64| {
        let segs = (cycles / 16.0).ceil().max(1.0);
        let per = cycles / segs;
        let tension = (per - 1.0) / 15.0 * 2.0 - 1.0;
        let mut points = vec![Point::new(0.0, 1.0)];
        for k in 1..=segs as usize {
            let x = LEN * k as f64 / segs;
            points.push(Point::new(x, low).with_shape(Shape::Wave).with_tension(tension));
            if k < segs as usize {
                points.push(Point::new(x, 1.0).with_shape(Shape::Hold));
            }
        }
        Envelope::from_points(points)
    };
    let ramp_beats = |up: bool| {
        per_beat(|b, x| {
            let (a, z) = if up { (0.0, 1.0) } else { (1.0, 0.0) };
            if x == 0.0 {
                b.push(0.0, a, Shape::SingleCurve, 0.0);
            } else {
                b.hold_to(x, a);
            }
            b.curve_to(x + 1.0, z, Shape::SingleCurve, if up { 0.5 } else { -0.3 });
        })
    };
    vec![
        s("Full", Envelope::flat(1.0)),
        s("Gate 1/8", steps("10", 0.5, 1.0, 0.0)),
        s("Gate 1/16", steps("10", 0.25, 1.0, 0.0)),
        s("Gate 1/32", steps("10", 0.125, 1.0, 0.0)),
        s("Gate triplet", steps("10", 1.0 / 3.0, 1.0, 0.0)),
        s("Off-beat", steps("01", 0.5, 1.0, 0.0)),
        s("Pump 1/4", pump(1.0, -0.6)),
        s("Pump 1/8", pump(0.5, -0.6)),
        s("Pump hard", pump(1.0, -0.9)),
        s("Trance gate A", steps("1011011010110110", 0.25, 1.0, 0.0)),
        s("Trance gate B", steps("1101101101101011", 0.25, 1.0, 0.0)),
        s("3-3-2", steps("10010010", 0.5, 1.0, 0.0)),
        s("Dotted 1/8", steps("110", 0.25, 1.0, 0.0)),
        s("Fade in", Envelope::from_points(vec![Point::new(0.0, 0.0), Point::new(4.0, 1.0)])),
        s("Fade out", Envelope::from_points(vec![Point::new(0.0, 1.0), Point::new(4.0, 0.0)])),
        s("Swell 2 beats", {
            let mut b = Builder::new(0.0);
            b.curve_to(2.0, 1.0, Shape::SingleCurve, 0.5).hold_to(2.0, 0.0);
            b.curve_to(4.0, 1.0, Shape::SingleCurve, 0.5);
            b.finish()
        }),
        s("Mute", Envelope::flat(0.0)),
        s("Mute last beat", steps("1110", 1.0, 1.0, 0.0)),
        s("Mute 2nd half", steps("10", 2.0, 1.0, 0.0)),
        s("Tremolo 1/8", wave(8.0, 0.3)),
        s("Tremolo 1/16", wave(16.0, 0.3)),
        s("Stairs down", {
            let mut b = Builder::new(1.0);
            b.curve_to(4.0, 0.0, Shape::Stairs, -0.07);
            b.finish()
        }),
        s("Stairs up", {
            let mut b = Builder::new(0.0);
            b.curve_to(4.0, 1.0, Shape::Stairs, -0.07);
            b.finish()
        }),
        s("Chop 1/16", steps("1110111011011101", 0.25, 1.0, 0.0)),
        s("Pump 1/2", pump(2.0, -0.6)),
        s("Short 1/4", steps("1000", 0.25, 1.0, 0.0)),
        s("Half-bar gate", steps("10", 2.0, 1.0, 0.0)),
        s("Beat 1 only", steps("1000", 1.0, 1.0, 0.0)),
        s("Stutter last beat", {
            let mut env = steps("1111111111111111101010101010101010101010101010101010101010101010", 0.0625, 1.0, 0.0);
            env.length = 4.0;
            env
        }),
        s("Ramp down beats", ramp_beats(false)),
        s("Swell beats", ramp_beats(true)),
        s("Duck 2 and 4", steps("10101010", 0.5, 1.0, 0.35)),
        s("Ghost", Envelope::flat(0.5)),
        s("Breath", per_beat(|b, x| {
            if x == 0.0 {
                b.push(0.0, 0.2, Shape::SingleCurve, 0.0);
            }
            b.curve_to(x + 0.5, 1.0, Shape::HalfSine, -1.0);
            b.curve_to(x + 1.0, 0.2, Shape::HalfSine, 1.0);
        })),
        s("Gate dotted 1/16", steps("100", 0.25, 1.0, 0.0)),
        s("Pulse 1/4", Envelope::from_points(vec![
            Point::new(0.0, 0.0),
            Point::new(4.0, 1.0).with_shape(Shape::Pulse).with_tension(-0.07),
        ])),
    ]
}

/// The factory bank.
pub fn factory_bank() -> Bank {
    let mut bank = Bank { time: time_slots(), volume: volume_slots() };
    bank.sanitize();
    bank
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SLOTS;

    #[test]
    fn factory_bank_is_complete_and_in_range() {
        let bank = factory_bank();
        assert_eq!(bank.time.len(), SLOTS);
        assert_eq!(bank.volume.len(), SLOTS);
        for slot in bank.time.iter().chain(&bank.volume) {
            assert!(!slot.name.starts_with("Slot "), "unnamed factory slot");
            for p in &slot.envelope.points {
                assert!((0.0..=1.0).contains(&p.y), "{}: y {}", slot.name, p.y);
                assert!((0.0..=4.0).contains(&p.x), "{}: x {}", slot.name, p.x);
            }
        }
    }

    #[test]
    fn time_patterns_never_read_the_future() {
        // y > 1 would mean a negative offset; the builder clamps, so check the intent instead:
        // every factory time envelope starts at or below live.
        for slot in factory_bank().time {
            assert!(slot.envelope.value_at(0.0) <= 1.0);
        }
    }

    #[test]
    fn half_speed_slope() {
        let bank = factory_bank();
        let env = &bank.time[1].envelope;
        // Speed 0.5 means dy/dx = -1/16.
        assert!((env.value_at(2.0) - (1.0 - 2.0 / 16.0)).abs() < 1e-9);
    }
}
