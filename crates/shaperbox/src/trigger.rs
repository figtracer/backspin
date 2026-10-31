//! The LFO trigger system (core.md section 5.5): the audio trigger detector
//! (`AudioTriggerProcessor`, 0x344178) and the trigger smoothers that ShaperBox runs with
//! `triggerSmoothing`: the Volume's cubic modulation ramp (0x461fd8), the Width and Noise
//! modulation crossfade (0x461fd8) and the audio click suppressor of the other shapers
//! (0x462d3c). The timing and event model is in [`crate::graph`].

use crate::bandsplit::F0;
use crate::shapers::tpt_k;
use crate::state::AudioTrigger;

/// `ln 1024` (`DAT_0130dc00`): the detector cutoffs are `F0 * 1024^x`.
const LN_1024: f64 = 6.931_471_805_599_453;
/// `sqrt 2` (`DAT_01328d28`): the band SVF damping is `2 sqrt 2`.
const SVF_R: f32 = std::f32::consts::SQRT_2;
/// The modulation smoother time of the Width and Noise shapers (0x3ac49ba6).
const PLAIN_TIME: f32 = 0.0015;
/// The cubic modulation smoother time of the Volume shaper (0x3bc49ba6).
const CUBIC_TIME: f32 = 0.006;
/// The audio click suppressor time (0x3ac49ba6).
const AUDIO_TIME: f32 = 0.0015;

/// How the modulation port of a shaper's sections is smoothed at a trigger (descriptor bytes
/// +0x41 and +0x42 of `DAT_01309610`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModSmoothing {
    None,
    /// Width, Noise: a 1.5 ms crossfade from the old value at the reset.
    Plain,
    /// Volume: a 6 ms Hermite ramp that ends at the reset (the shaper runs 6 ms late for it).
    Cubic,
}

/// The modulation smoothing of shaper `k`.
pub fn mod_smoothing(k: usize) -> ModSmoothing {
    match k {
        5 => ModSmoothing::Cubic,
        7 | 8 => ModSmoothing::Plain,
        _ => ModSmoothing::None,
    }
}

/// Whether shaper `k` runs the audio click suppressor at a trigger (descriptor byte +0x43):
/// Time, Crush, Filter, Pan, Reverb, Delay, Drive, Liquid and Shift.
pub fn audio_smoothed(k: usize) -> bool {
    matches!(k, 0..=4 | 6 | 9 | 11 | 13)
}

/// The Volume shaper's lookahead with `triggerSmoothing` and a free section: `int(0.006 sr)`
/// (0x539c38) plus the one sample the plugin adds on top [harness].
pub fn cubic_lookahead(sr: f32) -> usize {
    (sr as f64 * 0.006_000_000_052_154_064) as usize + 1
}

/// The audio trigger's lookahead (0x60c2ec): the onset detector's minimum length plus the
/// Trigger Shift delay.
pub fn audio_lookahead(sr: f32, p: &AudioTrigger) -> usize {
    let sr = sr as f64;
    let min_len = sr * if p.algorithm == 2 { 0.02 } else { 0.01 };
    min_len as usize + (p.delay as f64 * 0.008 * sr) as usize
}

// ---------------------------------------------------------------------------------------------
// Modulation smoothers.

/// `TriggerSmootherModulationCubic` (0x461fd8, state at +0x38 of the section runtime).
#[derive(Clone, Debug)]
pub struct CubicSmoother {
    len: f32,
    t: f32,
    prev: f32,
    slope: f32,
    p1: f32,
    m1: f32,
    c: [f32; 4],
    pending: bool,
    reset: bool,
}

impl CubicSmoother {
    pub fn new(sr: f32) -> Self {
        Self {
            len: sr * (1.0 / sr).max(CUBIC_TIME) + 0.5,
            t: 0.0,
            prev: 0.0,
            slope: 0.0,
            p1: 0.0,
            m1: 0.0,
            c: [0.0; 4],
            pending: false,
            reset: true,
        }
    }

