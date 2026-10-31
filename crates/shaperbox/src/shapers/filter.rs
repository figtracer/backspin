//! Filter: the FilterWrapper types 1..20 at 1x (shapers_a.md section 6; wrapper process 0x380a38,
//! jump table 0x9e7450). "Clean" types are the float32 `ResonantOnePole` and `Chamberlin12Db`,
//! "Warm" types the double-precision `Korg<double>` two-pole with saturating feedback. Every f32
//! step mirrors one plugin instruction (`mul_add` is fmadd/fmsub), so the output is bit-exact
//! with `.amp/in/sb/data/sb_filter_exact.py` where libm agrees.

use super::Wave;
use crate::smoother::Smoother;

/// `DAT_01328cc0`: 20.6 Hz base, `f = E0 2^(10 c)`.
const E0: f64 = 20.601_722_307_054_37;
/// `DAT_0130dc00` = ln(1024).
const LN1024: f64 = 6.931_471_805_599_453;
/// The clamp of `w` before the Pade tangent (ResonantOnePole, Chamberlin).
const WMAX_SVF: u32 = 0x3fc8_f622;
/// The same clamp in the Korg wrappers.
const WMAX_KORG: u32 = 0x3fc9_0ed3;
/// The clamp of `tan(w)` as a double in the Korg wrappers.
const GMAX_KORG: f64 = 3.138_451_060_936_203_5;
/// ResonantOnePole `k = cbrtf(res) * 1.995`.
const K_RES1: u32 = 0x3fff_5c29;
const TWO3: u32 = 0x3f2a_aaab;
/// Chamberlin damping `d = (1 - cbrtf(res)) * 0.9975 + 0.0025`.
const D_A: u32 = 0x3f7f_5c29;
const D_B: u32 = 0x3b23_d70a;
/// Korg "safe" resonance clamp 0.949.
const RES_MAX: u32 = 0x3f72_f1aa;
const C099: u32 = 0x3f7d_70a4;
const C001: u32 = 0x3c23_d70a;
const C1139: u32 = 0x3f91_e4f7;
const CM0139: u32 = 0xbe0f_27bb;
const C0166: u32 = 0x3e29_fbe7;
/// The `s^14` weight of clipping curve 1.
const C0069: u32 = 0x3d8d_4fdf;
/// The Warm notch `k` limits for clipping curves 0 and 1.
const KMAX_NOTCH0: u32 = 0x3fff_df3b;
const KMAX_NOTCH1: f64 = 1.998_999_953_269_958_5;
/// The resonance scale of the second Korg stage of types 4, 9 and 13 (0.278).
const SCALE2: u32 = 0x3e8e_5604;
/// The drive-gain smoother cutoff, 11.46255 Hz.
const HZ_SMOOTH: u32 = 0x4137_669b;
const MINUS_2PI: u32 = 0xc0c9_0fdb;
/// The Korg dither generator seed (MS rand).
const LCG_SEED: u32 = 0x8da2_537c;
/// The constant resonance port of the second Chamberlin (`DAT_013078c0`).
const SVF2_RES: f32 = 0.2;

fn fl(bits: u32) -> f32 {
    f32::from_bits(bits)
}

/// `fcsel` pair: `x > 1 -> 1`, then `x < 0 -> 0` (NaN falls through as in the plugin).
fn clamp01(x: f32) -> f32 {
    let v = if x > 1.0 { 1.0 } else { x };
    if x < 0.0 { 0.0 } else { v }
}

/// The per-channel cutoffs from the stereoOffset port `s`: `x = 2s - 1`, `d = copysign(x^2 / 2, x)`.
fn stereo_cutoffs(c: f32, s: f32) -> (f32, f32) {
    let x = s.mul_add(2.0, -1.0);
    let d = ((x * x) * 0.5).copysign(x);
    (c + d, c - d)
}

