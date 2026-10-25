//! Time (TimeShaperModulation::process 0x5634ac, sinc read 0x2e7348; shapers_a.md section 5).
//! The step crossfade of section 5.3 is not ported yet (it needs the wave processor of core.md).

use super::Wave;

const RING: usize = 1 << 21;
const TAPS: usize = 63;
/// The sinc table step (`DAT_01307250`) and its inverse (`DAT_01307254`).
const SINC_STEP: f64 = 0.101_242_735_981_941_22;
const SINC_SCALE: f64 = 9.877_251_625_061_035;
const SINC_N: usize = 1025;

/// `delayTimeMax` 0..7 to the full range in seconds, in f32 as `h + h`.
pub fn range_seconds(mode: i32, bpm: f32, f_lfo: f32) -> f32 {
    let h = match mode {
        0 => 1.0 / (2.0 * f_lfo),
        1..=4 => (120.0 / bpm) * (1 << (mode - 1)) as f32,
        5 => 0.0025,
        6 => 0.01,
        _ => 0.02,
    };
    h + h
}

/// `f32(R - R*w)` with one rounding (fmadd, 0x563720).
pub fn delay_seconds(r: f32, w: f32) -> f32 {
    (-r).mul_add(w, r)
}

/// One Time band: a ring of the input and a read head.
pub struct TimeBand {
    sr: f32,
    l: Vec<f32>,
    r: Vec<f32>,
    write: usize,
    filled: usize,
    prev_d: f64,
    table: Vec<f64>,
}

impl TimeBand {
    pub fn new(sr: f32) -> Self {
        let table = (0..SINC_N)
            .map(|i| {
                if i == 0 {
                    1.0
                } else {
                    (i as f64 * SINC_STEP).sin() / (i as f64 * SINC_STEP)
                }
            })
            .collect();
        Self {
            sr,
            l: vec![0.0; RING],
            r: vec![0.0; RING],
            write: 0,
            filled: 0,
            prev_d: 0.0,
            table,
        }
    }

    fn sinc(&self, x: f64) -> f64 {
        let a = x.abs() * SINC_SCALE;
        let i = a as usize;
        if i + 1 >= SINC_N {
            return 0.0;
        }
        self.table[i] + (a - i as f64) * (self.table[i + 1] - self.table[i])
    }

    fn read_sinc(&self, d: f64, rate: f64, remaining: usize) -> [f32; 2] {
        let base = self.write as f64 - remaining as f64;
        let pos = base - d;
        if d < 0.0 {
            return [0.0; 2];
        }
        let ipos = pos.floor();
        if d <= 9.999_999_747_378_752e-6 {
            if remaining > 0 && remaining <= self.filled {
                let k = (base - d) as i64 as usize & (RING - 1);
                return [self.l[k], self.r[k]];
            }
            return [0.0; 2];
        }
        let mut cut = 0.9 / rate.abs().max(1.0);
        if cut < 0.03125 {
            cut = 1.0;
        }
        let frac = pos - ipos;
        let half = (TAPS / 2) as i64;
        let oldest = self.write as i64 - (self.filled + 0x1f).min(RING) as i64;
        let newest = self.write as i64 - remaining as i64 + 1;
        let (mut al, mut ar) = (0.0f64, 0.0f64);
        for k in 0..TAPS as i64 {
            let idx = ipos as i64 + k - half;
            if idx < oldest || idx >= newest {
                continue;
            }
            let t = (k - half) as f64 - frac;
            let win = self
                .sinc(2.0 * std::f64::consts::PI * (t + 1.0) / TAPS as f64)
                .max(0.0);
            let ker = cut * self.sinc(std::f64::consts::PI * cut * t) * win;
            let j = idx as usize & (RING - 1);
            al += ker * self.l[j] as f64;
            ar += ker * self.r[j] as f64;
        }
        [al as f32, ar as f32]
    }

    /// `range` from `range_seconds`.
    pub fn process(&mut self, buf: &mut [[f32; 2]], w: Wave, range: f32) {
        let n = buf.len();
        for f in buf.iter() {
            self.l[self.write] = f[0];
            self.r[self.write] = f[1];
            self.write = (self.write + 1) & (RING - 1);
        }
        self.filled = (self.filled + n).min(RING);
        match w {
            Wave::PerSample(ws) => {
                for i in 0..n {
                    let d = (self.sr as f64 * delay_seconds(range, ws[i]) as f64).max(0.0);
                    buf[i] = self.read_sinc(d, (self.prev_d - d) + 1.0, n - i);
                    self.prev_d = d;
                }
            }
            Wave::Constant(wc) => {
                // Integer delay, truncated toward zero, no interpolation.
                let k = (self.sr as f64 * delay_seconds(range, wc) as f64) as i64;
                self.prev_d = k as f64;
                for (i, f) in buf.iter_mut().enumerate() {
                    let back = k + (n - i) as i64;
                    *f = if back < 1 || back as usize > self.filled || k >= 0x1f_fe00 {
                        [0.0; 2]
                    } else {
                        let j = (self.write as i64 - back) as usize & (RING - 1);
                        [self.l[j], self.r[j]]
                    };
                }
            }
        }
    }
}
