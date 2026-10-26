//! Time (TimeShaperModulation::process 0x5634ac, sinc read 0x2e7348; shapers_a.md section 5):
//! the delay from the wave, the windowed-sinc read head and the step crossfade (section 5.3).

use super::Wave;

const RING: usize = 1 << 21;
const TAPS: usize = 63;
/// The sinc table step (`DAT_01307250`) and its inverse (`DAT_01307254`).
const SINC_STEP: f64 = 0.101_242_735_981_941_22;
const SINC_SCALE: f64 = 9.877_251_625_061_035;
const SINC_N: usize = 1025;
/// At most this many crossfade segments per block (`mod+0xa0`).
const MAX_SEGMENTS: usize = 8;

/// The wave processor and LFO state that the step crossfade reads (section 5.3).
#[derive(Clone, Copy, Debug)]
pub struct Steps<'a> {
    /// The rendered wave table (raw values in [-1, 1], 8192 entries).
    pub table: &'a [f32],
    /// The sorted step positions in table units.
    pub steps: &'a [usize],
    /// The LFO phase in cycles of the block's first sample.
    pub phase: f64,
    /// The LFO rate in Hz.
    pub rate: f64,
}

/// One crossfade segment: `[start, end)` in block samples, the wave value and slope through the
/// step.
#[derive(Clone, Copy, Debug)]
struct Segment {
    start: i32,
    end: i32,
    value: f32,
    slope: f32,
}

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

    /// 0x2e7348: reads both channels `d` samples behind the current input sample. Taps past the
    /// newest written sample read that sample; taps older than the filled part are dropped.
    fn read_sinc(&self, d: f64, rate: f64, remaining: usize) -> [f32; 2] {
        if d < 0.0 {
            return [0.0; 2];
        }
        let base = self.write as f64 - remaining as f64;
        let pos = base - d;
        let ipos = pos.floor();
        let oldest = self.write as i64 - (self.filled + 0x1f).min(RING) as i64;
        if (ipos as i64) < oldest {
            return [0.0; 2];
        }
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
        let newest = self.write as i64 - 1;
        let (mut al, mut ar) = (0.0f64, 0.0f64);
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
            al += ker * self.l[j] as f64;
            ar += ker * self.r[j] as f64;
        }
        [al as f32, ar as f32]
    }

    /// `range` from `range_seconds`; `steps` is the crossfade input, `None` when the crossfade
    /// is off (constant wave, no steps, or a one-shot LFO not in Note timing).
    pub fn process(&mut self, buf: &mut [[f32; 2]], w: Wave, range: f32, steps: Option<Steps>) {
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
        if let Some(st) = steps {
            self.crossfade(buf, range, st);
        }
    }

    /// 0x563964..0x563d14: the segments of the steps that fall in this block or within 4 ms after
    /// it. A step's segment ends one sample after the step and starts 4 ms before.
    fn segments(&self, n: usize, st: Steps) -> ([Segment; MAX_SEGMENTS], usize) {
        let mut out = [Segment {
            start: 0,
            end: 0,
            value: 0.0,
            slope: 0.0,
        }; MAX_SEGMENTS];
        let mut count = 0;
        if st.steps.is_empty() || st.table.len() < 8192 || st.rate <= 0.0 {
            return (out, 0);
        }
        let sr = self.sr as f64;
        // The oscillator phase the plugin reads here lags the rendered wave by one sample
        // (measured against the harness: the segments land one sample early otherwise).
        let phase = st.phase - st.rate / sr;
        let mut pos = (phase * 8192.0) as u32 as i32;
        let len = (sr * 0.004_000_000_189_989_805) as i32;
        while count < MAX_SEGMENTS {
            let cycle = pos.div_euclid(8192) * 8192;
            let next = st
                .steps
                .iter()
                .map(|&s| s as i32 + cycle)
                .find(|&p| p > pos)
                .or_else(|| {
                    st.steps
                        .iter()
                        .map(|&s| s as i32 + cycle + 8192)
                        .find(|&p| p > pos)
                });
            let Some(p) = next else { break };
            pos = p;
            if pos < 0 {
                break;
            }
            // When the step falls on a sample, `t` sits within rounding noise of an integer and
            // the plugin's own phase arithmetic decides the segment end; this form matches the
            // dumped segments but lands one sample off in some blocks (error about 7e-3 there).
            let t = (sr / st.rate) * (pos as f64 / 8192.0 - phase);
            let end = t as i32 + 1;
            let start = end - len;
            if start >= n as i32 {
                break;
            }
            let y0 = st.table[(pos & 0x1fff) as usize];
            let y1 = st.table[((pos + 1) & 0x1fff) as usize];
            let slope = ((y1 - y0) * st.rate as f32 * 8192.0) / self.sr;
            let value = y0 + slope * ((((t as i32) as f64 - t) + 1.0) as f32 + 1.0);
            out[count] = Segment {
                start,
                end,
                value,
                slope,
            };
            count += 1;
        }
        (out, count)
    }

    /// 0x56396c..0x563dd8: inside each segment the output fades (smoothstep) toward a read at the
    /// delay of the wave extrapolated back from the step.
    fn crossfade(&self, buf: &mut [[f32; 2]], range: f32, st: Steps) {
        let n = buf.len();
        let (segs, count) = self.segments(n, st);
        let half = range * 0.5;
        let sr = self.sr as f64;
        for sg in &segs[..count] {
            let f52 = sg.slope * -half;
            let c0 = (1.0 - (sg.value - sg.slope * sg.end as f32)) * half;
            let inv = 1.0 / (sg.end - sg.start) as f32;
            let rate = 1.0 - sr * f52 as f64;
            for i in sg.start.max(0)..sg.end.min(n as i32) {
                let dsec = f52.mul_add(i as f32, c0).max(0.0);
                let rr = self.read_sinc(sr * dsec as f64, rate, n - i as usize);
                let x = (inv * (-sg.start) as f32 + inv * i as f32).clamp(0.0, 1.0);
                let s = x * x * x.mul_add(-2.0, 3.0);
                let o = &mut buf[i as usize];
                for c in 0..2 {
                    o[c] += s * (rr[c] - o[c]);
                }
            }
        }
    }
}