/// The plugin's float32 Pade tangent; `w` is already clamped.
fn pade_tan(w: f32) -> f32 {
    let w2 = w * w;
    let s2 = w2 + -378.0;
    let s2 = w2.mul_add(s2, 17325.0);
    let s2 = w2.mul_add(s2, -135_135.0);
    let num = w * s2;
    let s3 = w2.mul_add(28.0, -3150.0);
    let s3 = w2.mul_add(s3, 62370.0);
    let den = w2.mul_add(s3, -135_135.0);
    num / den
}

/// ResonantOnePole / Chamberlin: `g = tan(pi f / (2 sr))`; `k = E0 pi / sr`.
fn svf_g(c: f32, k: f64) -> f32 {
    let w = (k * (LN1024 * clamp01(c) as f64).exp()) as f32;
    let w = w * 0.5;
    pade_tan(if w > fl(WMAX_SVF) { fl(WMAX_SVF) } else { w })
}

/// Types 0 and 5: `k = cbrtf(res) * 1.995` (no clamp).
fn one_pole_k(res: f32) -> f32 {
    res.cbrt() * fl(K_RES1)
}

/// Chamberlin damping; the notch wrapper 0x418844 inverts the resonance first.
fn svf_damping(res: f32, notch: bool) -> f32 {
    let r = if notch {
        let r = if res > 1.0 { 1.0 } else { res };
        if res < 0.0 { 1.0 } else { 1.0 - r }
    } else {
        clamp01(res)
    };
    (1.0 - r.cbrt()).mul_add(fl(D_A), fl(D_B))
}

/// The coefficients of a Korg wrapper.
#[derive(Clone, Copy, Debug, Default)]
struct KorgCoefs {
    g: f64,
    /// `1 / (g + 1)`.
    big_g: f64,
    /// `g G`.
    g_g: f64,
    /// The topology-specific fourth coefficient (object +0x98).
    c2: f64,
}

impl KorgCoefs {
    /// `w = f32(k2 2^(10 c))` with `c` clamped from above only, `g = tan(w)` widened.
    fn new<const T: u8>(c: f32, k2: f64) -> Self {
        let c = if c > 1.0 { 1.0 } else { c };
        let w = (k2 * (LN1024 * c as f64).exp()) as f32;
        let w = if w > fl(WMAX_KORG) { fl(WMAX_KORG) } else { w };
        let g = pade_tan(w) as f64;
        let g = if g > GMAX_KORG { GMAX_KORG } else { g };
        let big_g = 1.0 / (g + 1.0);
        let g_g = g * big_g;
        let c2 = match T {
            LP | HP => g_g * g_g,
            BP => (-g_g).mul_add(g_g, g_g),
            _ => g_g.mul_add(g_g, -g_g),
        };
        Self { g, big_g, g_g, c2 }
    }
}

/// The Korg resonance parameter `k` (object +0x78).
fn korg_k<const T: u8, const C: u8>(res: f32, scale: f32, safe: bool) -> f64 {
    if T == NOTCH {
        let s = clamp01((-res).mul_add(scale, 1.0)).sqrt();
        let k = (s + s) as f64;
        let kmax = if C == 0 {
            fl(KMAX_NOTCH0) as f64
        } else {
            KMAX_NOTCH1
        };
        return if k > kmax { kmax } else { k };
    }
    let r = if safe && res > fl(RES_MAX) {
        fl(RES_MAX)
    } else {
        res
    };
    let s0 = (r * fl(C099)).mul_add(scale, fl(C001));
    let s1 = if s0 > 1.0 { 1.0 } else { s0 };
    let s8 = if s0 < fl(C001) { fl(C001) } else { s1 };
    let c = s8.cbrt();
    let two_c = c + c;
    if C == 0 {
        let t = s8.mul_add(fl(C1139), fl(CM0139));
        let t = if t < 0.0 { 0.0 } else { t };
        let t = t * t;
        let t = t * t;
        let t = t * t;
        t.mul_add(fl(C0166), two_c) as f64
    } else {
        let t = s8 * s8;
        let t = s8 * t;
        let t = t * t;
        let t = s8 * t;
        let t = t * t;
        t.mul_add(fl(C0069), two_c) as f64
    }
}

