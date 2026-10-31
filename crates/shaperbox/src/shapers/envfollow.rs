//! The ShaperBox envelope follower of a section (`ShaperBoxEnvelopeFollowerState`; core.md 5.4,
//! shapers_b.md section 7; models `.amp/in/sb/data/sb_envfollow_exact.py` and
//! `sb_dynamics_exact.py`).
//!
//! Every `SectionEnvState` section has one follower per band. The Dynamics shaper builds the
//! `Analog` core (`FUN_00348ee8`) and applies its output as a gain (`super::dynamics`); every other
//! shaper builds the `Proportional` core (`FUN_0034bdc8`) and merges its output into the section's
//! LFO port ([`SectionFollower`]). Both read the detector input through the same selection
//! (`FUN_00351544` / `FUN_0035123c`) and the same free filter (`FUN_0034fdc4`). All arithmetic is
//! f32 with the fused multiply-adds of the binary; run it under [`crate::ftz::FlushToZero`].

use super::{Wave, tpt_k};
use crate::smoother::Smoother;

/// `DAT_01328cc0`, the frequency of a parameter value of 0.
const F0: f64 = 20.601_722_307_054_37;
/// `DAT_0130dc00` = ln(1024).
const LN1024: f64 = 6.931_471_805_599_453;
/// 2^-15, the level floor of the Analog core.
const MIN_LEVEL: f32 = f32::from_bits(0x3800_0000);
/// 1/6 as the binary stores it.
const SIXTH: f32 = 0.166_666_67;

/// The values of a `ShaperBoxEnvelopeFollowerState` (ids and init defaults of core.md 5.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvParams {
    /// `active`.
    pub active: bool,
    /// `mode`: 0 adds to the port, 1 scales it (no effect in Dynamics).
    pub mode: i32,
    /// `useSidechain`.
    pub use_sidechain: bool,
    /// `filterSetupIsFree`.
    pub filter_setup_is_free: bool,
    /// `hpCutoff`.
    pub hp_cutoff: f32,
    /// `lpCutoff`.
    pub lp_cutoff: f32,
    /// `depth`: 0..2 (no effect in Dynamics).
    pub depth: i32,
    /// `threshold`.
    pub threshold: f32,
    /// `amount`.
    pub amount: f32,
    /// `attack`.
    pub attack: f32,
    /// `hold`.
    pub hold: f32,
    /// `release`.
    pub release: f32,
    /// `autoRelease`.
    pub auto_release: bool,
    /// `knee`: has no effect anywhere.
    pub knee: f32,
    /// `offset` (no effect in Dynamics).
    pub offset: f32,
    /// `makeup` (Dynamics only).
    pub makeup: f32,
    /// `autoMakeup` (Dynamics only).
    pub auto_makeup: bool,
}

impl Default for EnvParams {
    fn default() -> Self {
        Self {
            active: false,
            mode: 0,
            use_sidechain: false,
            filter_setup_is_free: false,
            hp_cutoff: 0.0,
            lp_cutoff: 1.0,
            depth: 0,
            threshold: 0.0,
            amount: 0.5,
            attack: 0.1,
            hold: 0.0,
            release: 0.2,
            auto_release: false,
            knee: 0.5,
            offset: 0.5,
            makeup: 0.0,
            auto_makeup: false,
        }
    }
}

/// The signals a follower can listen to besides its band audio, aligned with the band block.
#[derive(Clone, Copy, Debug)]
pub struct EnvSources<'a> {
    /// The unsplit shaper input (the free filter's source).
    pub full: &'a [[f32; 2]],
    /// Band k of the sidechain bus, split with the shaper's own crossover (`useSidechain`).
    /// `None` when the host gives no sidechain bus: the detector then hears silence.
    pub sidechain_band: Option<&'a [[f32; 2]]>,
    /// The unsplit sidechain bus, the free filter's source with `useSidechain` and
    /// `filterSetupIsFree` both on.
    pub sidechain_full: Option<&'a [[f32; 2]]>,
}

impl<'a> EnvSources<'a> {
    /// Sources without a sidechain bus.
    pub fn new(full: &'a [[f32; 2]]) -> Self {
        Self {
            full,
            sidechain_band: None,
            sidechain_full: None,
        }
    }

