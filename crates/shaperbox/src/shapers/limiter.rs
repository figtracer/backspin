//! Limiter (`LimiterShaperModulation::process` 0x472994, `LookaheadLimiter::process` 0x2e9c1c;
//! shapers_c.md section 2): a lookahead brick-wall limiter whose threshold is the section wave.
//!
//! Per band and sample, in f32 ("dB" is `6 log2`, one unit is 6.02 dB): the input is scaled by
//! `1 / threshold`; a 3 kHz one-pole splits a transient share that shortens the attack
//! (`transientBoost` 1 and 2); the gain computer follows `6 log2 max(peak, 1)` with that attack
//! and an exponential release; a sliding maximum over the lookahead window smoothed by three
//! moving averages gives the gain, applied to the input delayed by the window; `transientBoost`
//! 1 adds a 0.2 ms second stage; the output is clamped to +-1. `autoRelease` runs two cascaded
//! gain states and a programme-dependent release and blends them. The threshold scaling is not
//! undone: the limiter's ceiling is +-1 in the scaled domain. The dry path of the band mix is
//! delayed by the lookahead ([`LimiterBand::delay_dry`]); the latency is not reported.
//!
//! ShaperBox 3.6.3 never runs this code: `LimiterShaperProcessor::processBlock` is empty in that
//! build (shapers_c.md section 0), so the plugin passes the band through. This port is
//! code-derived and cannot be checked against the plugin.

use super::Wave;

/// `DAT_01309fb0`: `1 - 2^((C / 3) / T)` is a one-pole coefficient reaching `1 - e^-2` in `T`
/// samples.
const C: f32 = f32::from_bits(0xc10a_740e);
/// `DAT_013072d0`, 2^-10: the floor of the peak envelope in the transient ratio.
const EPS: f32 = f32::from_bits(0x3a80_0000);
const PI_F: f32 = std::f32::consts::PI;
/// The second stage window: 0.2 ms.
const STAGE2_SECONDS: f32 = 0.0002;
/// The full lookahead: 6 ms.
const LOOKAHEAD_SECONDS: f32 = 0.006;

/// The band state of the Limiter shaper (`LimiterBandState`, shapers_c.md 2.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterParams {
    /// `lookahead` 0..1: the window is `round_up4(int(v 0.006 sr))` samples.
    pub lookahead: f32,
    /// `release` 0..1: the release time is `v^3` seconds.
    pub release: f32,
    /// `autoRelease`.
    pub auto_release: bool,
    /// `transientBoost` (`TransientBoost`): 0 instant attack, one stage; 1 transient-adaptive
    /// attack and the 0.2 ms second stage; 2 transient-adaptive attack, one stage.
    pub transient_boost: i32,
    /// `transientAttack` 0..1: the attack on transients is up to `v^3 10 ms`.
    pub transient_attack: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self {
            lookahead: 0.5,
            release: 0.464_158_9,
            auto_release: false,
            transient_boost: 1,
            transient_attack: 0.5,
        }
    }
}

/// `1 - 2^((C / 3) / t)` for `t` samples, 1 when `t <= 0` (0x2f2f14).
fn coef(t: f32) -> f32 {
    if t > 0.0 {
        1.0 - ((C / 3.0) / t).exp2()
    } else {
        1.0
    }
}

/// [`coef`] with the inline exp2 approximation of the per-sample attack paths (0x2e9c1c).
fn coef_fast(t: f32) -> f32 {
    if t <= 0.0 {
        return 1.0;
    }
    let u = (C / 3.0) / t;
    let fr0 = if u >= 0.0 { 0.0 } else { 1.0 };
    let v = u.max(-126.0);
    let fr = fr0 + (v - v.trunc());
    let e = fr.mul_add(
        -1.490_129_1,
        (v + 121.274_055) + 27.728_024 / (4.842_525_5 - fr),
    );
    1.0 - f32::from_bits((e * 8_388_608.0) as i32 as u32)
}

/// `x + ((-x) & 3)`.
fn round_up4(x: i32) -> i32 {
    x + ((-x) & 3)
}

/// The threshold wave to a linear gain (0x472994): `2^((60 w - 60) / 6)`, -60 .. 0 "dB".
pub fn threshold_gain(w: f32) -> f32 {
    (w.mul_add(60.0, -60.0) * 0.166_666_67).exp2()
}