/// The ResonantOnePole saturation gain `(d + (q - d) / 2) / d`, 1 for `d == 0`.
fn sat_gain_one_pole(d: f32) -> f32 {
    if d == 0.0 {
        return 1.0;
    }
    let p = cubic_clip(d);
    let q = if p > 1.0 { 1.0 } else { p };
    let q = q + q;
    let q = if p < -1.0 { -2.0 } else { q };
    (q - d).mul_add(0.5, d) / d
}

/// The Chamberlin saturation gain `(2 v - q) / v`, 1 for `v == 0`.
fn sat_gain_svf(v: f32) -> f32 {
    if v == 0.0 {
        return 1.0;
    }
    let p = cubic_clip(v);
    let q = if p > 1.0 { 1.0 } else { p };
    let q = q * -2.0;
    let q = if p < -1.0 { 2.0 } else { q };
    v.mul_add(2.0, q) / v
}

/// `p(u) = u (u^2 + 27) / (9 u^2 + 27)` of `u = clamp(d / 2, -3, 3)`.
fn cubic_clip(d: f32) -> f32 {
    let h = d * 0.5;
    let u = if h > 3.0 { 3.0 } else { h };
    let u = if h < -3.0 { -3.0 } else { u };
    let n = u * u.mul_add(u, 27.0);
    let m = (u * 9.0).mul_add(u, 27.0);
    n / m
}

/// Korg clipping curve 0 (soft).
fn korg_sat_soft(s: f64) -> f64 {
    let u = ((s + 0.08) * 1.2).abs();
    if u < 3.0 {
        let t = (u * 0.370_117_099_66).mul_add(u, 1.160_558_985_4);
        1.0 / t + 0.107_100_067_1
    } else {
        0.989_211_724_084_013_1 / u
    }
}

/// Korg clipping curve 1 (hard).
fn korg_sat_hard(s: f64) -> f64 {
    let t = s + 0.08;
    let u = t.abs();
    if u < 0.8 {
        return 1.0;
    }
    let q = -0.039_999_999_999_999_98 / (u + -0.600_000_000_000_000_1) + 1.0;
    (q / t).abs()
}

// Korg topologies.
const LP: u8 = 0;
const HP: u8 = 1;
const BP: u8 = 2;
const NOTCH: u8 = 3;

/// Chamberlin outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SvfOut {
    Lp,
    Hp,
    Bp,
    Notch,
    Peak,
    /// Type 19: band-pass stages, then `2 bp + x`.
    Peak2,
}

/// ResonantOnePole (0x428cc8 LP, 0x427568 HP): three TPT one-poles per channel, two passes per
/// sample with the same input.
#[derive(Clone, Copy, Debug, Default)]
struct OnePoleRes {
    st: [[f32; 3]; 2],
}

impl OnePoleRes {
    fn pass(st: &mut [f32; 3], x: f32, g: f32, k: f32) -> (f32, f32) {
        let [a, b, c] = *st;
        let r = sat_gain_one_pole(b - a);
        let big_g = 1.0 / (g + 1.0);
        let g2_big = big_g * big_g;
        let g2 = g * g;
        let t = r * k;
        let u = (-g).mul_add(g2_big, big_g);
        let u = u.mul_add(a, -(big_g * b));
        let num = t.mul_add(u, x);
        let v = g2.mul_add(g2_big, -(big_g * g)) * k;
        let y = num / v.mul_add(r, 1.0);
        let ya = big_g * g.mul_add(y, a);
        let yb = big_g * g.mul_add(ya, b);
        let yc = big_g * g.mul_add(x, c);
        let gg = g + g;
        *st = [
            gg.mul_add(y - ya, a),
            gg.mul_add(ya - yb, b),
            gg.mul_add(x - yc, c),
        ];
        (ya, yc)
    }