    /// The samples `a..b` of every source.
    pub fn range(&self, a: usize, b: usize) -> Self {
        Self {
            full: &self.full[a..b],
            sidechain_band: self.sidechain_band.map(|s| &s[a..b]),
            sidechain_full: self.sidechain_full.map(|s| &s[a..b]),
        }
    }
}

/// `u (u^2 + 27) / (9 u^2 + 27)` in the binary's fma order (`u` already in [-3, 3]).
pub fn sat_cubic(u: f32) -> f32 {
    (u * u.mul_add(u, 27.0)) / (u * 9.0).mul_add(u, 27.0)
}

/// The attack state to seconds: `a^3 0.19999 + 1e-5` (10 us .. 0.2 s).
pub fn attack_seconds(a: f32) -> f32 {
    (a * a * a).mul_add(0.199_99, 1e-5)
}

/// The hold state to seconds: `h^2 0.5`.
pub fn hold_seconds(h: f32) -> f32 {
    (h * h).mul_add(0.5, 0.0)
}

/// The release state to seconds: `exp(-1.89712 + 2.6390572 r) - 0.1` (0.05 .. 2 s) for the
/// Dynamics shaper, `exp(-2.207275 + 4.5198107 r) - 0.1` (0.01 .. 10 s) for the other sections.
pub fn release_seconds(r: f32, dynamics: bool) -> f32 {
    if dynamics {
        r.mul_add(2.639_057_2, -1.897_12).exp() + -0.1
    } else {
        r.mul_add(4.519_810_7, -2.207_275).exp() + -0.1
    }
}

/// The Analog core coefficient `1 - 2^((-9.518667/6) / (t sr))` (`FUN_0034ba84`); `None` when
/// `t sr <= 0`, which keeps the old value.
pub fn analog_coef(t: f32, sr: f32) -> Option<f32> {
    let ts = t * sr;
    (ts > 0.0).then(|| 1.0 - ((-9.518_667_f64 / 6.0) as f32 / ts).exp2())
}

/// The Proportional core coefficient `1 - 2^((-8.653334/3) / (t sr))` (`FUN_0034db5c`); 1 when
/// `t sr <= 0`.
pub fn prop_coef(t: f32, sr: f32) -> f32 {
    let ts = t * sr;
    if ts > 0.0 {
        1.0 - ((-8.653_334_f64 / 3.0) as f32 / ts).exp2()
    } else {
        1.0
    }
}

/// The threshold state in units of 6 log2 (6.0206 dB): `60 t - 60`.
pub fn threshold_cu(t: f32) -> f32 {
    t.mul_add(60.0, -60.0)
}

/// The amount state to the Analog core ratio `min(0.99, 1 - a)`, computed as
/// `1 - 1/(1/(1 - r))`.
pub fn ratio_from_amount(a: f32) -> f32 {
    let r = (1.0 - a).min(0.99);
    1.0 - 1.0 / (1.0 / (1.0 - r))
}

/// The maximum over the last `n` pushed values (`FUN_002f3430` / `FUN_002f6fd4` /
/// `FUN_0030ec2c`). Pushed values are clamped to `>= 0` and history never written reads as 0.
/// The plugin keeps a binary tree over a ring; this keeps a monotonic deque, which gives the same
/// maximum in O(1) per sample, and rebuilds it from the ring when `n` changes.
#[derive(Clone, Debug)]
pub struct SlidingMax {
    buf: Vec<f32>,
    mask: u64,
    /// The time of the newest value.
    t: u64,
    /// Times of the deque, a ring over `dq_buf`.
    dq_buf: Vec<u64>,
    head: usize,
    len: usize,
    window: usize,
}

impl SlidingMax {
    /// A window of up to `capacity` values (the ring holds `2^k >= 2 capacity`).
    pub fn new(capacity: usize) -> Self {
        let mut size = 1;
        while size < capacity * 2 {
            size *= 2;
        }
        Self {
            buf: vec![0.0; size],
            mask: size as u64 - 1,
            t: size as u64,
            dq_buf: vec![0; size],
            head: 0,
            len: 0,
            window: 0,
        }
    }

