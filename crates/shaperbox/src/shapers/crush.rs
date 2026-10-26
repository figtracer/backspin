//! Crush (CrushShaperModulation::process 0x557698; shapers_a.md section 8): per band, bit
//! reduction (0x559164 with the quantiser 0x5593b0), the post gain, the decimator (0x558eec), the
//! 350 Hz tone blend and a 10 Hz DC blocker. All arithmetic is f32 like the plugin.

use super::{OnePole, Wave, tpt_k};
use crate::smoother::Smoother;

/// The MS rand LCG seed of every Crush generator (ctor 0x556ef4).
const SEED: i32 = 0xe1873;

fn lcg(x: &mut i32) -> i32 {
    *x = x.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
    *x
}

/// Two LCG draws averaged, in [-1, 1).
fn noise(x: &mut i32) -> f32 {
    let a = lcg(x) as f32 / 2.147_483_6e9;
    let b = lcg(x) as f32 / 2.147_483_6e9;
    (a + b) * 0.5
}

/// `x - floor(x)` through `modff` as the plugin computes it.
fn frac(x: f32) -> f32 {
    let f = x.fract();
    if f >= 0.0 { f } else { f + 1.0 }
}

/// 0x5593b0: quantises `x` to `floor(x) * step`, averaging the staircase along the straight
/// line from the previous input when the level changes inside the sample.
#[derive(Clone, Copy, Default)]
struct Quantiser {
    px: f32,
    pf: f32,
    ps: f32,
}

impl Quantiser {
    fn tick(&mut self, x: f32, step: f32) -> f32 {
        let fl = x - frac(x);
        let (px, pf, ps) = (self.px, self.pf, self.ps);
        let out = if fl == pf {
            fl * step
        } else {
            let cross = |a: f32, level: f32| {
                let d = (x - level) - (a - level);
                if d != 0.0 { -(a - level) / d } else { 0.0 }
            };
            let (t5, t4) = if fl <= pf {
                (cross(px, pf), cross(px, fl + 1.0))
            } else {
                (cross(px, pf + 1.0), cross(px, fl))
            };
            fl * step * (1.0 - t4) + (t5 * pf * ps + (t4 - t5) * (fl * step + pf * ps) * 0.5)
        };
        self.set(x, fl, step);
        out
    }

    fn set(&mut self, x: f32, fl: f32, step: f32) {
        self.px = x;
        self.pf = fl;
        self.ps = step;
    }
}

/// 0x559164 for one channel: `Q(off + L x) - Q(off)` with its own dither generator.
#[derive(Clone, Copy)]
struct Bits {
    offset: Quantiser,
    signal: Quantiser,
    seed: i32,
}

impl Bits {
    fn new() -> Self {
        Self {
            offset: Quantiser::default(),
            signal: Quantiser::default(),
            seed: SEED,
        }
    }

    /// `a` from `bits_amount`, `dc` the dc-offset port.
    fn tick(&mut self, x: f32, a: f32, dc: f32, dither: bool) -> f32 {
        let levels = a * 32767.0 + 1.0;
        let d = if dither {
            noise(&mut self.seed) * 0.5
        } else {
            0.0
        };
        let step = 1.0 / levels;
        let off = (dc * 0.499 + 0.5) + d;
        let v = off + levels * x;
        if a == 1.0 {
            // Bypass, keeping the quantiser states current.
            self.offset.set(off, off - frac(off), step);
            self.signal.set(v, v - frac(v), step);
            return x;
        }
        let o = self.offset.tick(off, step);
        self.signal.tick(v, step) - o
    }
}

/// `u^32 * 0.99957275 + u * 14/32767` for the clamped bits wave `u` (0x557898).
fn bits_amount(w: f32) -> f32 {
    let u = w.clamp(0.0, 1.0);
    let mut p = u * u * u * u;
    p *= p;
    p *= p;
    p = p * p * 0.999_572_75;
    u.mul_add(0.000_427_246_1, p)
}

/// The post gain (0x557a4c): `1 - 0.5 (0.5 d^3 + 0.5 d^9 b^3)`, `d = 1 - c(w)`.
fn comp_gain(w: f32, b: f32) -> f32 {
    let d = if w >= 0.0 { 1.0 - w.min(1.0) } else { 1.0 };
    let d3 = d * d * d;
    (d3 * 0.5 + d3 * d3 * d3 * 0.5 * b * b * b) * -0.5 + 1.0
}

/// The hold rate step per sample of the decimate wave (0x558e94), 1 at the top.
fn decimate_step(w: f32, sr: f32) -> f32 {
    let u = w.clamp(0.0, 1.0);
    if u == 1.0 {
        return 1.0;
    }
    let hz = u.mul_add(5.240_677, 5.458_648_7).exp() - 225.96;
    hz.max(1e-12) / sr
}

