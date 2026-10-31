//! The `RingBuffer` of the Time and Delay shapers (0x2d2fb8 write) with its
//! `RingBufferReaderLegacyLanczos` read head (0x2e7348 stereo, 0x2e7670 one channel): 2^21 frames
//! per channel and a 63-tap windowed-sinc read whose bandwidth follows the read rate
//! (shapers_a.md 5.2, shapers_c.md 1.4).

/// Frames per channel.
pub const RING: usize = 1 << 21;
/// Taps of the sinc read head.
pub const TAPS: usize = 63;
/// The sinc table step (`DAT_01307250`) and its inverse (`DAT_01307254`).
const SINC_STEP: f64 = 0.101_242_735_981_941_22;
const SINC_SCALE: f64 = 9.877_251_625_061_035;
const SINC_N: usize = 1025;

/// A stereo ring with the sinc read head.
pub struct SincRing {
    l: Vec<f32>,
    r: Vec<f32>,
    write: usize,
    filled: usize,
    table: Vec<f64>,
}

impl Default for SincRing {
    fn default() -> Self {
        Self::new()
    }
}

impl SincRing {
    pub fn new() -> Self {
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
            l: vec![0.0; RING],
            r: vec![0.0; RING],
            write: 0,
            filled: 0,
            table,
        }
    }

    /// Frames written so far, up to [`RING`].
    pub fn filled(&self) -> usize {
        self.filled
    }

    /// Appends a block.
    pub fn write(&mut self, buf: &[[f32; 2]]) {
        for f in buf {
            self.l[self.write] = f[0];
            self.r[self.write] = f[1];
            self.write = (self.write + 1) & (RING - 1);
        }
        self.filled = (self.filled + buf.len()).min(RING);
    }

    /// Adds `gain * v` to the frame `back` samples behind the write position (the Delay's
    /// feedback, 0x2e54cc: `ring[w - n + i] += fb * out[i]`).
    pub fn add_scaled(&mut self, back: usize, gain: f32, v: [f32; 2]) {
        let j = self.write.wrapping_sub(back) & (RING - 1);
        self.l[j] = gain.mul_add(v[0], self.l[j]);
        self.r[j] = gain.mul_add(v[1], self.r[j]);
    }

    fn sinc(&self, x: f64) -> f64 {
        let a = x.abs() * SINC_SCALE;
        let i = a as usize;
        if i + 1 >= SINC_N {
            return 0.0;
        }
        self.table[i] + (a - i as f64) * (self.table[i + 1] - self.table[i])
    }

    /// The whole-sample read `k` samples behind the current input sample (`remaining` samples
    /// before the end of the block): 0 before the oldest written frame or past the newest.
    pub fn read_int(&self, k: i64, remaining: usize) -> [f32; 2] {
        let back = k + remaining as i64;
        if back < 1 || back as usize > self.filled {
            return [0.0; 2];
        }
        let j = (self.write as i64 - back) as usize & (RING - 1);
        [self.l[j], self.r[j]]
    }

    /// 0x2e7348: reads both channels `d` samples behind the current input sample. Taps past the
    /// newest written sample read that sample; taps older than the filled part are dropped.
    pub fn read_sinc(&self, d: f64, rate: f64, remaining: usize) -> [f32; 2] {
        let mut out = [0.0f64; 2];
        self.read_sinc_into(d, rate, remaining, &mut out, [true, true]);
        [out[0] as f32, out[1] as f32]
    }

    /// 0x2e7670: [`Self::read_sinc`] for channel `ch` alone (the Delay reads L and R at their own
    /// positions when `stereoOffset` is off centre).
    pub fn read_sinc_channel(&self, ch: usize, d: f64, rate: f64, remaining: usize) -> f32 {
        let mut out = [0.0f64; 2];
        self.read_sinc_into(d, rate, remaining, &mut out, [ch == 0, ch == 1]);
        out[ch] as f32
    }

    fn read_sinc_into(
        &self,
        d: f64,
        rate: f64,
        remaining: usize,
        out: &mut [f64; 2],
        chans: [bool; 2],
    ) {
        if d < 0.0 {
            return;
        }
        let base = self.write as f64 - remaining as f64;
        let pos = base - d;
        let ipos = pos.floor();
        let oldest = self.write as i64 - (self.filled + 0x1f).min(RING) as i64;
        if (ipos as i64) < oldest {
            return;
        }
        if d <= 9.999_999_747_378_752e-6 {
            if remaining > 0 && remaining <= self.filled {
                let k = (base - d) as i64 as usize & (RING - 1);
                *out = [self.l[k] as f64, self.r[k] as f64];
            }
            return;
        }
        let mut cut = 0.9 / rate.abs().max(1.0);
        if cut < 0.03125 {
            cut = 1.0;
        }
        let frac = pos - ipos;
        let half = (TAPS / 2) as i64;
        let newest = self.write as i64 - 1;
        for k in 0..TAPS as i64 {
            let idx = ipos as i64 + k - half;
            if idx < oldest {
                continue;
            }
            let t = (k - half) as f64 - frac;
            let win = self
                .sinc(2.0 * std::f64::consts::PI * (t + 1.0) / TAPS as f64)
                .max(0.0);
            let ker = cut * self.sinc(std::f64::consts::PI * cut * t) * win;
            let j = idx.min(newest) as usize & (RING - 1);
            if chans[0] {
                out[0] += ker * self.l[j] as f64;
            }
            if chans[1] {
                out[1] += ker * self.r[j] as f64;
            }
        }
    }
}