    #[inline]
    fn tick(&mut self, ch: usize, x: f32, g: f32, k: f32, hp: bool) -> f32 {
        let st = &mut self.st[ch];
        Self::pass(st, x, g, k);
        let (ya, yc) = Self::pass(st, x, g, k);
        let out = (ya - yc).mul_add(fl(TWO3), yc);
        if hp { x - out } else { out }
    }
}

/// Chamberlin12Db: a TPT state-variable filter, two passes per sample, with the saturation gain
/// on the band-pass state. State per channel: hp, bp, lp, s1, s2.
#[derive(Clone, Copy, Debug, Default)]
struct Chamberlin {
    st: [[f32; 5]; 2],
}

impl Chamberlin {
    #[inline]
    fn tick(&mut self, ch: usize, x: f32, g: f32, d: f32, out: SvfOut) -> f32 {
        let [_, bp0, _, s1, s2] = self.st[ch];
        let dd = d + d;
        let a = (-g - dd) + 2.0;
        let d0 = g.mul_add(g, (dd + -2.0) * g) + 1.0;
        let g2 = g + g;
        let r = sat_gain_svf(bp0);
        let hp1 = r.mul_add(-2.0, a).mul_add(s1, x - s2) / g2.mul_add(r, d0);
        let v1 = hp1 * g;
        let bp1 = s1 + v1;
        let v2 = bp1 * g;
        let lp1 = s2 + v2;
        let (s1, s2) = (v1 + bp1, v2 + lp1);
        let r = sat_gain_svf(bp1);
        let hp2 = r.mul_add(-2.0, a).mul_add(s1, x - s2) / g2.mul_add(r, d0);
        let v1 = hp2 * g;
        let bp2 = s1 + v1;
        let v2 = bp2 * g;
        let lp2 = s2 + v2;
        self.st[ch] = [hp2, bp2, lp2, v1 + bp2, v2 + lp2];
        match out {
            SvfOut::Lp => lp2,
            SvfOut::Bp | SvfOut::Peak2 => bp2,
            SvfOut::Peak => x + bp2,
            SvfOut::Hp => ((hp1 + 0.0) + hp2) * 0.5,
            SvfOut::Notch => (((lp1 + hp1) + 0.0) + (lp2 + hp2)) * 0.5,
        }
    }
}

/// Korg<double>: two cascaded TPT one-poles with zero-delay resonance feedback `k sat(s2)`, two
/// passes per sample, in double. The input is scaled by the smoothed drive gain and the output
/// divided by it.
#[derive(Clone, Copy, Debug)]
struct Korg {
    scale: f32,
    coef: f32,
    first: bool,
    gain: [f32; 3],
    /// s1, s2 of L, then of R.
    s: [f64; 4],
    lcg: u32,
}

impl Korg {
    fn new(sr: f64, scale: f32) -> Self {
        let s0 = fl(HZ_SMOOTH) / sr as f32;
        Self {
            scale,
            coef: 1.0 - (s0 * fl(MINUS_2PI)).exp(),
            first: true,
            gain: [0.0; 3],
            s: [0.0; 4],
            lcg: LCG_SEED,
        }
    }

    /// The three one-pole drive-gain smoother; the first call starts at the target.
    fn gain(&mut self, target: f32) -> f32 {
        let c = self.coef;
        let g = &mut self.gain;
        if self.first {
            self.first = false;
            *g = [target; 3];
        } else {
            g[0] = (target - g[0]).mul_add(c, g[0]);
            g[1] = (g[0] - g[1]).mul_add(c, g[1]);
            g[2] = (g[1] - g[2]).mul_add(c, g[2]);
        }
        g[2]
    }

    /// One draw of the dither: `1e-12 lcg()` when `k` is above the threshold, else 0.
    fn noise<const C: u8>(&mut self, k: f64) -> f64 {
        let thr = if C == 0 { 2.069 } else { 2.0 };
        let d = if k > thr { 1e-12 } else { 0.0 };
        self.lcg = self.lcg.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        d * ((self.lcg as i32) as f64 / 2_147_483_648.0) as f32 as f64
    }