    fn insert(&mut self, t: u64) {
        let m = self.dq_buf.len() - 1;
        let v = self.buf[(t & self.mask) as usize];
        while self.len > 0 {
            let back = self.dq_buf[(self.head + self.len - 1) & m];
            if self.buf[(back & self.mask) as usize] <= v {
                self.len -= 1;
            } else {
                break;
            }
        }
        self.dq_buf[(self.head + self.len) & m] = t;
        self.len += 1;
    }

    /// Pushes `v` and returns the maximum of the last `n` values (`n >= 1`).
    pub fn push_max(&mut self, v: f32, n: usize) -> f32 {
        let v = if v <= 0.0 { 0.0 } else { v };
        let n = n.clamp(1, self.buf.len());
        self.t += 1;
        self.buf[(self.t & self.mask) as usize] = v;
        if n != self.window {
            self.window = n;
            self.len = 0;
            for t in self.t + 1 - n as u64..=self.t {
                self.insert(t);
            }
        } else {
            self.insert(self.t);
        }
        let m = self.dq_buf.len() - 1;
        while self.dq_buf[self.head] + n as u64 <= self.t {
            self.head = (self.head + 1) & m;
            self.len -= 1;
        }
        self.buf[(self.dq_buf[self.head] & self.mask) as usize]
    }
}

/// The detector filter of `filterSetupIsFree` (`FUN_0034fdc4`): `hp' = min(hp, 0.95)`,
/// `lp' = max(lp, hp' + 0.05)`, `f = 20.6017 * 1024^p` Hz. Per channel four TPT one-pole
/// high-passes tapped after the second stage, then the same with low-passes when `lp' < 1`.
#[derive(Clone, Debug)]
pub struct FreeFilter {
    sr: f32,
    set: Option<(f32, f32)>,
    g_hp: Option<f32>,
    g_lp: Option<f32>,
    s_hp: [[f32; 4]; 2],
    s_lp: [[f32; 4]; 2],
}

impl FreeFilter {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            set: None,
            g_hp: None,
            g_lp: None,
            s_hp: [[0.0; 4]; 2],
            s_lp: [[0.0; 4]; 2],
        }
    }

    fn coef(&self, p: f32) -> f32 {
        let f = (F0 * (LN1024 * p as f64).exp()) as f32;
        tpt_k(f, self.sr)
    }

    /// Sets the `hpCutoff` / `lpCutoff` state values (the filter states are kept).
    pub fn set(&mut self, hp: f32, lp: f32) {
        if self.set == Some((hp, lp)) {
            return;
        }
        self.set = Some((hp, lp));
        let hp = if hp <= 0.95 { hp } else { 0.95 };
        let lim = hp + 0.05;
        let lp = if lp > lim { lp } else { lim };
        self.g_hp = (hp > 0.0).then(|| self.coef(hp));
        self.g_lp = (lp < 1.0).then(|| self.coef(lp));
    }

    pub fn process(&mut self, buf: &mut [[f32; 2]]) {
        if let Some(g) = self.g_hp {
            for f in buf.iter_mut() {
                for (c, x) in f.iter_mut().enumerate() {
                    let s = &mut self.s_hp[c];
                    let mut v = *x;
                    for (k, sk) in s.iter_mut().enumerate() {
                        let d = g * (v - *sk);
                        let lp = *sk + d;
                        *sk = d + lp;
                        v -= lp;
                        if k == 1 {
                            *x = v;
                        }
                    }
                }
            }
        }
        if let Some(g) = self.g_lp {
            for f in buf.iter_mut() {
                for (c, x) in f.iter_mut().enumerate() {
                    let s = &mut self.s_lp[c];
                    let mut v = *x;
                    for (k, sk) in s.iter_mut().enumerate() {
                        let d = g * (v - *sk);
                        let lp = *sk + d;
                        *sk = d + lp;
                        v = lp;
                        if k == 1 {
                            *x = v;
                        }
                    }
                }
            }
        }
    }
}

/// The detector input selection (`FUN_00351544` / `FUN_0035123c`) with its free filter: the band
/// audio; with `useSidechain` band k of the sidechain (silence without a bus); with
/// `filterSetupIsFree` the unsplit input (or the unsplit sidechain) through the free filter.
#[derive(Clone, Debug)]
pub struct DetectorInput {
    filter: FreeFilter,
    buf: Vec<[f32; 2]>,
}

impl DetectorInput {
    pub fn new(sr: f32, max_block: usize) -> Self {
        Self {
            filter: FreeFilter::new(sr),
            buf: vec![[0.0; 2]; max_block],
        }
    }