    /// 0x461b6c: a trigger arrives 6 ms ahead; `p1` is the wave's value at phase 0 and `m1` its
    /// slope per sample.
    pub fn arm(&mut self, p1: f32, m1: f32) {
        self.p1 = p1;
        self.m1 = m1;
        self.pending = true;
    }

    /// A Hermite tangent limited to the chord `d` (monotone, same sign).
    fn limit(m: f32, d: f32) -> f32 {
        if m.abs() >= 1e-6 && (m >= 0.0 || d < 0.0) && (m <= 0.0 || d > 0.0) {
            let avg = (m + d) * 0.5;
            avg * (m * 3.0 / avg).min(d * 3.0 / avg).min(1.0)
        } else {
            0.0
        }
    }

    /// Smooths the port; returns whether it changed the block (a ramp is running).
    pub fn process(&mut self, buf: &mut [f32]) -> bool {
        let Some(&x0) = buf.first() else { return false };
        if self.reset {
            self.t = 0.0;
            self.prev = x0;
            self.slope = 0.0;
            self.reset = false;
        }
        if self.pending {
            self.t = 1.0;
            let (p0, p1) = (self.prev, self.p1);
            let d = p1 - p0;
            let (mut m0, mut m1) = (0.0, 0.0);
            if (p0 - p1).abs() >= 1e-6 {
                m0 = Self::limit(p0 - (p0 - self.slope), d);
                m1 = Self::limit(self.len.mul_add(self.m1, p1) - p1, d);
            }
            self.c = [
                d.mul_add(-2.0, m0 + m1),
                d.mul_add(3.0, m0 * -2.0) - m1,
                m0,
                p0,
            ];
            self.pending = false;
        }
        if self.t != 0.0 {
            for x in buf.iter_mut() {
                if self.t <= 0.0 {
                    break;
                }
                let u = 1.0 - self.t;
                let v = u.mul_add(
                    u.mul_add(u.mul_add(self.c[0], self.c[1]), self.c[2]),
                    self.c[3],
                );
                let v = self.t.mul_add(*x - v, v);
                self.slope = v - self.prev;
                self.prev = v;
                *x = v;
                self.t = (self.t - 1.0 / self.len).max(0.0);
            }
            true
        } else {
            let last = buf[buf.len() - 1];
            self.slope = last - self.prev;
            self.prev = last;
            false
        }
    }
}

/// `TriggerSmootherModulation` (0x461fd8, +0x28 of the section runtime): at the reset the port
/// crossfades linearly from its old value to the new signal over 1.5 ms.
#[derive(Clone, Debug)]
pub struct PlainSmoother {
    dec: f32,
    t: f32,
    prev: f32,
    start: f32,
    reset: bool,
}

impl PlainSmoother {
    pub fn new(sr: f32) -> Self {
        Self {
            dec: 1.0 / (sr * (1.0 / sr).max(PLAIN_TIME)),
            t: 0.0,
            prev: 0.0,
            start: 0.0,
            reset: true,
        }
    }

    /// 0x461794: the section was reset; `buf` is the port after the reset. Returns whether the
    /// block changed (a ramp is running).
    pub fn process(&mut self, buf: &mut [f32], triggered: bool) -> bool {
        let Some(&x0) = buf.first() else { return false };
        if self.reset {
            self.t = 0.0;
            self.prev = x0;
            self.reset = false;
        }
        if triggered && (x0 - self.prev).abs() > 1e-6 {
            self.t = 1.0;
            self.start = self.prev;
        }
        if self.t != 0.0 {
            for x in buf.iter_mut() {
                self.t = (self.t - self.dec).max(0.0);
                *x = self.t.mul_add(self.start - *x, *x);
                self.prev = *x;
            }
            true
        } else {
            self.prev = buf[buf.len() - 1];
            false
        }
    }
}

/// `TriggerSmootherAudio` (0x462d3c, +0x48 of the shaper runtime): at a trigger the shaper's
/// output crossfades over up to 1.5 ms from a low-pass that opens from 5 Hz to the full band,
/// so the jump at the reset does not click. Larger jumps ramp longer.
#[derive(Clone, Debug)]
pub struct AudioSmoother {
    sr: f32,
    t: [f32; 2],
    dec: [f32; 2],
    prev2: [f32; 2],
    prev: [f32; 2],
    lp_y: [f32; 2],
    lp_s: [f32; 2],
    reset: bool,
    triggered: bool,
    active: bool,
}