/// The lookahead window in samples for a `lookahead` state value (0x2f35b8).
pub fn lookahead_samples(lookahead: f32, sr: f32) -> usize {
    round_up4((lookahead * LOOKAHEAD_SECONDS * sr) as i32 as f32 as i32) as usize
}

/// `u (u^2 + 27) / (9 u^2 + 27)`, the cubic soft clip of the autoRelease paths.
fn soft(u: f32) -> f32 {
    (u * u.mul_add(u, 27.0)) / (u * u).mul_add(9.0, 27.0)
}

/// 0x2f3430 / 0x2f3960 / 0x2f6d78: a sliding maximum over the last `n` pushed values (clamped
/// at 0) followed by three moving averages of `n/2 + 1`, `n/4 + 1` and `n/4 + 1` samples with
/// int64 accumulators of `trunc(v -2^31)`.
struct MaxSmoother {
    n: usize,
    hist: Vec<f32>,
    pos: usize,
    boxes: [BoxFilter; 3],
}

struct BoxFilter {
    buf: Vec<i64>,
    len: usize,
    pos: usize,
    acc: i64,
    scale: f64,
}

impl BoxFilter {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0; (len + 1).next_power_of_two()],
            len,
            pos: 0,
            acc: 0,
            scale: (1.0 / (len as f64 + 1.0)) * -4.656_612_873_077_393e-10,
        }
    }

    /// Pushes the fixed-point `q` and returns the average of the last `len + 1` values.
    fn push(&mut self, q: i64) -> f32 {
        let mask = self.buf.len() - 1;
        let old = self.buf[(self.pos + self.buf.len() - self.len - 1) & mask];
        self.acc = self.acc + q - old;
        self.buf[self.pos] = q;
        self.pos = (self.pos + 1) & mask;
        (self.scale * self.acc as f64) as f32
    }
}

impl MaxSmoother {
    fn new(n: usize) -> Self {
        let n = n.max(1);
        Self {
            n,
            hist: vec![0.0; n],
            pos: 0,
            boxes: [
                BoxFilter::new((n / 2).max(1)),
                BoxFilter::new((n / 4).max(1)),
                BoxFilter::new((n / 4).max(1)),
            ],
        }
    }

    fn push(&mut self, v: f32) -> f32 {
        self.hist[self.pos] = v.max(0.0);
        self.pos = (self.pos + 1) % self.n;
        let m = self.hist.iter().copied().fold(0.0f32, f32::max);
        let mut x = m;
        for b in &mut self.boxes {
            x = b.push((x * -2_147_483_648.0) as i64);
        }
        x
    }
}

/// One Limiter band.
pub struct LimiterBand {
    sr: f32,
    p: LimiterParams,
    /// The lookahead and second-stage windows in samples.
    l1: usize,
    l2: usize,
    /// Release coefficients: the user release, 1 s, 0.25 s, 0.1 s and 70 ms.
    rel: f32,
    rel1s: f32,
    c_quarter: f32,
    c_100ms: f32,
    c_70ms: f32,
    /// The 3 kHz one-pole coefficient.
    g3k: f32,
    s1: MaxSmoother,
    s2: MaxSmoother,
    ring1: Vec<[f32; 2]>,
    ring2: Vec<[f32; 2]>,
    w1: usize,
    w2: usize,
    /// Transient detector states: the low-pass per channel, the HF and peak envelopes, the
    /// smoothed ratio.
    lp: [f32; 2],
    env_a: f32,
    env_b: f32,
    sm: f32,
    /// Gain reduction states: the main one, and the second, the level follower and the pair of
    /// the autoRelease paths.
    gr: f32,
    gr2: f32,
    lvl: f32,
    pair: [f32; 2],
    dry: Vec<[f32; 2]>,
    dry_w: usize,
}