    /// The detector input for one block of `n <= max_block` samples; `None` means the band audio
    /// itself.
    pub fn fill(&mut self, p: &EnvParams, n: usize, src: &EnvSources) -> Option<&[[f32; 2]]> {
        let free = p.filter_setup_is_free;
        let input = match (p.use_sidechain, free) {
            (true, false) => src.sidechain_band,
            (true, true) => src.sidechain_full,
            (false, true) => Some(src.full),
            (false, false) => return None,
        };
        let out = &mut self.buf[..n];
        match input {
            Some(x) => out.copy_from_slice(&x[..n]),
            None => out.fill([0.0; 2]),
        }
        if free {
            self.filter.set(p.hp_cutoff, p.lp_cutoff);
            self.filter.process(out);
        }
        Some(out)
    }
}

/// `EnvelopeFollowerCoreAnalog::process` (`FUN_00348ee8`), the Dynamics detector: per sample the
/// gain reduction in units of 6 log2.
#[derive(Clone, Debug)]
pub struct AnalogCore {
    sr: f32,
    s_out: f32,
    s1: f32,
    s2: f32,
    lp: [f32; 2],
    atk: f32,
    rel: f32,
    rel2: f32,
    g: f32,
    hold_n: usize,
    smax: SlidingMax,
}

impl AnalogCore {
    pub fn new(sr: f32, attack: f32, hold: f32, release: f32) -> Self {
        let mut c = Self {
            sr,
            s_out: 0.0,
            s1: 0.0,
            s2: 0.0,
            lp: [0.0; 2],
            atk: 0.0,
            rel: 0.0,
            rel2: analog_coef(0.7, sr).unwrap_or(0.0),
            g: tpt_k(2.0, sr),
            hold_n: 0,
            smax: SlidingMax::new((sr * 0.51) as usize),
        };
        c.set_times(attack, hold, release);
        c
    }

    /// Sets the attack, hold and release state values.
    pub fn set_times(&mut self, attack: f32, hold: f32, release: f32) {
        if let Some(a) = analog_coef(attack_seconds(attack), self.sr) {
            self.atk = a;
        }
        if let Some(r) = analog_coef(release_seconds(release, true), self.sr) {
            self.rel = r;
        }
        self.hold_n = (hold_seconds(hold) * self.sr) as usize;
    }

    fn follow(&self, s: f32, e: f32, rel: f32) -> f32 {
        let c = (e * e * e * 0.002).clamp(-3.0, 3.0);
        let c = sat_cubic(c).clamp(-1.0, 1.0).max(0.0);
        s + (self.atk * c).mul_add(e, -(s * rel))
    }

    /// One sample: `x` is the detector input, `thr` the threshold in units (`60 t - 60`), `ratio`
    /// the amount ratio. Returns the gain reduction `GR`.
    #[inline]
    pub fn tick(&mut self, x: [f32; 2], thr: f32, ratio: f32, auto_release: bool) -> f32 {
        let g = ((self.s_out.mul_add(-0.02, -90.0) - thr) * SIXTH).exp2();
        let mut m = 0.0f32;
        for (c, v) in x.iter().enumerate() {
            let xg = v * g;
            let d = self.g * (xg - self.lp[c]);
            let lp = self.lp[c] + d;
            self.lp[c] = d + lp;
            m = m.max((xg - lp).abs());
        }
        let m = if m < MIN_LEVEL { MIN_LEVEL } else { m };
        let lvl = (m.log2() * 6.0 + 90.0).mul_add(ratio.abs(), 1e-7);
        let s = sat_cubic((lvl + lvl).clamp(-3.0, 3.0));
        let k = if s < -1.0 { -0.5 } else { s.min(1.0) * 0.5 };
        let mut tgt = lvl - k;
        if self.hold_n >= 1 {
            tgt = self.smax.push_max(tgt, self.hold_n + 1);
        }
        self.s1 = self.follow(self.s1, tgt - self.s1, self.rel);
        if auto_release {
            self.s2 = self.follow(self.s2, tgt - self.s2, self.rel2);
            self.s_out = (self.s1 + self.s2) * 0.5;
        } else {
            self.s_out = self.s1;
        }
        self.s_out.abs().copysign(ratio)
    }
}