impl AudioSmoother {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            t: [0.0; 2],
            dec: [0.0; 2],
            prev2: [0.0; 2],
            prev: [0.0; 2],
            lp_y: [0.0; 2],
            lp_s: [0.0; 2],
            reset: true,
            triggered: false,
            active: false,
        }
    }

    /// A section of the shaper was reset (0x461794).
    pub fn trigger(&mut self) {
        self.triggered = true;
    }

    pub fn process(&mut self, buf: &mut [[f32; 2]]) {
        let Some(&x0) = buf.first() else { return };
        if self.reset {
            *self = Self {
                prev: x0,
                reset: false,
                ..Self::new(self.sr)
            };
            return;
        }
        let sr = self.sr;
        let fny = sr * 0.5 + -10.0;
        for f in buf.iter_mut() {
            if self.triggered {
                for (c, &x) in f.iter().enumerate() {
                    let d = x - self.prev[c];
                    let r = (d * d).mul_add(8.0, (d - (self.prev[c] - self.prev2[c])).abs() * 32.0);
                    let r = r.clamp(-3.0, 3.0);
                    // `r (27 + r^2) / (27 + 9 r^2)` in this site's fma order.
                    let r = (r * r.mul_add(r, 27.0)) / (r * r).mul_add(9.0, 27.0);
                    let r = r.clamp(-1.0, 1.0) * AUDIO_TIME;
                    self.dec[c] = 1.0 / (r.max(1.0 / sr) * sr);
                    self.t[c] = 1.0;
                    self.lp_y[c] = self.prev[c];
                    self.lp_s[c] = self.prev[c];
                }
                self.triggered = false;
                self.active = true;
            }
            let x = *f;
            if self.active {
                self.t = [
                    (self.t[0] - self.dec[0]).max(0.0),
                    (self.t[1] - self.dec[1]).max(0.0),
                ];
                if self.t[0].max(self.t[1]) == 0.0 {
                    self.active = false;
                } else {
                    for c in 0..2 {
                        let t = self.t[c];
                        let u = t.min(1.0);
                        let fc = (fny + -5.0) * (1.0 - t).mul_add(-(1.0 - t), 1.0);
                        let g = tpt_k(fny - fc, sr);
                        let v = (x[c] - self.lp_s[c]) * g;
                        let y = self.lp_s[c] + v;
                        self.lp_y[c] = y;
                        self.lp_s[c] = v + y;
                        let w = u * u * u * u.mul_add(6.0, -15.0).mul_add(u, 10.0);
                        f[c] = w.mul_add(y - x[c], x[c]);
                    }
                }
            }
            self.prev2 = self.prev;
            self.prev = *f;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The audio trigger detector.

/// A TPT one-pole stage with its state (`y = s + k (x - s); s = y + k (x - s)`).
#[derive(Clone, Copy, Debug, Default)]
struct Pole(f32);

impl Pole {
    fn lp(&mut self, x: f32, k: f32) -> f32 {
        let v = (x - self.0) * k;
        let y = self.0 + v;
        self.0 = v + y;
        y
    }
}

/// `FluxCell` (0x347748): an 8-stage asymmetric follower whose slow branch adapts its attack to the
/// ratio of the fast and slow envelopes.
#[derive(Clone, Copy, Debug, Default)]
struct FluxCell {
    s: [f32; 8],
}

impl FluxCell {
    fn tick(&mut self, x: f32, atk: f32, rel: f32, p: f32, final_mul: f32) -> f32 {
        let s = &mut self.s;
        let d = x.abs() - s[0];
        s[0] = atk.mul_add(d.max(0.0), d.min(0.0).mul_add(rel, s[0]));
        for i in 1..4 {
            let d = s[i - 1] - s[i];
            s[i] = rel.mul_add(d.min(0.0), s[i] + d.max(0.0));
        }
        let env = s[3];
        let ratio = env.max(1e-12) / s[7].max(1e-12);
        let c = (1.0 - (ratio * -std::f32::consts::PI).exp2()).clamp(0.0, 1.0);
        let k = c.mul_add(p + -0.5, 0.5);
        let d = env - s[4];
        s[4] = k.mul_add(d.max(0.0), s[4] + d.min(0.0));
        for i in 5..8 {
            let d = s[i - 1] - s[i];
            s[i] = k.mul_add(d.max(0.0), s[i] + d.min(0.0));
        }
        s[7].mul_add(final_mul, env.max(0.0))
    }

    /// The vector copy of the cell inside the 3- and 8-band flux (0x347220, 0x346cf0): the same
    /// follower with the vector code's operation order and its polynomial `exp2`.
    fn tick_band(&mut self, x: f32, atk: f32, rel: f32, p: f32) -> f32 {
        let s = &mut self.s;
        let d = x.abs() - s[0];
        s[0] = d.min(0.0).mul_add(rel, d.max(0.0).mul_add(atk, s[0]));
        for i in 1..4 {
            let d = s[i - 1] - s[i];
            s[i] = rel.mul_add(d.min(0.0), s[i] + d.max(0.0));
        }
        let env = s[3];
        let a = ((env.max(1e-12) / s[7].max(1e-12)) * -std::f32::consts::PI)
            .clamp(f32::from_bits(0xc2fd_ffff), f32::from_bits(0x4301_0000));
        let c = (1.0 - exp2_poly(a)).clamp(0.0, 1.0);
        let k = (p + -0.5).mul_add(c, 0.5);
        let d = env - s[4];
        s[4] = k.mul_add(d.max(0.0), s[4]) + d.min(0.0);
        for i in 5..8 {
            let d = s[i - 1] - s[i];
            s[i] = k.mul_add(d.max(0.0), s[i] + d.min(0.0));
        }
        s[7].mul_add(-1.5, env.max(0.0))
    }
}

/// The vector `exp2` of the band flux (0x347220): `2^a` as a cubic in the fraction times the
/// power of two of `int(a - 0.5)`.
fn exp2_poly(a: f32) -> f32 {
    let i = (a + -0.5) as i32;
    let f = a - i as f32;
    let poly = f.mul_add(
        f.mul_add(f.mul_add(0.078_024_52, 0.226_067_16), 0.695_833_56),
        0.999_925_2,
    );
    poly * f32::from_bits(((i + 0x7f) << 23) as u32)
}

/// A 2nd-order TPT SVF stage (Cytomic form) of the 3- and 8-band flux (0x346cf0).
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    g: f32,
    k: f32,
    a1: f32,
    s1: f32,
    s2: f32,
}

impl Svf {
    fn new(fc: f32, sr: f32) -> Self {
        let w = (fc / sr).clamp(0.0, 0.5) * std::f32::consts::PI;
        let w2 = w * w;
        let g = (w * w2.mul_add(w2.mul_add(w2 + -378.0, 17325.0), -135135.0))
            / w2.mul_add(w2.mul_add(w2.mul_add(28.0, -3150.0), 62370.0), -135135.0);
        let k = SVF_R.max(1e-9) * 2.0;
        Self {
            g,
            k,
            a1: 1.0 / (k.mul_add(g, g * g) + 1.0),
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// `(hp, lp)` of one sample.
    fn tick(&mut self, x: f32) -> (f32, f32) {
        let (g, k) = (self.g, self.k);
        let hp = self.a1 * (self.s1.mul_add(-g, self.s1.mul_add(-k, x)) - self.s2);
        let bp = g.mul_add(hp, self.s1);
        let lp = g.mul_add(bp, self.s2);
        self.s1 = g.mul_add(hp, bp);
        self.s2 = g.mul_add(bp, lp);
        (hp, lp)
    }
}

/// One band of the 3- and 8-band flux: two HP stages at the low edge, two LP stages at the high
/// edge per channel, a cell.
#[derive(Clone, Copy, Debug, Default)]
struct FluxBand {
    stages: [[Svf; 4]; 2],
    cell: FluxCell,
    rel: f32,
    p: f32,
}

impl FluxBand {
    fn band(stages: &mut [Svf; 4], x: f32) -> f32 {
        let (h1, _) = stages[0].tick(x);
        let (h2, _) = stages[1].tick(h1);
        let (_, l3) = stages[2].tick(h2);
        let (_, l4) = stages[3].tick(l3);
        l4
    }
}

/// `AudioTriggerOnsetDetector` (0x346ab0, prepare 0x34317c).
#[derive(Clone, Debug)]
struct Onset {
    min_len: i32,
    max_below: i32,
    delay: i32,
    ring: Vec<f32>,
    rd: usize,
    lp_k: f32,
    lp: Pole,
    active: bool,
    floor: f32,
    peak: f32,
    max_rise: f32,
    max_rel: f32,
    peak_amp: f32,
    n_since: i32,
    n_below: i32,
    cnt88: i32,
    cnt: i32,
    fired: bool,
    flux_thr: f32,
    amp_thr: f32,
}

impl Onset {
    fn new(sr: f32, p: &AudioTrigger) -> Self {
        let srd = sr as f64;
        Self {
            min_len: (srd * if p.algorithm == 2 { 0.02 } else { 0.01 }) as i32,
            max_below: (srd * 0.005) as i32,
            delay: (p.delay as f64 * 0.008 * srd) as i32,
            ring: vec![0.0; (srd * 0.028) as usize],
            rd: 0,
            lp_k: tpt_k(18000.0, sr),
            lp: Pole::default(),
            active: false,
            floor: 0.0,
            peak: 0.0,
            max_rise: 0.0,
            max_rel: 0.0,
            peak_amp: 0.0,
            n_since: 0,
            n_below: 0,
            cnt88: 0,
            cnt: 0,
            fired: false,
            flux_thr: flux_threshold(p.detail),
            amp_thr: p.threshold,
        }
    }

    fn place(&mut self, cnt: i32) {
        let len = self.ring.len() as i32;
        let idx = (self.min_len - cnt.min(self.min_len)) + self.rd as i32 + self.delay;
        self.ring[idx.rem_euclid(len) as usize] = self.peak_amp;
        self.fired = true;
    }

    /// One sample of flux `x` with the filtered audio pair; returns the trigger amplitude emitted
    /// at this sample (0 for none).
    fn tick(&mut self, x: f32, al: f32, ar: f32) -> f32 {
        let y = self.lp.lp(x, self.lp_k);
        let mut skip = false;
        let prev_rise;
        if self.active {
            prev_rise = self.max_rise;
        } else if x <= self.floor + self.floor {
            self.floor = self.floor.min(x);
            skip = true;
            prev_rise = 0.0;
        } else {
            self.active = true;
            self.fired = false;
            self.floor = self.floor.min(x);
            self.max_rise = 0.0;
            self.max_rel = 0.0;
            self.peak_amp = 0.0;
            self.n_since = 0;
            self.n_below = 0;
            self.cnt88 = 0;
            self.cnt = 0;
            self.peak = x;
            prev_rise = 0.0;
        }
        if !skip {
            let d = x - y;
            if prev_rise < d {
                self.cnt88 = 0;
            }
            let peak = self.peak;
            let rel = d / x.max(peak).mul_add(4.0, 1.0).max(1e-12);
            let cnt = if rel <= self.max_rel { self.cnt + 1 } else { 1 };
            self.max_rise = prev_rise.max(d);
            self.max_rel = self.max_rel.max(rel);
            self.peak_amp = self.peak_amp.max(al.abs().max(ar.abs()));
            self.cnt88 += 1;
            self.cnt = cnt;
            let n_since = self.n_since;
            self.n_since += 1;
            self.n_below = if x <= peak { self.n_below + 1 } else { 0 };
            let (flux_thr, amp_thr) = (self.flux_thr, self.amp_thr);
            if x < peak * 0.5 || self.max_below < self.n_below {
                self.active = false;
                self.floor = x;
                if flux_thr <= peak && !self.fired {
                    self.place(cnt);
                }
            } else if self.min_len <= n_since
                && amp_thr <= self.peak_amp
                && flux_thr <= peak
                && !self.fired
            {
                self.place(cnt);
            }
            self.peak = peak.max(x);
        }
        let out = self.ring[self.rd];
        self.ring[self.rd] = 0.0;
        self.rd = (self.rd + 1) % self.ring.len();
        out
    }
}

/// `AudioTriggerProcessor` (0x344178): input filters, flux envelope, onset detector. One per
/// processor; every section with `triggerSource` 1 listens to it.
#[derive(Clone, Debug)]
pub struct Detector {
    p: AudioTrigger,
    hp_k: Option<f32>,
    lp_k: Option<f32>,
    hp: [[Pole; 4]; 2],
    lp: [[Pole; 4]; 2],
    atk: f32,
    rel: f32,
    chain_k: f32,
    chain: [f32; 4],
    split_k: f32,
    split: [Pole; 2],
    cells: [FluxCell; 2],
    bands: Vec<FluxBand>,
    gain: f32,
    onset: Onset,
}

impl Detector {
    pub fn new(sr: f32, p: &AudioTrigger) -> Self {
        let inv = 1.0 / sr;
        let cutoff = |x: f32| (F0 * (LN_1024 * x as f64).exp()) as f32;
        let nb = match p.algorithm {
            1 => 3,
            2 => 8,
            _ => 0,
        };
        let bands = (0..nb)
            .map(|b| {
                let edge = |k: usize| cutoff(k as f32 / nb as f32).clamp(20.0, 20000.0);
                let (lo, hi) = (edge(b), edge(b + 1));
                let stage = [
                    Svf::new(lo, sr),
                    Svf::new(lo, sr),
                    Svf::new(hi, sr),
                    Svf::new(hi, sr),
                ];
                FluxBand {
                    stages: [stage, stage],
                    cell: FluxCell::default(),
                    rel: (lo * inv + 1.0) * 150.0 * inv,
                    p: (lo * inv + 1.0) * 200.0 * inv,
                }
            })
            .collect();
        Self {
            p: *p,
            hp_k: (p.hp_cutoff > 0.0).then(|| tpt_k(cutoff(p.hp_cutoff), sr)),
            lp_k: (p.lp_cutoff < 1.0).then(|| tpt_k(cutoff(p.lp_cutoff), sr)),
            hp: Default::default(),
            lp: Default::default(),
            atk: (10000.0 * inv).clamp(0.0, 0.5),
            rel: (200.0 * inv).clamp(0.0, 0.5),
            chain_k: (400.0 * inv).clamp(0.0, 0.5),
            chain: [0.0; 4],
            split_k: tpt_k(500.0, sr),
            split: [Pole::default(); 2],
            cells: [FluxCell::default(); 2],
            bands,
            gain: if p.algorithm == 2 { 4.5 } else { 2.0 },
            onset: Onset::new(sr, p),
        }
    }

    pub fn params(&self) -> &AudioTrigger {
        &self.p
    }

    /// The 4-stage instant-attack release chain.
    fn chain(&mut self, x: f32) -> f32 {
        let mut v = x;
        for s in &mut self.chain {
            let d = v - *s;
            *s = self.chain_k.mul_add(d.min(0.0), *s + d.max(0.0));
            v = *s;
        }
        v
    }

    /// Analyses one block of the undelayed input and appends the sample offsets of the triggers
    /// to `out` (an onset fires once; `out` must have room for one entry per sample).
    pub fn process(&mut self, input: &[[f32; 2]], out: &mut Vec<usize>) {
        if !self.p.active {
            return;
        }
        let thr = self.p.threshold;
        for (i, f) in input.iter().enumerate() {
            let (mut l, mut r) = (f[0], f[1]);
            if let Some(k) = self.hp_k {
                for s in 0..4 {
                    l -= self.hp[0][s].lp(l, k);
                    r -= self.hp[1][s].lp(r, k);
                }
            }
            if let Some(k) = self.lp_k {
                for s in 0..4 {
                    l = self.lp[0][s].lp(l, k);
                    r = self.lp[1][s].lp(r, k);
                }
            }
            let x = if self.bands.is_empty() {
                let yl = self.split[0].lp(l, self.split_k);
                let yr = self.split[1].lp(r, self.split_k);
                let low = yl.abs().max(yr.abs());
                let high = (l - yl).abs().max((r - yr).abs());
                let (atk, rel) = (self.atk, self.rel);
                let a = self.cells[0].tick(low, atk, rel, rel, -1.25);
                let b = self.cells[1].tick(high, atk, rel, rel, -1.25).max(0.0);
                self.chain(a.max(b))
            } else {
                // Groups of four bands (one vector), their lanes summed left to right.
                let mut acc = 0.0;
                let (atk, gain) = (self.atk, self.gain);
                for group in self.bands.chunks_mut(4) {
                    let mut sum = 0.0;
                    for (i, bd) in group.iter_mut().enumerate() {
                        let xl = FluxBand::band(&mut bd.stages[0], l);
                        let xr = FluxBand::band(&mut bd.stages[1], r);
                        let m = xl.abs().max(xr.abs());
                        let v = (bd.cell.tick_band(m, atk, bd.rel, bd.p) * gain).max(0.0);
                        sum = if i == 0 { v } else { sum + v };
                    }
                    acc += sum;
                }
                2.0 * self.chain(acc).max(0.0)
            };
            let v = self.onset.tick(x, l, r);
            if v > thr {
                out.push(i);
            }
        }
    }
}

/// The flux threshold from the Detail knob (0x345b60).
pub fn flux_threshold(detail: f32) -> f32 {
    let d = 1.0 - detail;
    let d2 = d * d;
    let d8 = d2 * d2 * (d2 * d2);
    (1.0 - detail * detail * detail)
        .mul_add(0.074_271_3, d8.mul_add(0.825_729, d2 * 0.1))
        .mul_add(0.999, 0.001)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookaheads_at_48k() {
        assert_eq!(cubic_lookahead(48000.0), 289);
        let mut p = AudioTrigger::default();
        assert_eq!(audio_lookahead(48000.0, &p), 672);
        p.algorithm = 2;
        p.delay = 0.0;
        assert_eq!(audio_lookahead(48000.0, &p), 960);
    }

    /// A kick train on the sidechain fires 4 samples after each onset in audio-aligned time
    /// (captures/exp_lfo/at_b512: the harness reset the LFO at input samples 12004, 36004,
    /// 62404, 91204, 115204 with the 672-sample lookahead removed).
    #[test]
    fn kick_train_triggers() {
        let sr = 48000.0f32;
        let kicks = [0.25, 0.75, 1.3, 1.9, 2.4];
        let n = 3 * 48000;
        // wavio.kick_train of the RE workspace: decaying 60 Hz bursts with a pitch sweep.
        let mut sc = vec![[0.0f32; 2]; n];
        for &k in &kicks {
            let start = (k * sr as f64) as usize;
            let len = ((0.4 * sr as f64) as usize).min(n - start);
            for (i, f) in sc[start..start + len].iter_mut().enumerate() {
                let t = i as f64 / sr as f64;
                let env = (-t / 0.08).exp();
                let ph = std::f64::consts::TAU * 60.0 * t * (1.0 + 2.0 * (-t / 0.02).exp());
                let v = (0.8 * env * ph.sin()) as f32;
                *f = [f[0] + v, f[1] + v];
            }
        }
        let p = AudioTrigger {
            active: true,
            ..AudioTrigger::default()
        };
        let mut d = Detector::new(sr, &p);
        let mut fires = Vec::new();
        for (b, chunk) in sc.chunks(512).enumerate() {
            let mut ev = Vec::with_capacity(512);
            d.process(chunk, &mut ev);
            fires.extend(ev.into_iter().map(|o| b * 512 + o));
        }
        let want: Vec<usize> = kicks
            .iter()
            .map(|k| (k * sr as f64) as usize + 4 + 672)
            .collect();
        assert_eq!(fires, want);
    }
}