    #[inline]
    fn tick<const T: u8, const C: u8>(
        &mut self,
        x: [f32; 2],
        co: &[KorgCoefs; 2],
        k: f64,
        target: f32,
    ) -> [f32; 2] {
        let gain = self.gain(target);
        let n = self.noise::<C>(k);
        let inv = 1.0 / gain;
        let [sl1, sl2, sr1, sr2] = &mut self.s;
        let yl = korg_channel::<T, C>(n + (gain * x[0]) as f64, &co[0], k, sl1, sl2);
        let yr = korg_channel::<T, C>(n + (gain * x[1]) as f64, &co[1], k, sr1, sr2);
        [yl as f32 * inv, yr as f32 * inv]
    }
}

/// One channel of a Korg kernel (0x42853c lp, 0x426d98 hp, 0x425a3c bp, 0x4246f0 notch and the
/// hard-clip variants), both passes.
#[inline(always)]
fn korg_channel<const T: u8, const C: u8>(
    x: f64,
    co: &KorgCoefs,
    k: f64,
    s1: &mut f64,
    s2: &mut f64,
) -> f64 {
    let KorgCoefs { g, big_g, g_g, c2 } = *co;
    let sat = |s: f64| {
        if C == 0 {
            korg_sat_soft(s)
        } else {
            korg_sat_hard(s)
        }
    };
    let num = match T {
        HP => g_g.mul_add(-2.0, c2) + 1.0,
        NOTCH => c2.mul_add(2.0, 1.0),
        _ => c2,
    };
    let dd = c2 - g_g;
    let den = |kk: f64| match T {
        BP => (-kk).mul_add(c2, 1.0),
        NOTCH => c2.mul_add(kk, 1.0),
        _ => dd.mul_add(kk, 1.0),
    };
    let s2_next = |y: f64, s2: f64| match T {
        HP | NOTCH => (y - x).mul_add(2.0, -s2),
        _ => y.mul_add(2.0, -s2),
    };
    let v = |kk: f64, y: f64, s1: f64| match T {
        BP => big_g * (-g).mul_add(kk.mul_add(y, x), s1),
        NOTCH => big_g * g.mul_add(x.mul_add(2.0, -(kk * y)), s1),
        _ => big_g * g.mul_add((-kk).mul_add(y, x), s1),
    };
    let kk1 = k * sat(*s2);
    let y1 = num.mul_add(x, big_g * g_g.mul_add(*s1, *s2)) / den(kk1);
    *s2 = s2_next(y1, *s2);
    let kk2 = k * sat(*s2);
    *s1 = v(kk1, y1, *s1).mul_add(2.0, -*s1);
    let y2 = num.mul_add(x, big_g * g_g.mul_add(*s1, *s2)) / den(kk2);
    *s1 = v(kk2, y2, *s1).mul_add(2.0, -*s1);
    *s2 = s2_next(y2, *s2);
    y2
}

/// The sub-filters of one type.
#[derive(Clone, Debug)]
enum Kernel {
    OnePole {
        hp: bool,
        f: OnePoleRes,
    },
    Svf {
        out: SvfOut,
        notch: bool,
        /// The resonance of the second stage: the constant 0.2 port, or `None` for the wave.
        res2: Option<f32>,
        f: Vec<Chamberlin>,
    },
    Korg {
        topology: u8,
        clip: u8,
        safe: bool,
        target: f32,
        f: Vec<Korg>,
    },
}

/// One band of the Filter shaper.
#[derive(Clone, Debug)]
pub struct FilterBand {
    /// `E0 pi / sr` (FilterAudioMath+0x38).
    k: f64,
    kernel: Kernel,
}