/// The fast log2 of the Proportional core (Mineiro, on the float bits; floor 1e-12).
fn fastlog2(x: f32) -> f32 {
    let x = if x < 1e-12 { 1e-12 } else { x };
    let bits = x.to_bits();
    let mant = f32::from_bits((bits & 0x007f_ffff) | 0x3f00_0000);
    let y = bits as f32 * f32::from_bits(0x3400_0000) + f32::from_bits(0xc2f8_7377);
    let y = f32::from_bits(0xbfbf_bf75).mul_add(mant, y);
    y + f32::from_bits(0xbfdc_e9a3) / (mant + f32::from_bits(0x3eb4_44f9))
}

/// `(max(e, 0), min(e, 0))` with the binary's select semantics.
fn pn(e: f32) -> (f32, f32) {
    (if e < 0.0 { 0.0 } else { e }, if e > 0.0 { 0.0 } else { e })
}

/// `EnvelopeFollowerCoreProportional::process` (`FUN_0034bdc8`), the follower of every section
/// other than Dynamics. The section processor gives it `thr = 0` and `ratio = 1`.
#[derive(Clone, Debug)]
pub struct ProportionalCore {
    sr: f32,
    s: f32,
    a: [f32; 4],
    b: [f32; 5],
    atk: f32,
    rel: f32,
    rel4: f32,
    relp: f32,
    c05: f32,
    c10: f32,
    c20: f32,
    hold_n: usize,
    smax: SlidingMax,
}

impl ProportionalCore {
    pub fn new(sr: f32, attack: f32, hold: f32, release: f32) -> Self {
        let mut c = Self {
            sr,
            s: 0.0,
            a: [0.0; 4],
            b: [0.0; 5],
            atk: 0.0,
            rel: 0.0,
            rel4: 0.0,
            relp: 0.0,
            c05: prop_coef(0.0005, sr),
            c10: prop_coef(0.01, sr),
            c20: prop_coef(0.02, sr),
            hold_n: 0,
            smax: SlidingMax::new((sr * 0.51) as usize),
        };
        c.set_times(attack, hold, release);
        c
    }

    /// Sets the attack, hold and release state values.
    pub fn set_times(&mut self, attack: f32, hold: f32, release: f32) {
        let sr = self.sr;
        self.atk = prop_coef(attack_seconds(attack), sr);
        let rel_s = release_seconds(release, false);
        self.rel = prop_coef(rel_s, sr);
        let q = rel_s * 0.25;
        self.rel4 = prop_coef(if q <= 0.001 { 0.001 } else { q }, sr);
        self.relp = prop_coef(rel_s + 0.5, sr);
        self.hold_n = (hold_seconds(hold) * sr) as usize;
    }

    /// The autoRelease coefficient from the chain of peak-hold followers of `d`.
    fn auto_release_coef(&mut self, d: f32) -> f32 {
        let (a, b) = (&mut self.a, &mut self.b);
        let (c05, c10, c20) = (self.c05, self.c10, self.c20);
        let (ep, en) = pn(d - a[0]);
        a[0] += ep.mul_add(c05, c10 * en);
        for j in 1..4 {
            let (ep, en) = pn(a[j - 1] - a[j]);
            a[j] += en.mul_add(c10, ep);
        }
        let (ep, en) = pn(a[3] - b[0]);
        b[0] += ep + c20 * en;
        for j in 1..3 {
            let (ep, en) = pn(b[j - 1] - b[j]);
            b[j] += en.mul_add(c20, ep);
        }
        let (ep, en) = pn(b[2] - b[3]);
        let inc3 = en.mul_add(c20, ep);
        let (ep, en) = pn(a[3] - b[4]);
        let inc4 = ep + en;
        b[3] += inc3;
        b[4] += inc4;
        let crest = fastlog2(b[3]) * 6.0 - fastlog2(b[4]) * 6.0;
        let t = (0.1 * crest).clamp(0.0, 1.0);
        let ss = (t * (t * t)) * t.mul_add(t.mul_add(6.0, -15.0), 10.0);
        -((self.rel4 - self.relp) * ss + self.relp)
    }