impl LimiterBand {
    pub fn new(sr: f32, p: LimiterParams) -> Self {
        let l1 = lookahead_samples(p.lookahead, sr);
        let l2 = round_up4((STAGE2_SECONDS * sr) as i32 as f32 as i32) as usize;
        let size = (l1.max(l2) + 0x200).next_power_of_two();
        let r = p.release;
        let t = (3000.0 / sr).min(0.4999);
        let w = if 3000.0 / sr >= 0.0 { t * PI_F } else { 0.0 };
        let tan = w.tan().max(0.0);
        let latency = l1
            + if p.transient_boost == 1 && l1 > 0 {
                l2
            } else {
                0
            };
        Self {
            sr,
            p,
            l1,
            l2,
            rel: coef(((r * r) * r) * sr),
            rel1s: coef(sr),
            c_quarter: coef(sr * 0.25),
            c_100ms: coef(sr * 0.1),
            c_70ms: coef(sr * 0.07),
            g3k: tan / (tan + 1.0),
            s1: MaxSmoother::new(l1),
            s2: MaxSmoother::new(l2),
            ring1: vec![[0.0; 2]; size],
            ring2: vec![[0.0; 2]; size],
            w1: 0,
            w2: 0,
            lp: [0.0; 2],
            env_a: 0.0,
            env_b: 0.0,
            sm: 0.0,
            gr: 0.0,
            gr2: 0.0,
            lvl: 0.0,
            pair: [0.0; 2],
            dry: vec![[0.0; 2]; (latency + 1).next_power_of_two()],
            dry_w: 0,
        }
    }

    /// The latency of the wet path in samples: the lookahead window, plus the second stage with
    /// `transientBoost` 1; 0 without lookahead.
    pub fn latency(&self) -> usize {
        self.l1
            + if self.p.transient_boost == 1 && self.l1 > 0 {
                self.l2
            } else {
                0
            }
    }

    /// Delays the band's dry signal by [`Self::latency`] so the band mix lines up.
    pub fn delay_dry(&mut self, dry: &mut [[f32; 2]]) {
        let lat = self.latency();
        let mask = self.dry.len() - 1;
        for f in dry.iter_mut() {
            self.dry[self.dry_w] = *f;
            *f = self.dry[(self.dry_w + self.dry.len() - lat) & mask];
            self.dry_w = (self.dry_w + 1) & mask;
        }
    }

    /// The transient detector: the attack coefficient of this sample, 1 (instant) without
    /// `transientBoost`.
    fn attack_coef(&mut self, x: [f32; 2]) -> f32 {
        if self.p.transient_boost == 0 {
            return 1.0;
        }
        let mut lp = [0.0f32; 2];
        for c in 0..2 {
            let v = (x[c] - self.lp[c]) * self.g3k;
            lp[c] = self.lp[c] + v;
            self.lp[c] = v + lp[c];
        }
        let hp = (x[1] - lp[1]).abs().max((x[0] - lp[0]).abs());
        let pk = x[1].abs().max(x[0].abs());
        let d = hp - self.env_a;
        self.env_a = self.c_100ms.mul_add(d.min(0.0), self.env_a + d.max(0.0));
        let d = pk - self.env_b;
        self.env_b = d
            .max(0.0)
            .mul_add(1.0, self.c_100ms.mul_add(d.min(0.0), self.env_b));
        let ratio = self.env_a / self.env_b.max(EPS);
        let d = ratio - self.sm;
        self.sm = d
            .max(0.0)
            .mul_add(self.c_70ms, 1.0f32.mul_add(d.min(0.0), self.sm));
        let t = ratio - self.sm;
        let t = if t >= 0.0 { t.min(1.0) } else { 0.0 };
        let ta = self.p.transient_attack;
        let t_att = self.sr * ta * ta * ta * t * t * t.mul_add(-2.0, 3.0) * 0.01;
        coef_fast(t_att)
    }

    /// The gain computer: the gain reduction in `6 log2` units handed to the smoother.
    fn gain_reduction(&mut self, l6: f32, c_att: f32) -> f32 {
        if !self.p.auto_release {
            let d = l6 - self.gr;
            let gr1 = c_att.mul_add(d.max(0.0), self.gr);
            self.gr = if d < 0.0 {
                gr1 + d.max(-(self.rel * gr1))
            } else {
                gr1
            };
            return self.gr;
        }
        // The programme level follower and the release it sets: 1 .. 33 ms.
        let step = if l6 <= 0.0 { 0.0 } else { 1.0 };
        let e = l6.mul_add(0.083_333_336, step) - self.lvl;
        self.lvl = e
            .min(0.0)
            .mul_add(self.c_quarter, e.max(0.0).mul_add(self.c_quarter, self.lvl));
        let q = (self.lvl * self.lvl).min(3.0);
        let w = soft(q).clamp(-1.0, 1.0);
        let c2 = coef_fast(self.sr * w.mul_add(0.032, 0.001));
        let follow = |s: f32, target: f32, rel: f32| {
            let d = target - s;
            let s1 = d.max(0.0).mul_add(c_att, s);
            if d < 0.0 { s1 + d.max(s1 * -rel) } else { s1 }
        };
        self.gr = follow(self.gr, l6, c2);
        self.gr2 = follow(self.gr2, self.gr, c2);
        self.pair[0] = follow(self.pair[0], self.gr2, c2);
        self.pair[1] = follow(self.pair[1], l6, self.rel1s);
        let sat = soft(self.lvl.clamp(-3.0, 3.0));
        let blend = if sat >= -1.0 {
            sat.min(1.0) * 0.75
        } else {
            -0.75
        };
        blend.mul_add(self.pair[1] - self.pair[0], self.pair[0])
    }

