//! Per-sample envelope rendering (envelope.md section 3).

use crate::Envelope;

/// The playback state of an envelope (the 0x40-byte record at lane+0x160).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cursor {
    /// Index of the point at the end of the current segment.
    pub idx: usize,
    /// Position inside the segment, 0..1.
    pub phase: f64,
    /// `1 / dx` of the current segment.
    pub inv_len: f64,
    /// Value at the segment start.
    pub start: f64,
    /// End value minus start value.
    pub delta: f64,
    /// The last value of the non-looping stepper (state+0x38).
    pub out: f64,
    /// True when the non-looping stepper reached the last point (mode 9).
    pub done: bool,
}

impl Cursor {
    /// The cursor at the envelope start (virtual 23, 0x23f700). The first step jumps into the
    /// first segment when point 0 is at 0.
    pub fn new(env: &Envelope) -> Self {
        let dx0 = env.points.first().map_or(0.0, |p| p.dx);
        let (inv_len, phase) = if dx0 == 0.0 {
            (1.0, 1.0)
        } else {
            (1.0 / dx0, 0.0)
        };
        let start = env.value(0);
        Self {
            idx: 0,
            phase,
            inv_len,
            start,
            delta: 0.0,
            out: start,
            done: false,
        }
    }

    /// The current position in envelope units (0x243100), for the editor's playback locator.
    pub fn position(&self, env: &Envelope) -> f64 {
        let dx = env.points.get(self.idx).map_or(0.0, |p| p.dx);
        env.abs_pos(self.idx.saturating_sub(1)) + self.phase * dx
    }

    /// Advances one looping step and returns the value (one iteration of 0x2422d0).
    #[inline]
    fn step_loop(&mut self, env: &Envelope, step: f64, dphase: &mut f64) -> f64 {
        let hi = env.hi();
        self.phase += *dphase;
        while phase_wraps(self.phase) {
            self.phase = if self.inv_len == 0.0 {
                step
            } else {
                (self.phase - 1.0) / self.inv_len
            };
            self.start = env.value(self.idx);
            self.idx = if self.idx < hi { self.idx + 1 } else { 0 };
            self.delta = env.value(self.idx) as f32 as f64 - self.start;
            let len = env.points[self.idx].dx;
            if len <= 0.0 {
                self.inv_len = 1.0;
                self.phase += 1.0;
            } else {
                self.inv_len = 1.0 / len;
                self.phase *= self.inv_len;
            }
            *dphase = step * self.inv_len;
        }
        let f = env.points.get(self.idx).map_or(0.0, |p| p.eval(self.phase));
        self.start + f * self.delta
    }

    /// Renders `out.len()` samples, advancing `step` envelope units per sample and looping at the
    /// end (0x2422d0, f64 output). The phase advances before the first value.
    pub fn render_f64(&mut self, env: &Envelope, step: f64, out: &mut [f64]) {
        if self.idx > env.hi() || env.points.is_empty() {
            return;
        }
        let mut dphase = step * self.inv_len;
        for o in out {
            *o = self.step_loop(env, step, &mut dphase);
        }
    }

    /// The same as [`Self::render_f64`] with f32 output (0x2415c0).
    pub fn render_f32(&mut self, env: &Envelope, step: f64, out: &mut [f32]) {
        if self.idx > env.hi() || env.points.is_empty() {
            return;
        }
        let mut dphase = step * self.inv_len;
        for o in out {
            *o = self.step_loop(env, step, &mut dphase) as f32;
        }
    }

    /// Advances by `amount` in one step (the "big step" of a skipped lane and the slot seek):
    /// `render_f32(amount, env, cursor, tmp, 1)`.
    pub fn advance(&mut self, env: &Envelope, amount: f64) {
        let mut tmp = [0f32];
        self.render_f32(env, amount, &mut tmp);
    }

    /// One non-looping step of `amount` (0x2408d0): stops at the last point instead of wrapping,
    /// and does not round the end value to f32. The value is in [`Cursor::out`].
    pub fn step_once(&mut self, env: &Envelope, amount: f64) {
        let hi = env.hi();
        if self.idx > hi || env.points.is_empty() {
            return;
        }
        self.phase += amount * self.inv_len;
        while phase_wraps(self.phase) {
            let old = self.idx;
            self.phase = if self.inv_len == 0.0 {
                amount
            } else {
                (self.phase - 1.0) / self.inv_len
            };
            self.start = env.value(self.idx);
            if self.idx < hi {
                self.idx += 1;
            } else {
                self.done = true;
            }
            self.delta = env.value(self.idx) - self.start;
            if self.idx == old {
                self.phase = 0.0;
                self.inv_len = 0.0;
            } else {
                let len = env.points[self.idx].dx;
                if len <= 0.0 {
                    self.inv_len = 1.0;
                    self.phase += 1.0;
                } else {
                    self.inv_len = 1.0 / len;
                    self.phase *= self.inv_len;
                }
            }
        }
        let f = env.points[self.idx].eval(self.phase);
        self.out = self.start + f * self.delta;
    }
}

impl Envelope {
    /// The value at `pos` (GetValueAt 0x2430a0): a fresh cursor plus one non-looping step.
    pub fn value_at(&self, pos: f64) -> f64 {
        let mut c = Cursor::new(self);
        c.step_once(self, pos);
        c.out
    }
}

/// The loop test of the renderers: the unsigned high word of the phase is above 0x3fefffff, which
/// is true for phase >= 1 and for negative phases.
#[inline]
fn phase_wraps(phase: f64) -> bool {
    ((phase.to_bits() >> 32) as u32) > 0x3fef_ffff
}
