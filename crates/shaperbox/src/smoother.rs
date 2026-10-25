//! `ValueSmoother` (vtable 0xb2f700, block step 0x2d04b0): three cascaded one-poles that ramp a
//! float parameter toward its state value (shapers_a.md section 1.3).

/// A smoothed parameter port.
#[derive(Clone, Debug)]
pub struct Smoother {
    a: f64,
    y: [f64; 3],
    /// True when the port holds a per-sample ramp in `buf` (else `value` is constant).
    pub ramping: bool,
    pub value: f32,
    pub buf: Vec<f32>,
}

impl Smoother {
    /// A port at `value`; `t` is the port's smoothing time (0 for every port seen so far).
    pub fn new(value: f32, sr: f64, t: f64) -> Self {
        let mut s = Self {
            a: 0.0,
            y: [value as f64; 3],
            ramping: false,
            value,
            buf: Vec::new(),
        };
        s.set_sample_rate(sr, t);
        s
    }

    /// 0x2d0904: `a = exp(-2 * (50 pi / sr) * exp(5.8918 t))`, a 50 Hz pole for `t = 0`.
    pub fn set_sample_rate(&mut self, sr: f64, t: f64) {
        self.a = (-2.0 * (50.0 * std::f64::consts::PI / sr) * (5.8918 * t).exp()).exp();
    }

    /// One block toward `target`.
    pub fn process(&mut self, target: f32, n: usize) {
        self.value = target;
        let t = target as f64;
        if (t - self.y[2]).abs() > 1.0 / 16384.0 {
            self.ramping = true;
            self.buf.resize(n, 0.0);
            let k = 1.0 - self.a;
            for v in &mut self.buf[..n] {
                self.y[0] += k * (t - self.y[0]);
                self.y[1] += k * (self.y[0] - self.y[1]);
                self.y[2] += k * (self.y[1] - self.y[2]);
                *v = self.y[2] as f32;
            }
        } else {
            self.ramping = false;
        }
    }

    /// The value at sample `i` of the last block.
    pub fn at(&self, i: usize) -> f32 {
        if self.ramping {
            self.buf[i]
        } else {
            self.value
        }
    }
}