    /// Processes one band block in place up to the wet signal (before the band mix); `thr` is
    /// the threshold section wave.
    pub fn process(&mut self, buf: &mut [[f32; 2]], thr: Wave) {
        let mask1 = self.ring1.len() - 1;
        for (i, f) in buf.iter_mut().enumerate() {
            let inv = 1.0 / threshold_gain(thr.at(i));
            let x = [inv * f[0], inv * f[1]];
            let c_att = self.attack_coef(x);
            let l6 = x[1].abs().max(x[0].abs()).max(1.0).log2() * 6.0;
            let gr = self.gain_reduction(l6, c_att);
            let y = if self.l1 == 0 {
                let g = (gr * -0.166_666_67).exp2();
                [x[0] * g, x[1] * g]
            } else {
                self.ring1[self.w1] = x;
                self.w1 = (self.w1 + 1) & mask1;
                let g1 = (self.s1.push(gr) * -0.166_666_67).exp2();
                let d = self.ring1[(self.w1 + self.ring1.len() - (self.l1 + 1)) & mask1];
                let y1 = [g1 * d[0], g1 * d[1]];
                self.ring2[self.w2] = y1;
                self.w2 = (self.w2 + 1) & mask1;
                if self.p.transient_boost == 1 {
                    let over = y1[1].abs().max(y1[0].abs()).max(1.0).log2() * 6.0;
                    let g2 = (self.s2.push(over) * -0.166_666_67).exp2();
                    let d = self.ring2[(self.w2 + self.ring2.len() - (self.l2 + 1)) & mask1];
                    [g2 * d[0], g2 * d[1]]
                } else {
                    y1
                }
            };
            *f = [y[0].clamp(-1.0, 1.0), y[1].clamp(-1.0, 1.0)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficients_and_windows() {
        // About 1 - e^-2 after T samples (C / 3 is -2.8844, not 2 / log2 e); the 6 ms lookahead
        // at 48 kHz is 288 samples, the 0.2 ms stage 12 (int(9.6) rounded up to a multiple of 4).
        let c = coef(100.0);
        let mut s = 0.0f32;
        for _ in 0..100 {
            s += c * (1.0 - s);
        }
        assert!((s - (1.0 - (-2.0f32).exp())).abs() < 1e-3, "{s}");
        assert_eq!(coef(0.0), 1.0);
        assert!((coef_fast(100.0) - c).abs() < 1e-3);
        assert_eq!(lookahead_samples(1.0, 48000.0), 288);
        assert_eq!(lookahead_samples(0.5, 48000.0), 144);
        assert_eq!(lookahead_samples(0.0, 48000.0), 0);
        assert_eq!(round_up4((STAGE2_SECONDS * 48000.0) as i32), 12);
        assert_eq!(threshold_gain(1.0), 1.0);
        assert!((threshold_gain(0.0) - 0.000_976_562_5).abs() < 1e-9);
    }

    #[test]
    fn moving_average_boxes() {
        // A box of length 2 (len 1) averages the last two values.
        let mut b = BoxFilter::new(1);
        let q = |v: f32| (v * -2_147_483_648.0) as i64;
        assert!((b.push(q(1.0)) - 0.5).abs() < 1e-6);
        assert!((b.push(q(1.0)) - 1.0).abs() < 1e-6);
        assert!((b.push(q(0.0)) - 0.5).abs() < 1e-6);
        let mut s = MaxSmoother::new(4);
        for _ in 0..8 {
            s.push(2.0);
        }
        assert!((s.push(2.0) - 2.0).abs() < 1e-5);
        // The maximum holds for 4 pushes, then the averages decay over 2 + 1 + 1 more.
        let mut v = Vec::new();
        for _ in 0..12 {
            v.push(s.push(0.0));
        }
        assert!(v[2] > 1.99 && v[8] < 1e-6, "{v:?}");
    }
}