    /// One sample of the gated detector input `x` (`|x|` or 0). Returns the envelope.
    #[inline]
    pub fn tick(&mut self, x: [f32; 2], auto_release: bool) -> f32 {
        let mut d = x[0].abs().max(x[1].abs());
        if d < 0.0 {
            d = 0.0;
        }
        let mut relc = if auto_release {
            self.auto_release_coef(d)
        } else {
            -self.rel
        };
        if self.hold_n >= 1 {
            d = self.smax.push_max(d, self.hold_n + 1);
        }
        let e = d - self.s;
        let ep = if e >= 0.0 { e } else { 0.0 };
        if e > 0.0 {
            relc = -0.0;
        }
        self.s += self.atk.mul_add(ep, self.s * relc);
        self.s
    }
}

/// Which merge slots and depth table a section uses (descriptor bytes +0xab / +0xac / +0xb0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvShaper {
    /// Slots 15 / 12, depths {2, 4, 8}: every shaper not listed below.
    Default,
    /// Slots 14 / 12 (the Reverb sections).
    Reverb,
    /// Slots 15 / 13 (the Pitch sections).
    Pitch,
    /// Slots 15 / 12, depths {1, 2, 4} (the Noise section).
    Noise,
}

/// The follower of one `SectionEnvState` section of one band (every shaper but Dynamics): the
/// detector input, the gate (`FUN_0034ee6c`), the Proportional core and the merge into the
/// section's LFO port (`FUN_003511d0`, slots 12..15).
#[derive(Clone, Debug)]
pub struct SectionFollower {
    kind: EnvShaper,
    core: ProportionalCore,
    det: DetectorInput,
    offset: Smoother,
    out: Vec<f32>,
}

impl SectionFollower {
    pub fn new(sr: f32, max_block: usize, kind: EnvShaper, p: &EnvParams) -> Self {
        let mut offset = Smoother::new(p.offset, sr as f64, 0.0);
        offset.buf.reserve(max_block);
        Self {
            kind,
            core: ProportionalCore::new(sr, p.attack, p.hold, p.release),
            det: DetectorInput::new(sr, max_block),
            offset,
            out: vec![0.0; max_block],
        }
    }

    /// Modulates the section port `port` for one block of `band.len() <= max_block` samples;
    /// `band` is the band audio before the shaper runs. With `active` off the port is returned
    /// unchanged.
    pub fn process<'a>(
        &'a mut self,
        port: Wave<'a>,
        band: &[[f32; 2]],
        src: &EnvSources,
        p: &EnvParams,
    ) -> Wave<'a> {
        let n = band.len();
        assert!(n <= self.out.len(), "block longer than max_block");
        self.offset.process(p.offset, n);
        if !p.active {
            return port;
        }
        self.core.set_times(p.attack, p.hold, p.release);
        let det = self.det.fill(p, n, src).unwrap_or(band);
        let thr = p.threshold;
        let depths: [f32; 3] = if self.kind == EnvShaper::Noise {
            [1.0, 2.0, 4.0]
        } else {
            [2.0, 4.0, 8.0]
        };
        let dv = depths[p.depth.clamp(0, 2) as usize];
        let slot = match (self.kind, p.mode == 1) {
            (EnvShaper::Pitch, true) => 13,
            (_, true) => 12,
            (EnvShaper::Reverb, false) => 14,
            (_, false) => 15,
        };
        let a = p.amount;
        let k = match slot {
            12 | 13 => a * dv,
            14 => (1.0 - a) * dv,
            _ => a.mul_add(2.0, -1.0) * dv,
        };
        for (i, x) in det.iter().enumerate() {
            let gate = |v: f32| if v.abs() >= thr { v.abs() } else { 0.0 };
            let env = self.core.tick([gate(x[0]), gate(x[1])], p.auto_release);
            let off = self.offset.at(i).mul_add(2.0, -1.0);
            let w = port.at(i);
            self.out[i] = match slot {
                12 => w.mul_add(k * env, off),
                13 => (w + -0.5).mul_add(k * env, off) + 0.5,
                14 => (-env).mul_add(k, 1.0).max(0.0).mul_add(w, off),
                _ => {
                    let v = env.mul_add(k, off);
                    let hi = 1.0 - w;
                    let c = if v < -w {
                        -w
                    } else if hi < v {
                        hi
                    } else {
                        v
                    };
                    w + c
                }
            };
        }
        Wave::PerSample(&self.out[..n])
    }
}