/// The Pade `tan(w)` of 0x557ecc for the damp filter (`w = pi * min(step, 0.4999)`).
fn pade_tan(step: f32) -> f32 {
    let w = if step < 0.0 {
        0.0
    } else {
        step.min(0.499_9) * std::f32::consts::PI
    };
    let w2 = w * w;
    let num = w * w2.mul_add(w2.mul_add(w2 - 378.0, 17325.0), -135_135.0);
    let den = w2.mul_add(w2.mul_add(w2.mul_add(28.0, -3150.0), 62370.0), -135_135.0);
    num / den
}

/// 0x558eec for one channel: an optional 8-pole damp low-pass, then a sample and hold whose new
/// value is the input interpolated at the phase crossing.
#[derive(Clone, Copy)]
struct Decimator {
    prev: f32,
    hold: f32,
    phase: f32,
    damp: [f32; 8],
    seed: i32,
}

impl Decimator {
    fn new() -> Self {
        Self {
            prev: 0.0,
            hold: 0.0,
            phase: 0.0,
            damp: [0.0; 8],
            seed: SEED,
        }
    }

    /// `jit` is the squared jitter port, `g` the damp coefficient, `active` false when the
    /// decimate wave is the constant 1.
    fn tick(&mut self, x: f32, step: f32, jit: f32, g: f32, damp: bool, active: bool) -> f32 {
        if !active {
            self.damp = [x; 8];
            self.hold = x;
            self.prev = x;
            self.phase = frac(step + self.phase);
            return x;
        }
        let mut x = x;
        if damp {
            let g = g.max(0.0);
            let k = g / (g + 1.0);
            for s in &mut self.damp {
                let v = k * (x - *s);
                let y = *s + v;
                *s = v + y;
                x = y;
            }
        }
        let n = noise(&mut self.seed);
        let step =
            (((n * 0.5 * jit + jit * jit * -0.271_828_17) * 10.0 + 1.0) * step).clamp(0.0, 1.0);
        let phase = self.phase;
        let mut out = self.hold;
        if 1.0 <= phase + step {
            let t = ((1.0 - phase) / step).clamp(0.0, 1.0);
            let new = self.prev + t * (x - self.prev);
            out = (1.0 - t) * new + self.hold * t;
            self.hold = new;
        }
        let b = ((1.0 - step) * -100.0 + 1.0).clamp(0.0, 1.0);
        let out = out + b * b * b.mul_add(-2.0, 3.0) * (x - out);
        self.prev = x;
        self.phase = frac(step + phase);
        out
    }
}

/// One Crush band.
pub struct CrushBand {
    sr: f32,
    bits: [Bits; 2],
    dec: [Decimator; 2],
    tone: [OnePole; 2],
    dc: [OnePole; 2],
    dc_offset: Smoother,
    jitter: Smoother,
    treble: Smoother,
}

/// The band parameters other than the waves.
#[derive(Clone, Copy, Debug)]
pub struct CrushParams {
    pub dc_offset: f32,
    pub jitter: f32,
    pub treble: f32,
    pub damp: bool,
    pub dither: bool,
}

impl CrushBand {
    pub fn new(sr: f32, p: CrushParams) -> Self {
        Self {
            sr,
            bits: [Bits::new(); 2],
            dec: [Decimator::new(); 2],
            tone: [OnePole::default(); 2],
            dc: [OnePole::default(); 2],
            dc_offset: Smoother::new(p.dc_offset, sr as f64, 0.0),
            jitter: Smoother::new(p.jitter, sr as f64, 0.0),
            treble: Smoother::new(p.treble, sr as f64, 0.0),
        }
    }

    /// `bits` and `dec` are the two section waves.
    pub fn process(&mut self, buf: &mut [[f32; 2]], bits: Wave, dec: Wave, p: CrushParams) {
        let n = buf.len();
        self.dc_offset.process(p.dc_offset, n);
        self.jitter.process(p.jitter, n);
        self.treble.process(p.treble, n);
        let active = match dec {
            Wave::Constant(w) => w < 1.0,
            Wave::PerSample(_) => true,
        };
        let tone_on = self.treble.ramping || self.treble.value != 1.0;
        let k_tone = tpt_k(350.0, self.sr);
        let k_dc = tpt_k(10.0, self.sr);
        for (i, f) in buf.iter_mut().enumerate() {
            let (wb, wd) = (bits.at(i), dec.at(i));
            let dc = self.dc_offset.at(i);
            let a = bits_amount(wb);
            let g = comp_gain(wb, dc);
            let step = decimate_step(wd, self.sr);
            let damp_g = if p.damp { pade_tan(step) } else { 0.0 };
            let j = self.jitter.at(i);
            let mix = wb * wd + self.treble.at(i) * (1.0 - wb * wd);
            for (c, v) in f.iter_mut().enumerate() {
                let mut x = self.bits[c].tick(*v, a, dc, p.dither) * g;
                x = self.dec[c].tick(x, step, j * j, damp_g, p.damp, active);
                if tone_on {
                    let y = self.tone[c].lp(x, k_tone);
                    x = y + (x - y) * mix;
                }
                *v = x - self.dc[c].lp(x, k_dc);
            }
        }
    }
}