impl FilterBand {
    /// A band of `typ` (`typeWithoutBypass`, 0..19) with the plain state values `drive`
    /// ("Res. Dist"), `safe` (`safeResonance`) and `clip` (`clippingCurve`).
    pub fn new(sr: f64, typ: i32, drive: f32, safe: bool, clip: i32) -> Self {
        let clip = u8::from(clip != 0);
        let korg = |topology: u8, stages: usize| {
            let scale2 = if topology == NOTCH { 1.0 } else { fl(SCALE2) };
            let d2 = drive * drive;
            Kernel::Korg {
                topology,
                clip,
                safe,
                target: if clip == 0 {
                    d2.mul_add(15.5, 0.5)
                } else {
                    d2.mul_add(15.0, 1.0)
                },
                f: (0..stages)
                    .map(|i| Korg::new(sr, if i == 0 { 1.0 } else { scale2 }))
                    .collect(),
            }
        };
        let svf = |out: SvfOut, stages: usize| Kernel::Svf {
            out,
            notch: out == SvfOut::Notch,
            res2: (typ != 16).then_some(SVF2_RES),
            f: vec![Chamberlin::default(); stages],
        };
        let kernel = match typ {
            0 | 5 => Kernel::OnePole {
                hp: typ == 5,
                f: OnePoleRes::default(),
            },
            1 => svf(SvfOut::Lp, 1),
            3 => svf(SvfOut::Lp, 2),
            6 => svf(SvfOut::Hp, 1),
            8 => svf(SvfOut::Hp, 2),
            10 => svf(SvfOut::Bp, 1),
            12 => svf(SvfOut::Bp, 2),
            14 => svf(SvfOut::Notch, 1),
            16 => svf(SvfOut::Notch, 2),
            18 => svf(SvfOut::Peak, 1),
            19 => svf(SvfOut::Peak2, 2),
            4 => korg(LP, 2),
            7 => korg(HP, 1),
            9 => korg(HP, 2),
            11 => korg(BP, 1),
            13 => korg(BP, 2),
            15 => korg(NOTCH, 1),
            17 => korg(NOTCH, 2),
            _ => korg(LP, 1),
        };
        Self {
            k: E0 * std::f64::consts::PI / sr,
            kernel,
        }
    }

    /// Filters one block in place. `cutoff` and `resonance` are the section waves, `stereo_offset`
    /// the smoothed stereoOffset port. Coefficients are recomputed per sample only while a port
    /// moves; the plugin's constant-wave fast paths give the same bits.
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        cutoff: Wave,
        resonance: Wave,
        stereo_offset: &Smoother,
    ) {
        let varying = !matches!(cutoff, Wave::Constant(_))
            || !matches!(resonance, Wave::Constant(_))
            || stereo_offset.ramping;
        let cutoffs = |i: usize| {
            let c = cutoff.at(i);
            let s = stereo_offset.at(i);
            if s == 0.5 {
                (c, c, true)
            } else {
                let (l, r) = stereo_cutoffs(c, s);
                (l, r, false)
            }
        };
        let k = self.k;
        match &mut self.kernel {
            Kernel::OnePole { hp, f } => {
                let hp = *hp;
                let coefs = |i: usize| {
                    let (cl, cr, _) = cutoffs(i);
                    (svf_g(cl, k), svf_g(cr, k), one_pole_k(resonance.at(i)))
                };
                let mut co = coefs(0);
                for (i, s) in buf.iter_mut().enumerate() {
                    if varying {
                        co = coefs(i);
                    }
                    let (gl, gr, kk) = co;
                    *s = [f.tick(0, s[0], gl, kk, hp), f.tick(1, s[1], gr, kk, hp)];
                }
            }
            Kernel::Svf {
                out,
                notch,
                res2,
                f,
            } => {
                let (out, notch, res2) = (*out, *notch, *res2);
                let coefs = |i: usize| {
                    let (cl, cr, _) = cutoffs(i);
                    let r = resonance.at(i);
                    let d1 = svf_damping(r, notch);
                    let d2 = svf_damping(res2.unwrap_or(r), notch);
                    (svf_g(cl, k), svf_g(cr, k), [d1, d2])
                };
                let mut co = coefs(0);
                for (i, s) in buf.iter_mut().enumerate() {
                    if varying {
                        co = coefs(i);
                    }
                    let (gl, gr, d) = co;
                    let (mut a, mut b) = (s[0], s[1]);
                    for (st, &d) in f.iter_mut().zip(&d) {
                        a = st.tick(0, a, gl, d, out);
                        b = st.tick(1, b, gr, d, out);
                    }
                    *s = if out == SvfOut::Peak2 {
                        [(a + a) + s[0], (b + b) + s[1]]
                    } else {
                        [a, b]
                    };
                }
            }
            Kernel::Korg {
                topology,
                clip,
                safe,
                target,
                f,
            } => {
                let (safe, target) = (*safe, *target);
                match (*topology, *clip) {
                    (LP, 0) => {
                        run_korg::<LP, 0>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (LP, _) => {
                        run_korg::<LP, 1>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (HP, 0) => {
                        run_korg::<HP, 0>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (HP, _) => {
                        run_korg::<HP, 1>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (BP, 0) => {
                        run_korg::<BP, 0>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (BP, _) => {
                        run_korg::<BP, 1>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    (_, 0) => {
                        run_korg::<NOTCH, 0>(buf, f, k, safe, target, varying, cutoffs, resonance)
                    }
                    _ => run_korg::<NOTCH, 1>(buf, f, k, safe, target, varying, cutoffs, resonance),
                }
            }
        }
    }
}

/// The block loop of the Warm types, one monomorphised copy per topology and clipping curve.
#[allow(clippy::too_many_arguments)]
fn run_korg<const T: u8, const C: u8>(
    buf: &mut [[f32; 2]],
    f: &mut [Korg],
    k: f64,
    safe: bool,
    target: f32,
    varying: bool,
    cutoffs: impl Fn(usize) -> (f32, f32, bool),
    resonance: Wave,
) {
    let k2 = k * 0.5;
    let scales = [f[0].scale, f.get(1).map_or(1.0, |s| s.scale)];
    let coefs = |i: usize| {
        let (cl, cr, same) = cutoffs(i);
        let l = KorgCoefs::new::<T>(cl, k2);
        let r = if same { l } else { KorgCoefs::new::<T>(cr, k2) };
        let res = resonance.at(i);
        ([l, r], scales.map(|scale| korg_k::<T, C>(res, scale, safe)))
    };
    let mut co = coefs(0);
    for (i, s) in buf.iter_mut().enumerate() {
        if varying {
            co = coefs(i);
        }
        let (c, kk) = &co;
        let mut x = *s;
        for (st, &kk) in f.iter_mut().zip(kk) {
            x = st.tick::<T, C>(x, c, kk, target);
        }
        *s = x;
    }
}

/// The 69 -> 70 preset migration (`FUN_0068fd68`): the new `y` of a resonance wave point of a
/// Warm type. Types 2, 4, 7, 9, 11, 13 map through the new resonance curve, the notch types 15
/// and 17 through `1 - (1 - w)^4`; other types keep `y`.
pub fn migrate_resonance_y(y: f32, typ: i32) -> f32 {
    let w = y.mul_add(0.5, 0.5);
    let y2 = match typ {
        2 | 4 | 7 | 9 | 11 | 13 => {
            let s0 = w.powf(0.2) * f32::from_bits(0x4002_e148);
            let t = w.mul_add(fl(C1139), fl(CM0139));
            let t = if t < 0.0 { 0.0 } else { t };
            let t = t * t;
            let t = t * t;
            let t = t * t;
            let s0 = t.mul_add(f32::from_bits(0xbe29_fbe7), s0) * 0.5;
            let s0 = s0 * (s0 * s0);
            s0.mul_add(2.0, -1.0)
        }
        15 | 17 => {
            let t = clamp01(1.0 - w);
            let t = t * t;
            (-t).mul_add(t, 1.0).mul_add(2.0, -1.0)
        }
        _ => return y,
    };
    let y2 = if y2 > 1.0 { 1.0 } else { y2 };
    if y2 < -1.0 { -1.0 } else { y2 }
}
