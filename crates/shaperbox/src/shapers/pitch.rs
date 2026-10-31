//! Pitch (`PitchShaperModulation::process` `FUN_0046b880`; shapers_b.md section 2, model
//! `.amp/in/sb/data/sb_pitch_exact.py`): per band a pitch shifter picked per block by the
//! shaper-level `mode`.
//!
//! - Mode 3, Cyclic (`FUN_0042c720`): a two-grain delay-line shifter. Bit-exact in the model.
//! - Modes 1 and 2, PSOLA Vocals / Drums (`FUN_0042cde4`) with the pitch detector
//!   (`FUN_0042a5d8`) and, in Drums, the transient detector. The synthesis is bit-exact in the
//!   model; the detector uses a double-precision FFT (`realfft`) where the plugin uses pffft, so
//!   voiced material can differ by a few ulps of the detected period.
//! - Mode 0, WSOLA (`FUN_0042eda4`): **not ported yet**. Mode 0 falls back to PSOLA Vocals
//!   (its latency, 2381 samples at 48 kHz, instead of WSOLA's 1687).
//!
//! The band mix is the send-style law (`super::reverb::send_mix`) with the dry signal delayed by
//! the shifter latency ([`PitchBand::delay_dry`]). The latency is not reported to the host.
//! All arithmetic is f32 with the binary's fused multiply-adds; run it under
//! [`crate::ftz::FlushToZero`].

use std::sync::{Arc, OnceLock};

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{OnePole, Wave, tpt_k};
use crate::smoother::Smoother;

/// 0.10124 rad per sinc table index.
const SINC_STEP: f32 = f32::from_bits(0x3dcf_585a);
/// 9.877 table indices per rad.
const IDX_PER_RAD: f32 = f32::from_bits(0x411e_0939);
const PI_F: f32 = std::f32::consts::PI;
const BW_NUM: f32 = 0.9;
const BW_MIN: f32 = 0.031_25;
/// The delay under which the ring read returns the current sample.
const TINY: f64 = 9.999_999_747_378_752e-6;
const CYC_C099: f32 = f32::from_bits(0x3dca_c083);
const CYC_C001: f32 = f32::from_bits(0x3a83_126f);
const C_1_9: f32 = f32::from_bits(0x3de3_8e39);
const C_2_9: f32 = f32::from_bits(0x3e63_8e39);
const C_3_9: f32 = f32::from_bits(0x3eaa_aaab);
const C_02: f32 = f32::from_bits(0x3e4c_cccd);
const C_0666: f32 = f32::from_bits(0x3f2a_7efa);
const C_04: f32 = f32::from_bits(0x3ecc_cccd);
const C_0001: f32 = f32::from_bits(0x3a83_126f);
const C_001: f32 = f32::from_bits(0x3c23_d70a);
const C_1E9: f32 = f32::from_bits(0x3089_705f);
/// The detector's internal rate and window.
const RATE: f64 = 11025.0;
const N: usize = 512;
const FLT_MAX_NEG: f32 = f32::from_bits(0xff7f_ffff);
const EPS_SIL: f32 = f32::from_bits(0x3586_37bd);
const EPS_ONE: f32 = f32::from_bits(0x38d1_b717);
const MARKS: usize = 16;
const GRAINS: usize = 128;
const TRANS: usize = 128;

/// The interpolation tables, shared by every band.
struct Tables {
    /// 1025 entries of `sinc(i * 0.10124)`.
    sinc: Vec<f32>,
    /// The second half of a 2048-point 4-term Blackman-Harris window, then two zeros.
    win: Vec<f32>,
    /// `1 / sum` of the 16 integer-offset taps.
    norm: f32,
    /// The detector's Hann window, scaled to sum to 512.
    hann: Vec<f32>,
}

impl Tables {
    fn new() -> Self {
        let sinc = (0..0x401)
            .map(|i| {
                let x = i as f64 * SINC_STEP as f64;
                if x == 0.0 { 1.0 } else { (x.sin() / x) as f32 }
            })
            .collect::<Vec<_>>();
        let mut win = (0..0x400)
            .map(|k| {
                let a = ((0x400 + k) as f64 * std::f64::consts::TAU) / 2047.0;
                let w = a.cos().mul_add(-0.48829, 0.35875);
                let w = (a + a).cos().mul_add(0.14128, w);
                (a * 3.0).cos().mul_add(-0.01168, w) as f32
            })
            .collect::<Vec<_>>();
        win.extend([0.0, 0.0]);
        let step = IDX_PER_RAD * PI_F;
        let mut arg = step * -7.0;
        let mut t = 0.0f32;
        let mut acc = 0.0f32;
        for _ in 0..16 {
            let a = arg.abs();
            let i = a as usize;
            let s0 = sinc[i];
            let tc = if t <= 1.0 { t } else { 1.0 };
            let wpos = if t >= 0.0 { (tc + -0.5).abs() } else { 0.5 };
            let wpos = 2048.0 * wpos;
            let j = wpos as usize;
            let w0 = win[j];
            let s = s0 + (a - i as f32) * (sinc[i + 1] - s0);
            let w = w0 + (wpos - j as f32) * (win[j + 1] - w0);
            acc += s * w;
            arg += step;
            t += 0.066_666_67;
        }
        let mut hann = (0..N)
            .map(|n| {
                let a = ((2 * n) as f32 * PI_F) / 511.0;
                let c = (a as f64).cos() as f32;
                (0.5 + -0.5 * c as f64) as f32
            })
            .collect::<Vec<_>>();
        let s = hann.iter().fold(0.0f32, |s, v| s + v);
        let scale = 512.0 / s;
        for v in &mut hann {
            *v *= scale;
        }
        Self {
            sinc,
            win,
            norm: 1.0 / acc,
            hann,
        }
    }
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(Tables::new)
}

fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p *= 2;
    }
    p
}

/// `t^2 (3 - 2t)` of `t` clamped to [0, 1].
fn smoothstep(t: f32) -> f32 {
    let t = if t > 1.0 { 1.0 } else { t };
    let t = if t < 0.0 { 0.0 } else { t };
    (t * t) * t.mul_add(-2.0, 3.0)
}

/// `t^3 (10 - 15t + 6t^2)` of `t` clamped to [0, 1].
fn smootherstep(t: f32) -> f32 {
    let t = if t > 1.0 { 1.0 } else { t };
    let t = if t < 0.0 { 0.0 } else { t };
    let a = t * (t * t);
    let b = t.mul_add(6.0, -15.0);
    a * t.mul_add(b, 10.0)
}

/// The 4-point cubic of the PSOLA reader and the harmonic score.
fn cubic(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c3 = (y1 - y2).mul_add(1.5, (y3 - y0) * 0.5);
    let c2 = y2.mul_add(2.0, y0);
    let c2 = y1.mul_add(-2.5, c2);
    let c2 = y3.mul_add(-0.5, c2);
    let c1 = (y2 - y0) * 0.5;
    let v = c3.mul_add(t, c2);
    let v = v.mul_add(t, c1);
    v.mul_add(t, y1)
}

/// The section value `w` in [0, 1] to the pitch ratio `exp2f(4w - 2)` (0.25 .. 4).
pub fn ratio_from_wave(w: f32) -> f32 {
    w.mul_add(4.0, -2.0).exp2()
}

/// A stereo ring (`FUN_002d2fb8` write, `FUN_003131e4` windowed-sinc read).
struct Ring {
    l: Vec<f32>,
    r: Vec<f32>,
    size: i64,
    mask: i64,
    w: i64,
    filled: i64,
}

impl Ring {
    fn new(size: usize) -> Self {
        Self {
            l: vec![0.0; size],
            r: vec![0.0; size],
            size: size as i64,
            mask: size as i64 - 1,
            w: 0,
            filled: 0,
        }
    }

    fn write(&mut self, x: &[[f32; 2]]) {
        for (i, f) in x.iter().enumerate() {
            let k = ((self.w + i as i64) & self.mask) as usize;
            self.l[k] = f[0];
            self.r[k] = f[1];
        }
        self.w = (self.w + x.len() as i64) & self.mask;
        self.filled = self.size.min(self.filled + x.len() as i64);
    }

    fn at(&self, k: i64) -> (f32, f32) {
        let k = (k & self.mask) as usize;
        (self.l[k], self.r[k])
    }

    /// The 15-tap windowed sinc around `floor(d) = fl`; taps before `lead` are skipped and taps
    /// from `hi` on reuse the last loaded sample.
    #[allow(clippy::too_many_arguments)]
    fn kernel(&self, fl: i64, frac: f32, lead: i64, hi: i64, bw: f32, tab: &Tables) -> (f32, f32) {
        let step = (bw * PI_F) * IDX_PER_RAD;
        let wstep = 1.0f32 / 15.0;
        let mut arg = (-7.0 - frac) * step;
        let mut t = -(frac * wstep);
        for _ in 0..lead {
            arg += step;
            t += wstep;
        }
        let g = bw * tab.norm;
        let (mut accl, mut accr) = (0.0f32, 0.0f32);
        let (mut xl, mut xr) = (0.0f32, 0.0f32);
        let total = (hi - lead) + if hi > 15 { 0 } else { 16 - hi };
        for (it, idx) in (0..total).zip(lead + fl - 7..) {
            if it < hi - lead {
                (xl, xr) = self.at(idx);
            }
            let a = arg.abs();
            let ai = a as usize;
            let af = a - ai as f32;
            let s0 = tab.sinc[ai];
            let s = (tab.sinc[ai + 1] - s0).mul_add(af, s0);
            let tc = if t > 1.0 { 1.0 } else { t };
            let wp = if t < 0.0 { 0.5 } else { (tc + -0.5).abs() } * 2048.0;
            let wi = wp as usize;
            let w0 = tab.win[wi];
            let w = (tab.win[wi + 1] - w0).mul_add(wp - wi as f32, w0);
            let c = (g * s) * w;
            accl = xl.mul_add(c, accl);
            accr = xr.mul_add(c, accr);
            arg += step;
            t += wstep;
        }
        (accl, accr)
    }

    /// `FUN_003131e4`: the ring at `pos` samples behind the sample `remaining` before the write
    /// head, low-passed for `|ratio| > 1`.
    fn read(&self, pos: f64, ratio: f32, remaining: i64, tab: &Tables) -> (f32, f32) {
        let base = self.w - remaining;
        let d = base as f64 - pos;
        let i10 = d as i64;
        let fl = i10 - (d < i10 as f64) as i64;
        let limit = self.size.min(self.filled + 31);
        let lo = self.w - limit;
        if pos < 0.0 || fl < lo {
            return (0.0, 0.0);
        }
        if pos <= TINY {
            if remaining < 1 || self.filled < remaining {
                return (0.0, 0.0);
            }
            return self.at(base);
        }
        let frac = (d - fl as f64) as f32;
        let mut bw = BW_NUM / ratio.abs().max(1.0);
        if bw < BW_MIN {
            bw = 1.0;
        }
        let rel = fl - (lo + 7);
        let lead = if rel > 0 { 0 } else { (-rel).min(16) };
        let tail = (limit - remaining) - rel;
        let cnt = if tail + 1 < 0 { 0 } else { (tail + 1).min(16) };
        self.kernel(fl, frac, lead, cnt.max(lead), bw, tab)
    }
}

/// Mode 3, `PitchShifterCyclic` (`FUN_0042c720`).
struct Cyclic {
    sr: f32,
    ring: Ring,
    ph: f32,
    g: f32,
}

impl Cyclic {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            ring: Ring::new(next_pow2((sr as f64 * 0.1) as usize * 2 + 6)),
            ph: 0.0,
            g: -1.0,
        }
    }

    fn process(&mut self, buf: &mut [[f32; 2]], ratio: &[f32], grain: &Smoother, tab: &Tables) {
        let n = buf.len();
        self.ring.write(buf);
        for (i, f) in buf.iter_mut().enumerate() {
            let g = grain.at(i);
            let mut gt = self.sr * (g * g).mul_add(CYC_C099, CYC_C001);
            if self.g >= 0.0 {
                gt = self.g + (gt - self.g).clamp(-1.0, 1.0);
            }
            let big = gt;
            self.g = big;
            let mut ph = self.ph;
            while ph < 0.0 {
                ph += big;
            }
            while ph > big {
                ph -= big;
            }
            let mut ph2 = big.mul_add(-0.5, ph);
            if ph2 < 0.0 {
                ph2 += big;
            }
            let r = ratio[i];
            let rem = (n - i) as i64;
            let (l1, r1) = self.ring.read((ph + 7.0) as f64, r, rem, tab);
            let (l2, r2) = self.ring.read((ph2 + 7.0) as f64, r, rem, tab);
            let win = |p: f32| {
                let s = (1.0 - (p / big).mul_add(2.0, -1.0).abs()).clamp(0.0, 1.0);
                (s * s) * s.mul_add(-2.0, 3.0)
            };
            let (w1, w2) = (win(ph), win(ph2));
            *f = [w1.mul_add(l1, l2 * w2), w1.mul_add(r1, r2 * w2)];
            self.ph = ph - (r + -1.0);
        }
    }
}

/// `[1, 2, 3, 2, 1] / 9` smoothing with clamped edges, as an fma chain.
fn smooth5(src: &[f32], out: &mut [f32]) {
    let last = src.len() as i64 - 1;
    for (j, o) in out.iter_mut().enumerate() {
        let at = |d: i64| src[(j as i64 + d).clamp(0, last) as usize];
        let v = at(-2).mul_add(C_1_9, 0.0);
        let v = at(-1).mul_add(C_2_9, v);
        let v = at(0).mul_add(C_3_9, v);
        let v = at(1).mul_add(C_2_9, v);
        *o = at(2).mul_add(C_1_9, v);
    }
}

/// `FUN_0042be3c`: the maximum of every positive run with parabolic refinement, as
/// `(lag, value)`.
fn pick_peaks(thr: f32, buf: &[f32], out: &mut Vec<(f64, f64)>) {
    out.clear();
    let n2 = buf.len();
    let last = n2 - 1;
    let push = |cur: usize, out: &mut Vec<(f64, f64)>| {
        let v = buf[cur];
        if v < thr {
            return;
        }
        if cur < last {
            let a = v.mul_add(-2.0, buf[cur + 1] + buf[cur - 1]);
            if a != 0.0 {
                let b = buf[cur - 1] - buf[cur + 1];
                let lag = cur as f64 + (b / (a + a)) as f64;
                let val = v as f64 + ((b * b) / (a * -8.0)) as f64;
                out.push((lag, val));
            } else {
                out.push((cur as f64, v as f64));
            }
        } else {
            let c = if buf[cur - 1] < v { cur - 1 } else { cur };
            out.push((c as f64, buf[c] as f64));
        }
    };
    let mut i = 2;
    if n2 >= 3 {
        while i < n2 && buf[i] > 0.0 {
            i += 1;
        }
        if i == n2 {
            return;
        }
    }
    while i < last && buf[i] <= 0.0 {
        i += 1;
    }
    if i >= last {
        return;
    }
    let mut cur = 0;
    while i < last {
        let v = buf[i];
        if buf[i - 1] < v && buf[i + 1] <= v && (cur == 0 || buf[cur] < v) {
            cur = i;
        }
        i += 1;
        if i < last && buf[i] <= 0.0 {
            if cur > 0 {
                push(cur, out);
            }
            cur = 0;
            let mut stop = false;
            while buf[i] <= 0.0 {
                i += 1;
                if i == last {
                    stop = true;
                    break;
                }
            }
            if stop {
                break;
            }
        }
    }
    if cur > 0 {
        push(cur, out);
    }
}

/// A pitch candidate of the detector.
#[derive(Clone, Copy, Debug, Default)]
struct Cand {
    lag: f64,
    score: f64,
    freq: f64,
    st: f64,
    acf: f64,
    cep: f64,
    harm: f64,
}

/// The pitch tracker (`FUN_0042c248`).
#[derive(Clone, Copy, Debug)]
struct Tracker {
    freq: f64,
    st: f64,
    count: u32,
    conf: f64,
}

impl Tracker {
    fn new() -> Self {
        Self {
            freq: 0.0,
            st: 0.0,
            count: 0,
            conf: 1.0,
        }
    }

    fn octdist(d: f64) -> f64 {
        let mut dm = d;
        for o in [-12.0, 12.0, -24.0, 24.0] {
            if (d + o).abs() < dm {
                dm = (d + o).abs();
            }
        }
        dm
    }

    /// `1 - |d - k|` for the first `k` of `ks` within 1 of `d`, else 0.
    fn near(d: f64, ks: [f64; 3]) -> f64 {
        for k in ks {
            let a = (d + k).abs();
            if a < 1.0 {
                return 1.0 - a;
            }
        }
        0.0
    }

    fn choose(&mut self, cands: &[Cand]) -> Option<Cand> {
        if cands.is_empty() {
            self.count = 0;
            self.conf = 1.0;
            return None;
        }
        if cands.len() == 1 {
            self.conf = (self.conf + 0.1).min(1.0);
            if self.count > 10 {
                self.conf *= 0.95;
            }
            let c = cands[0];
            self.freq = c.freq;
            self.st = c.st;
            self.count = self.count.min(0xffff) + 1;
            return Some(c);
        }
        let mut best = f64::NEG_INFINITY;
        let mut bi = 0;
        for (idx, c) in cands.iter().enumerate() {
            let mut score = c.score;
            let mut pen = 0.0;
            if self.count >= 1 {
                if self.freq > 0.0 {
                    let dm = Self::octdist((c.st - self.st).abs());
                    score += self.conf * (-dm).exp() * 0.7;
                }
                if self.count >= 3 && self.st != 0.0 {
                    let dd = c.st - self.st;
                    let b = Self::near(dd, [-12.0, -19.0, -24.0]);
                    score += b * c.acf * c.cep * c.harm * 0.1;
                    pen = Self::near(dd, [12.0, 19.0, 24.0]);
                }
            }
            score += pen * -0.2;
            if score > best {
                best = score;
                bi = idx;
            }
        }
        let c = cands[bi];
        let mut conf =
            if self.count == 0 || self.st == 0.0 || Self::octdist((c.st - self.st).abs()) >= 1.5 {
                (self.conf - 0.15).max(0.0)
            } else {
                (self.conf + 0.1).min(1.0)
            };
        if self.count >= 11 {
            conf *= 0.95;
        }
        self.conf = conf;
        self.freq = c.freq;
        self.st = c.st;
        self.count = self.count.min(0xffff) + 1;
        Some(c)
    }
}

/// The PSOLA pitch detector (prepare `FUN_00356f3c`, analyse `FUN_0042a5d8`).
struct Detector {
    dec: f64,
    offset: i64,
    ring: Ring,
    tracker: Tracker,
    thr_voiced: f32,
    thr_peak: f32,
    fwd1024: Arc<dyn RealToComplex<f64>>,
    inv1024: Arc<dyn ComplexToReal<f64>>,
    fwd512: Arc<dyn RealToComplex<f64>>,
    inv512: Arc<dyn ComplexToReal<f64>>,
    rbuf: Vec<f64>,
    cbuf: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    frame: Vec<f32>,
    work: Vec<f32>,
    nacf: Vec<f32>,
    acf_s: Vec<f32>,
    mag: Vec<f32>,
    cep_s: Vec<f32>,
    peaks: Vec<(f64, f64)>,
    cpeaks: Vec<(f64, f64)>,
    cands: Vec<Cand>,
}

impl Detector {
    fn new(sr: f64) -> Self {
        let dec = RATE / sr;
        let size = RATE as usize + (N as f64 / dec) as usize + 0x1ff;
        let mut planner = RealFftPlanner::<f64>::new();
        let fwd1024 = planner.plan_fft_forward(2 * N);
        let inv1024 = planner.plan_fft_inverse(2 * N);
        let fwd512 = planner.plan_fft_forward(N);
        let inv512 = planner.plan_fft_inverse(N);
        let scratch = fwd1024
            .get_scratch_len()
            .max(inv1024.get_scratch_len())
            .max(fwd512.get_scratch_len())
            .max(inv512.get_scratch_len());
        Self {
            dec,
            offset: 0,
            ring: Ring::new(next_pow2(size)),
            tracker: Tracker::new(),
            thr_voiced: 0.5,
            thr_peak: 0.0,
            fwd1024,
            inv1024,
            fwd512,
            inv512,
            rbuf: vec![0.0; 2 * N],
            cbuf: vec![Complex::new(0.0, 0.0); N + 1],
            scratch: vec![Complex::new(0.0, 0.0); scratch],
            frame: vec![0.0; N],
            work: vec![0.0; 2 * N],
            nacf: vec![0.0; N / 2],
            acf_s: vec![0.0; N / 2],
            mag: vec![0.0; N / 2],
            cep_s: vec![0.0; N / 2],
            peaks: Vec::with_capacity(N),
            cpeaks: Vec::with_capacity(N),
            cands: Vec::with_capacity(N),
        }
    }

    /// The constants of mode 1 (`DAT_009e5120`) or 2 (`DAT_009e5110`).
    fn set_mode(&mut self, vocals: bool) {
        (self.thr_voiced, self.thr_peak) = if vocals { (0.5, 0.0) } else { (0.0, 0.01) };
    }

    /// The last window resampled to 11025 Hz with the 15-tap windowed sinc, as `L + 0.001 R`.
    fn fill_frame(&mut self, remaining: i64, offset: i64, tab: &Tables) {
        let rg = &self.ring;
        let step = 1.0 / self.dec;
        let back = (self.offset + offset).max(7) + remaining;
        let mut d47 = (N as f64 / self.dec) as i64 as f64;
        let base = rg.w - back;
        let limit = rg.size.min(rg.filled + 31);
        let lo = rg.w - limit;
        let mut bw = BW_NUM / (step as f32).abs().max(1.0);
        if bw < BW_MIN {
            bw = 1.0;
        }
        let direct_ok = 0 < back && back <= rg.filled;
        for o in self.frame.iter_mut() {
            let d = base as f64 - d47;
            let i26 = d as i64;
            let fl = i26 - (d < i26 as f64) as i64;
            let (l, r) = if d47 < 0.0 || fl < lo {
                (0.0, 0.0)
            } else if d47 <= TINY {
                if direct_ok { rg.at(base) } else { (0.0, 0.0) }
            } else {
                let frac = (d - fl as f64) as f32;
                let rel = fl - (lo + 7);
                let lead = if rel > 0 { 0 } else { (-rel).min(16) };
                let cnt = if limit - rel < 0 {
                    0
                } else {
                    (limit - rel).min(16)
                };
                rg.kernel(fl, frac, lead, cnt.max(lead), bw, tab)
            };
            *o = r.mul_add(C_0001, l);
            d47 -= step;
        }
    }

    /// Analyses the window ending `remaining + max(offset + self.offset, 7)` samples behind the
    /// write head. Returns the period in host samples when voiced.
    fn analyse(&mut self, remaining: i64, offset: i64, tab: &Tables) -> Option<f64> {
        self.fill_frame(remaining, offset, tab);
        let n2 = N / 2;
        let x = &self.frame;
        // The autocorrelation through a 2N FFT.
        for (o, v) in self
            .rbuf
            .iter_mut()
            .zip(x.iter().chain(std::iter::repeat(&0.0)))
        {
            *o = *v as f64;
        }
        let _ =
            self.fwd1024
                .process_with_scratch(&mut self.rbuf, &mut self.cbuf, &mut self.scratch);
        for (k, c) in self.cbuf.iter_mut().enumerate() {
            let p = if k == 0 || k == N {
                0.0
            } else {
                let (re, im) = (c.re as f32, c.im as f32);
                re.mul_add(re, im * im)
            };
            *c = Complex::new(p as f64, 0.0);
        }
        let _ =
            self.inv1024
                .process_with_scratch(&mut self.cbuf, &mut self.rbuf, &mut self.scratch);
        let inv = 1.0f32 / (2 * N) as f32;
        let acf = &mut self.work[..N];
        for (a, v) in acf.iter_mut().zip(&self.rbuf) {
            *a = (*v as f32) * inv;
        }
        let mut e = x.iter().fold(0.0f32, |e, v| e + v * v);
        e += e;
        let den = if e >= 0.25 { e } else { 0.25 };
        self.nacf[0] = (acf[0] + acf[0]) / den;
        let invn = 1.0f32 / N as f32;
        for k in 1..n2 {
            e = (e - x[k - 1] * x[k - 1]) - x[N - k] * x[N - k];
            let den = if e >= 0.25 { e } else { 0.25 };
            let wgt = (invn * k as f32).mul_add(-C_02, 1.0);
            self.nacf[k] = wgt * ((acf[k] + acf[k]) / den);
        }
        smooth5(&self.nacf, &mut self.acf_s);
        let first_np = self.acf_s.iter().position(|v| *v <= 0.0).unwrap_or(n2);
        let a0 = self.acf_s[0];
        let inva = 1.0 / if a0 >= C_1E9 { a0 } else { C_1E9 };
        for v in &mut self.acf_s {
            *v *= inva;
        }
        pick_peaks(self.thr_peak, &self.acf_s, &mut self.peaks);
        let mut best: Option<(f64, f64)> = None;
        for p in &self.peaks {
            if best.is_none_or(|b| p.1 > b.1) {
                best = Some(*p);
            }
        }
        if best.is_none_or(|b| b.1 < self.thr_voiced as f64) {
            self.tracker = Tracker::new();
            return None;
        }
        // The spectrum of the windowed, RMS-normalised frame.
        let mut s = 0.0f64;
        for (i, v) in x.iter().enumerate() {
            let w = v * tab.hann[i];
            self.work[i] = w;
            s += (w * w) as f64;
        }
        let rms = ((s / N as f64) as f32).sqrt();
        let invr = 1.0 / if rms >= C_1E9 { rms } else { C_1E9 };
        for (o, v) in self.rbuf[..N].iter_mut().zip(&self.work[..N]) {
            *o = (v * invr) as f64;
        }
        let _ = self.fwd512.process_with_scratch(
            &mut self.rbuf[..N],
            &mut self.cbuf[..n2 + 1],
            &mut self.scratch,
        );
        for k in 0..n2 {
            self.mag[k] = if k == 0 {
                0.0
            } else {
                let (re, im) = (self.cbuf[k].re as f32, self.cbuf[k].im as f32);
                re.mul_add(re, im * im).sqrt()
            };
        }
        for k in 0..=n2 {
            let v = if k < n2 {
                (self.mag[k] + 0.5).log2()
            } else {
                0.0
            };
            self.cbuf[k] = Complex::new(v as f64, 0.0);
        }
        let _ = self.inv512.process_with_scratch(
            &mut self.cbuf[..n2 + 1],
            &mut self.rbuf[..N],
            &mut self.scratch,
        );
        let nz = first_np.clamp(2, 10);
        let cepg = &mut self.work[..n2];
        for (k, c) in cepg.iter_mut().enumerate() {
            let v = if k < nz {
                0.0
            } else {
                (self.rbuf[k] as f32) * invn
            };
            let v = v * if self.acf_s[k] > 0.0 { 1.0 } else { 0.0 };
            *c = if v >= 0.0 { v } else { 0.0 };
        }
        smooth5(cepg, &mut self.cep_s);
        pick_peaks(C_001, &self.cep_s, &mut self.cpeaks);
        self.cands.clear();
        for &(lag, val) in &self.peaks {
            let freq = RATE / lag;
            self.cands.push(Cand {
                lag,
                freq,
                st: (freq / 440.0).log2() * 12.0,
                acf: val,
                ..Cand::default()
            });
        }
        for &(lag, val) in &self.cpeaks {
            let st = ((RATE / lag) / 440.0).log2() * 12.0;
            let mut bestc = None;
            let mut bd = f64::MAX;
            for (k, c) in self.cands.iter().enumerate() {
                let d = (c.st - st).abs();
                if d <= 12.0 && d < bd {
                    bestc = Some(k);
                    bd = d;
                }
            }
            if let Some(k) = bestc {
                let v2 = val + val;
                let c = &mut self.cands[k];
                if v2 > c.cep {
                    c.cep = v2;
                }
            }
        }
        let last = n2 - 1;
        for c in &mut self.cands {
            let bin = (c.freq / RATE) * N as f64;
            let mut acc = 0.0f32;
            for m in 1..=5 {
                let pos = if m == 2 { bin + bin } else { bin * m as f64 };
                let idx = pos as i64;
                if !(0..n2 as i64).contains(&idx) {
                    break;
                }
                let idx = idx as usize;
                let m_ = &self.mag;
                let frac = (pos as f32) - idx as f32;
                let v = cubic(
                    m_[idx.max(1) - 1],
                    m_[idx],
                    m_[(idx + 1).min(last)],
                    m_[(idx + 2).min(last)],
                    frac,
                );
                let term = invn * v;
                acc = match m {
                    1 => term + 0.0,
                    2 => term + acc,
                    3 => term.mul_add(C_0666, acc),
                    4 => term.mul_add(0.5, acc),
                    _ => term.mul_add(C_04, acc),
                };
            }
            c.harm = ((acc / 5.0) * 5.0) as f64;
            // Both weights are 1 in either mode.
            c.score = 1.0f64.mul_add(c.harm, 1.0f64.mul_add(c.cep, c.acf));
        }
        let chosen = self.tracker.choose(&self.cands)?;
        Some(chosen.lag / self.dec)
    }
}

/// A pitch mark `{pos, b, flag}`.
#[derive(Clone, Copy, Debug, Default)]
struct Mark {
    pos: f32,
    b: f32,
    flag: u8,
}

/// A PSOLA grain (record of 0x38 bytes).
#[derive(Clone, Copy, Debug)]
struct Grain {
    resync: bool,
    pos: f32,
    read: f32,
    read2: f32,
    period: f32,
    len: i64,
    age: i64,
    phase: f32,
    rate: f32,
    gain: f32,
    gain_next: f32,
    cycle: i64,
    range_max: f32,
    done: bool,
}

impl Grain {
    fn new(resync: bool, pos: f32, period: f32, len: i64, phase: f32, rate: f32) -> Self {
        Self {
            resync,
            pos,
            read: pos,
            read2: pos,
            period,
            len,
            age: 0,
            phase,
            rate,
            gain: 1.0,
            gain_next: 1.0,
            cycle: 0,
            range_max: FLT_MAX_NEG,
            done: false,
        }
    }
}

/// The PSOLA ring read: windowed sinc for `f > 1`, nearest for `f == 1`, cubic for `f < 1`.
fn psola_read(rg: &Ring, d0: f64, f: f32, remaining: i64, tab: &Tables) -> (f32, f32) {
    if f > 1.0 {
        return rg.read(d0, f, remaining, tab);
    }
    let d3 = (rg.w - remaining) as f64 - d0;
    let i = d3 as i64;
    let fl = i - (d3 < i as f64) as i64;
    let limit = rg.size.min(rg.filled + 31);
    let lo = rg.w - limit;
    if !(d0 >= 0.0 && fl >= lo) {
        return (0.0, 0.0);
    }
    let rel = fl - lo;
    let hi = limit - remaining;
    if f == 1.0 {
        let k = if rel < 0 { 0 } else { hi.min(rel) };
        return rg.at(k + lo);
    }
    let frac = (d3 - fl as f64) as f32;
    let k0 = if rel > 0 { hi.min(rel - 1) } else { 0 };
    let k1 = if rel < 0 { 0 } else { hi.min(rel) };
    let k2 = if rel < -1 { 0 } else { hi.min(rel + 1) };
    let k3 = if rel < -2 { 0 } else { hi.min(rel + 2) };
    let (l0, r0) = rg.at(k0 + lo);
    let (l1, r1) = rg.at(k1 + lo);
    let (l2, r2) = rg.at(k2 + lo);
    let (l3, r3) = rg.at(k3 + lo);
    (cubic(l0, l1, l2, l3, frac), cubic(r0, r1, r2, r3, frac))
}

/// The PSOLA latency and its parts (shapers_b.md 2.3).
#[derive(Clone, Copy, Debug)]
struct PsolaSizes {
    win: i64,
    fade: i64,
    half: f32,
    lat: i64,
}

impl PsolaSizes {
    fn new(sr: f32, sync: i64) -> Self {
        let win = (N as f64 / (RATE / sr as f64)).ceil() as i64;
        let fade = (sr * 0.003) as i64;
        let half = (win / 2) as f32;
        let lat = (win + fade + sync + 7).max(sync + (half + half).ceil() as i64);
        Self {
            win,
            fade,
            half,
            lat,
        }
    }
}

/// Modes 1 and 2, `PitchShifterPsola` (`FUN_0042cde4`).
struct Psola {
    sr: f32,
    vocals: bool,
    win: i64,
    min_period: f32,
    half: f32,
    det_interval: i64,
    p960: f32,
    fade: i64,
    sync: i64,
    lat: i64,
    lat_fade: i64,
    ring: Ring,
    ratio_ring: Vec<f32>,
    formant_ring: Vec<f32>,
    rmask: i64,
    rw: i64,
    marks: [Mark; MARKS],
    m_head: usize,
    m_tail: usize,
    m_count: usize,
    grains: Vec<Grain>,
    g_head: usize,
    g_tail: usize,
    g_count: usize,
    trans: [i64; TRANS],
    t_head: usize,
    t_tail: usize,
    t_count: usize,
    mark_b: f32,
    mark_pos: f32,
    phase: f32,
    det_cnt: i64,
    acc: f64,
    silence: i64,
    nonsil: i64,
    frozen: bool,
    det: Detector,
    det_period: f32,
    det_voiced: bool,
}

impl Psola {
    fn new(sr: f32, vocals: bool, sync: i64) -> Self {
        let det = Detector::new(sr as f64);
        let s = PsolaSizes::new(sr, sync);
        let min_period = (sr as f64 / (RATE * 0.5)) as f32;
        let size = next_pow2(s.win as usize * 8);
        let mut p = Self {
            sr,
            vocals,
            win: s.win,
            min_period,
            half: s.half,
            det_interval: (min_period * 4.0) as i64,
            p960: (sr as f64 * 0.02) as f32,
            fade: s.fade,
            sync,
            lat: s.lat,
            lat_fade: s.lat - s.fade,
            ring: Ring::new(size),
            ratio_ring: vec![1.0; size],
            formant_ring: vec![1.0; size],
            rmask: size as i64 - 1,
            rw: 0,
            marks: [Mark::default(); MARKS],
            m_head: 0,
            m_tail: 0,
            m_count: 0,
            grains: vec![Grain::new(false, 0.0, 1.0, 1, 0.0, 0.0); GRAINS],
            g_head: 0,
            g_tail: 0,
            g_count: 0,
            trans: [0; TRANS],
            t_head: 0,
            t_tail: 0,
            t_count: 0,
            mark_b: s.half,
            mark_pos: s.lat as f32,
            phase: 0.0,
            det_cnt: 0,
            acc: 0.0,
            silence: 0x100_0000,
            nonsil: 0,
            frozen: false,
            det,
            det_period: 0.0,
            det_voiced: false,
        };
        p.det_cnt = p.det_interval;
        p.set_mode(vocals, sync);
        p
    }

    /// The mode flags, the sync distance and the latencies (`FUN_0045ff80`).
    fn set_mode(&mut self, vocals: bool, sync: i64) {
        let s = PsolaSizes::new(self.sr, sync);
        self.vocals = vocals;
        self.sync = sync;
        self.lat = s.lat;
        self.lat_fade = s.lat - s.fade;
        self.det.offset = (s.lat - (self.win as f64 * 0.666_666_666_666_666_6) as i64).max(0);
        self.det.set_mode(vocals);
    }

    fn push_mark(&mut self, pos: f32, b: f32, flag: u8) {
        if self.m_count != 0 && self.m_count == MARKS {
            self.m_tail = (self.m_tail + MARKS - 1) % MARKS;
            self.m_count -= 1;
            if self.m_count == 0 {
                self.m_head = 0;
                self.m_tail = 0;
            }
        }
        self.m_head = (self.m_head + MARKS - 1) % MARKS;
        self.marks[self.m_head] = Mark { pos, b, flag };
        self.m_count += 1;
    }

    fn pop_grain(&mut self) {
        self.g_tail = (self.g_tail + 1) % GRAINS;
        self.g_count -= 1;
        if self.g_count == 0 {
            self.g_tail = 0;
            self.g_head = 0;
        }
    }

    fn push_grain(&mut self, g: Grain) {
        if self.g_count == GRAINS {
            self.pop_grain();
        }
        self.grains[self.g_head] = g;
        self.g_head = (self.g_head + 1) % GRAINS;
        self.g_count += 1;
    }

    fn pop_trans(&mut self) {
        self.t_tail = (self.t_tail + 1) % TRANS;
        self.t_count -= 1;
        if self.t_count == 0 {
            self.t_tail = 0;
            self.t_head = 0;
        }
    }

    fn push_trans(&mut self, v: i64) {
        if self.t_count == TRANS {
            self.pop_trans();
        }
        self.trans[self.t_head] = v;
        self.t_head = (self.t_head + 1) % TRANS;
        self.t_count += 1;
    }

    fn newest(&self) -> &Grain {
        &self.grains[(self.g_head + GRAINS - 1) % GRAINS]
    }

    fn resync_grain(&self) -> Grain {
        let rate = 0.0 / (self.mark_b + self.mark_b);
        Grain::new(
            true,
            self.mark_pos,
            self.mark_b,
            self.fade.max(1),
            0.0,
            rate,
        )
    }

    fn analyse(&mut self, remaining: i64, offset: i64, tab: &Tables) {
        if let Some(per) = self.det.analyse(remaining, offset, tab) {
            self.det_period = per as f32;
            self.det_voiced = true;
        }
    }

    fn read_direct(&self, remaining: i64) -> (f32, f32) {
        let rg = &self.ring;
        let d3 = (rg.w - remaining) as f64 - self.lat as f64;
        let i = d3 as i64;
        let fl = i - (d3 < i as f64) as i64;
        let limit = rg.size.min(rg.filled + 31);
        let lo = rg.w - limit;
        let rel = fl - lo;
        if self.lat < 0 || rel < 0 {
            return (0.0, 0.0);
        }
        rg.at((limit - remaining).min(rel) + lo)
    }

    #[allow(clippy::too_many_arguments)]
    fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        ratio: &[f32],
        formant: &[f32],
        transients: &[i64],
        link_formant: bool,
        fill_gaps: f32,
        tab: &Tables,
    ) {
        let n = buf.len();
        if self.vocals {
            self.t_head = 0;
            self.t_tail = 0;
            self.t_count = 0;
        } else {
            for &v in transients {
                self.push_trans(self.sync - v);
            }
        }
        self.ring.write(buf);
        for i in 0..n {
            let k = ((self.rw + i as i64) & self.rmask) as usize;
            self.ratio_ring[k] = ratio[i];
            self.formant_ring[k] = formant[i];
        }
        self.rw = (self.rw + n as i64) & self.rmask;
        self.det.ring.write(buf);
        if self.m_count == 0 {
            self.mark_b = self.half;
            self.mark_pos = self.lat as f32;
            self.push_mark(self.lat as f32, self.half, 0);
        }
        if self.g_count == 0 {
            self.push_grain(self.resync_grain());
        }
        let w16 = fill_gaps > 0.0 && !link_formant;
        let sqrt_fill = fill_gaps.sqrt();
        let mut w23 = 0i64;
        for (i, out) in buf.iter_mut().enumerate() {
            let remaining = (n - i) as i64;
            let lat = self.lat;
            let k = ((self.rw - (lat + remaining)) & self.rmask) as usize;
            let r_raw = self.ratio_ring[k];
            let f_raw = if link_formant {
                r_raw
            } else {
                self.formant_ring[k]
            };
            let amp = out[0].abs().max(out[1].abs());
            self.silence = if amp > EPS_SIL {
                0
            } else {
                self.silence.min(0xff_ffff) + 1
            };
            self.nonsil = if self.silence < self.lat_fade {
                self.nonsil.min(0xff_ffff) + 1
            } else {
                0
            };
            let mut w19 = self.t_count != 0;
            let w22 = if self.t_count != 0 {
                w23 = self.trans[self.t_tail];
                w23 == self.lat_fade
            } else {
                false
            };
            let w21 = w22 as i32 + (self.nonsil == self.lat_fade) as i32;
            let fv44 = if w21 == 1 {
                // The reset / resync event.
                self.det.tracker = Tracker::new();
                self.det_period = 0.0;
                self.det_voiced = false;
                self.det_cnt = self.det_interval;
                if self.silence < self.lat_fade {
                    let off = -((self.win as f64 * 0.333_333_333_333_333_37) as i64) - self.fade;
                    self.analyse(remaining, off, tab);
                    self.det_cnt = 0;
                }
                self.push_mark(lat as f32, self.fade as f32, 0);
                let base = if self.det_voiced {
                    self.det_period
                } else {
                    self.p960
                };
                self.mark_b = base + self.fade as f32;
                self.mark_pos = self.lat_fade as f32;
                self.push_mark(self.mark_pos, self.mark_b, 1);
                if !self.frozen {
                    self.push_grain(self.resync_grain());
                }
                if w22 {
                    if self.t_count != 0 {
                        self.pop_trans();
                    }
                    if self.t_count == 0 {
                        w19 = false;
                        w23 = 0;
                    } else {
                        w23 = self.trans[self.t_tail];
                        w19 = true;
                    }
                }
                self.acc = 0.0;
                self.mark_b
            } else {
                let mut v = self.phase;
                if v < 1.0 {
                    if self.silence < self.lat_fade {
                        if self.det_cnt >= self.det_interval {
                            self.analyse(remaining, 0, tab);
                            self.det_cnt = 0;
                            v = self.phase;
                        }
                    } else {
                        self.det_period = 0.0;
                        self.det_voiced = false;
                    }
                    let s1 = if self.det_voiced {
                        self.det_period
                    } else {
                        self.half
                    };
                    self.mark_b = s1;
                    self.mark_pos = lat as f32 - v;
                    self.push_mark(self.mark_pos, s1, 0);
                    let base = if self.det_voiced {
                        self.det_period
                    } else {
                        self.sr / 440.0
                    };
                    v = base + self.phase;
                }
                v
            };
            for k in 0..self.t_count {
                self.trans[(self.t_tail + k) % TRANS] += 1;
            }
            let snap = |v: f32| {
                let v = if (v + -1.0).abs() > EPS_ONE { v } else { 1.0 };
                let v = if v < 0.25 { 0.25 } else { v };
                if v > 4.0 { 4.0 } else { v }
            };
            let (f, r) = (snap(f_raw), snap(r_raw));
            self.phase = fv44 + -1.0;
            let mut direct = false;
            if r == 1.0 && f == 1.0 {
                if w21 != 0 {
                    self.frozen = true;
                }
                let g = self.newest();
                direct = self.frozen && g.resync && g.age >= g.len;
            } else {
                self.frozen = false;
            }
            let (l, rr) = if direct {
                if self.g_count >= 2 {
                    self.g_tail = (self.g_tail + self.g_count - 1) % GRAINS;
                    self.g_count = 1;
                }
                self.read_direct(remaining)
            } else {
                let s13 = if w19 && (r > 1.0 || f > r) {
                    (self.fade + w23) as f64
                } else {
                    -(self.half as f64)
                };
                self.synth(r, f, remaining, s13, sqrt_fill, w16, tab)
            };
            *out = [l, rr];
            self.mark_pos += 1.0;
            for k in 0..self.m_count {
                self.marks[(self.m_head + k) % MARKS].pos += 1.0;
            }
            self.det_cnt = (self.det_cnt + 1).min(self.det_interval);
        }
    }

    /// Starts a grain when due, then sums the grains from the newest to the oldest.
    #[allow(clippy::too_many_arguments)]
    fn synth(
        &mut self,
        r: f32,
        f: f32,
        remaining: i64,
        s13: f64,
        sqrt_fill: f32,
        w16: bool,
        tab: &Tables,
    ) -> (f32, f32) {
        let d1 = r as f64 / self.newest().period as f64;
        self.acc += d1;
        if self.acc >= 1.0 {
            let (mut s4, mut s3) = (self.mark_pos, self.mark_b);
            if !self.vocals && self.m_count != 0 {
                let s5 = if f > 2.0 { -f } else { -2.0 };
                let mut idx = self.m_head;
                let mut left = self.m_count;
                loop {
                    left -= 1;
                    let mk = self.marks[idx % MARKS];
                    let s6 = mk.pos;
                    if mk.flag == 1 && s6 as f64 > self.lat as f64 {
                        let s4c = mk.b.mul_add(-0.5, s6);
                        let s7 = ((s4c as f64 - s13) as f32) * 0.5;
                        let s3c = if s7 < mk.b { s7 } else { mk.b };
                        if s3c >= self.min_period {
                            s4 = s4c;
                            s3 = s3c;
                            break;
                        }
                    }
                    s3 = mk.b;
                    s4 = s5.mul_add(mk.b, s6);
                    idx += 1;
                    if s13 < s4 as f64 || left == 0 {
                        s4 = s6;
                        break;
                    }
                }
            }
            let ph = (((self.acc - 1.0) / d1) * f as f64) as f32;
            let rate = ph / (s3 + s3);
            self.push_grain(Grain::new(false, s4, s3, 1, ph, rate));
            self.acc -= 1.0;
        }
        let mut count = self.g_count;
        if count < 2 {
            if count != 1 {
                return (0.0, 0.0);
            }
        } else {
            while self.grains[self.g_tail].done {
                self.g_tail = (self.g_tail + 1) % GRAINS;
                let old = count;
                count -= 1;
                self.g_count = count;
                if old <= 2 {
                    count = 1;
                    self.g_count = 1;
                    break;
                }
            }
        }
        let s1 = self.newest().period;
        let fv62 = (r / f).min(1.0);
        let r2 = if self.vocals && f < r { f } else { r };
        let inc = (r2 - f).mul_add(sqrt_fill, f) / (s1 + s1);
        let neg_half = -(self.half as f64);
        let (mut out_l, mut out_r) = (0.0f32, 0.0f32);
        let mut budget = 1.0f32;
        let mut idx = count as i64 - 1;
        while idx >= 0 {
            let j = (self.g_tail + idx as usize) % GRAINS;
            let ring = &self.ring;
            let g = &mut self.grains[j];
            let s0 = if s13 < g.pos as f64 { s13 } else { neg_half };
            if (g.range_max as f64) < s0 {
                g.range_max = s0 as f32;
            }
            let s15 = if g.resync {
                f * (g.age - g.len).min(0) as f32
            } else {
                0.0
            };
            let d0 = ((g.read - (s15 + g.phase)) as f64).max(1.0);
            let (l1, r1) = psola_read(ring, d0, f, remaining, tab);
            let s3 = (g.read - g.range_max).max(0.0);
            let q = g.period * 0.25;
            let w = smoothstep((s3 - g.phase) / q);
            let mut s10 = l1 * w;
            let mut s8 = r1 * w;
            if g.phase >= g.period && w16 {
                if g.cycle == 0 {
                    g.gain_next = g.gain;
                    g.read2 = g.period.mul_add(-fv62, g.read);
                    g.cycle = 1;
                }
                let s6 = g.phase - g.period;
                let d0b = ((g.read2 - (s15 + s6)) as f64).max(1.0);
                let (l2, r2v) = psola_read(ring, d0b, f, remaining, tab);
                let s16 = g.range_max - g.period;
                let s5 = (g.read2 - s16).max(0.0) - g.phase;
                let w2 = smoothstep(s5 / q);
                let (l2w, r2w) = (l2 * w2, r2v * w2);
                let x = smootherstep(s6 / g.period);
                let s16 = s10 * g.gain;
                s10 = ((g.gain_next * l2w) - s16).mul_add(x, s16);
                let s0b = g.gain * s8;
                s8 = ((g.gain_next * r2w) - s0b).mul_add(x, s0b);
            }
            let mut s0 = 0.0f32;
            if g.resync {
                s0 = (((g.age as f64 / g.len as f64) as f32) * 0.5).min(0.5);
            }
            if s0 < g.rate {
                s0 = g.rate;
            }
            let s0 = 1.0 - s0.mul_add(2.0, -1.0).abs();
            let wg = smootherstep(s0).min(budget);
            g.read += 1.0;
            g.read2 += 1.0;
            let ramp = g.resync && g.age < g.len;
            g.phase += if ramp { 0.0 } else { f };
            if g.phase >= g.period + g.period && w16 {
                g.phase -= g.period;
                let gn = g.gain_next;
                g.gain = gn;
                g.gain_next = if g.cycle & 1 == 0 { gn } else { -gn };
                g.read = g.read2;
                g.read2 = g.period.mul_add(-fv62, g.read2);
                g.cycle += 1;
            }
            if !ramp {
                g.rate += inc;
            }
            out_l = wg.mul_add(s10, out_l);
            out_r = wg.mul_add(s8, out_r);
            g.done = g.rate >= 1.0;
            g.age += 1;
            g.range_max += 1.0;
            g.pos += 1.0;
            let counts = if g.resync {
                true
            } else if idx == 0 {
                break;
            } else {
                self.grains[(self.g_tail + idx as usize - 1) % GRAINS].resync
            };
            if counts {
                budget -= wg;
                if budget < EPS_ONE {
                    // Drop the grains older than this one.
                    for _ in 0..idx {
                        if self.g_count == 0 {
                            break;
                        }
                        self.pop_grain();
                    }
                    break;
                }
            }
            idx -= 1;
        }
        (out_l, out_r)
    }
}

/// `PitchShifterTransientDetector` with its level-independent onset detector (vtable b38800):
/// an 82.4 Hz high-pass, a 5 kHz SVF split, log-domain flux followers and the onset logic.
struct TransientDetector {
    hp_g: f32,
    hp: [[f32; 4]; 2],
    ic: [[f32; 2]; 2],
    e: [f32; 8],
    h: [f32; 8],
    atk: f32,
    r10: f32,
    r5: f32,
    g: f32,
    k2: f32,
    hinv: f32,
    min_len: i64,
    flat_max: i64,
    oring: Vec<f32>,
    ridx: i64,
    prev: f32,
    floor: f32,
    peak: f32,
    active: bool,
    peakd: f32,
    peakcomb: f32,
    peakamp: f32,
    cntbelow: i64,
    cnt90: i64,
    cnt94: i64,
    fired: bool,
    scale: f32,
}

impl TransientDetector {
    /// The flux threshold (`DAT_0130ae78`).
    const FLUX_THR: f32 = 0.5;
    /// The amplitude threshold (`DAT_01309318`).
    const AMP_THR: f32 = 0.0;

    fn new(sr: f32) -> Self {
        let pole = |t: f32| 1.0 - (-1.0 / (sr * t)).exp();
        let w = f32::from_bits(0x459c_4000) / sr;
        let k = 1.0 / f32::from_bits(0x3fb5_04f3);
        let w2 = if w < 0.0 { 0.0 } else { w.min(0.5) * PI_F };
        let x2 = w2 * w2;
        let num = x2 + f32::from_bits(0xc3bd_0000);
        let num = x2.mul_add(num, f32::from_bits(0x4687_5a00));
        let num = x2.mul_add(num, f32::from_bits(0xc803_f7c0));
        let num = w2 * num;
        let den = x2.mul_add(28.0, f32::from_bits(0xc544_e000));
        let den = x2.mul_add(den, f32::from_bits(0x4773_a200));
        let den = x2.mul_add(den, f32::from_bits(0xc803_f7c0));
        let g = num / den;
        let k2 = k + k;
        let hh = g.mul_add(g, k2.mul_add(g, 1.0));
        let srd = sr as f64;
        Self {
            hp_g: tpt_k(82.4, sr),
            hp: [[0.0; 4]; 2],
            ic: [[0.0; 2]; 2],
            e: [-90.0; 8],
            h: [-90.0; 8],
            atk: pole(f32::from_bits(0x3b23_d70a)),
            r10: pole(f32::from_bits(0x3c23_d70a)),
            r5: pole(f32::from_bits(0x3ba3_d70a)),
            g,
            k2,
            hinv: 1.0 / hh,
            min_len: (srd * 0.009_999_999_776_482_582).ceil() as i64,
            flat_max: (srd * 0.005).ceil() as i64,
            oring: vec![0.0; (srd * 0.028) as usize],
            ridx: 0,
            prev: 0.0,
            floor: 0.0,
            peak: 0.0,
            active: true,
            peakd: 0.0,
            peakcomb: 0.0,
            peakamp: 0.0,
            cntbelow: 0,
            cnt90: 0,
            cnt94: 0,
            fired: false,
            scale: sr / 44100.0,
        }
    }

    fn hp(&mut self, c: usize, mut x: f32) -> f32 {
        for s in &mut self.hp[c] {
            let v = self.hp_g * (x - *s);
            let lp = *s + v;
            *s = v + lp;
            x -= lp;
        }
        x
    }

    fn svf(&mut self, c: usize, x: f32) -> (f32, f32) {
        let [ic1, ic2] = self.ic[c];
        let hp = (-self.k2).mul_add(ic1, x);
        let hp = (-self.g).mul_add(ic1, hp) - ic2;
        let hp = self.hinv * hp;
        let bp = self.g.mul_add(hp, ic1);
        let lp = self.g.mul_add(bp, ic2);
        self.ic[c] = [self.g.mul_add(hp, bp), self.g.mul_add(bp, lp)];
        (lp, hp)
    }

    fn flux(&mut self, xl: f32, xr: f32) -> f32 {
        const FLOOR: f32 = f32::from_bits(0x3800_0000);
        const C005: f32 = f32::from_bits(0x3d4c_cccd);
        const C1_15: f32 = f32::from_bits(0x3d88_8889);
        let (lpl, hpl) = self.svf(0, xl);
        let (lpr, hpr) = self.svf(1, xr);
        let lo = lpl.abs().max(lpr.abs());
        let hi = hpl.abs().max(hpr.abs());
        let lov = hi.mul_add(C005, lo);
        let hiv = lo.mul_add(C005, hi);
        let l6 = (if lov >= FLOOR { lov } else { FLOOR }).log2() * 6.0;
        let h6 = (if hiv >= FLOOR { hiv } else { FLOOR }).log2() * 6.0;
        let (atk, r10, r5) = (self.atk, self.r10, self.r5);
        let (e, h) = (&mut self.e, &mut self.h);
        let pn = |d: f32| (if d < 0.0 { 0.0 } else { d }, if d > 0.0 { 0.0 } else { d });
        let (p, q) = pn(l6 - e[0]);
        e[0] += p.mul_add(atk, r10 * q);
        for j in 1..4 {
            let (p, q) = pn(e[j - 1] - e[j]);
            e[j] += q.mul_add(r10, p);
        }
        let (p, q) = pn(h6 - h[0]);
        h[0] += p.mul_add(atk, r5 * q);
        for j in 1..4 {
            let (p, q) = pn(h[j - 1] - h[j]);
            h[j] += q.mul_add(r5, p);
        }
        for j in 4..8 {
            let (p, q) = pn(e[j - 1] - e[j]);
            e[j] += p.mul_add(r10, q);
        }
        for j in 4..8 {
            let (p, q) = pn(h[j - 1] - h[j]);
            h[j] += p.mul_add(r5, q);
        }
        (e[3] - e[7]) * C1_15 + (h[3] - h[7]) * C1_15
    }

    fn emit(&mut self) -> f32 {
        let i = self.ridx as usize;
        let out = self.oring[i];
        self.oring[i] = 0.0;
        self.ridx = (self.ridx + 1).rem_euclid(self.oring.len() as i64);
        out
    }

    fn onset(&mut self, al: f32, ar: f32, f: f32) -> f32 {
        let d = (f - self.prev) * self.scale;
        self.prev = f;
        if self.active {
            if d > self.peakd {
                self.cnt90 = 0;
            }
        } else {
            let fl = self.floor;
            self.floor = if fl < f { fl } else { f };
            if f <= fl + fl {
                return self.emit();
            }
            self.active = true;
            self.fired = false;
            self.peakd = 0.0;
            self.peakcomb = 0.0;
            self.cntbelow = 0;
            self.cnt90 = 0;
            self.peakamp = 0.0;
            self.cnt94 = 0;
            self.peak = f;
        }
        let peak = self.peak;
        let comb = d * (peak + 1.0);
        let cnt94 = if comb <= self.peakcomb {
            self.cnt94 + 1
        } else {
            1
        };
        if d >= self.peakd {
            self.peakd = d;
        }
        if comb >= self.peakcomb {
            self.peakcomb = comb;
        }
        let amp = al.abs().max(ar.abs());
        if amp >= self.peakamp {
            self.peakamp = amp;
        }
        self.cnt90 += 1;
        self.cnt94 = cnt94;
        self.cntbelow = if f <= peak { self.cntbelow + 1 } else { 0 };
        let mut fire = false;
        if f < peak * 0.5 || self.cntbelow > self.flat_max {
            self.active = false;
            self.floor = f;
            fire = true;
        } else if cnt94 >= self.min_len && self.peakamp >= Self::AMP_THR {
            fire = true;
        }
        if fire && peak >= Self::FLUX_THR && !self.fired {
            let m = self.cnt90.min(self.min_len);
            let idx = (self.min_len - m) + self.ridx;
            let r = idx.rem_euclid(self.oring.len() as i64) as usize;
            self.oring[r] = self.peakamp;
            self.fired = true;
        }
        if f >= peak {
            self.peak = f;
        }
        self.emit()
    }

    /// The in-block indices of the triggers of `x`.
    fn process(&mut self, x: &[[f32; 2]], out: &mut Vec<i64>) {
        out.clear();
        for (i, s) in x.iter().enumerate() {
            let hl = self.hp(0, s[0]);
            let hr = self.hp(1, s[1]);
            let f = self.flux(hl, hr);
            if self.onset(hl, hr, f) > 0.0 {
                out.push(i as i64);
            }
        }
    }
}

/// The band values of the Pitch shaper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchParams {
    /// `mode` of the shaper (`PitchState`, shared by all bands): 0 WSOLA (falls back to PSOLA
    /// Vocals), 1 PSOLA Vocals, 2 PSOLA Drums, 3 Cyclic. Default 0.
    pub mode: i32,
    /// `grainSizeCyclic` (band float, default 0.5): the Cyclic grain `sr (g^2 0.099 + 0.001)`.
    pub grain_size_cyclic: f32,
    /// `linkFormant` (band bool, default true): PSOLA ignores the formant section.
    pub link_formant: bool,
    /// `fillGaps` (band float, default 1).
    pub fill_gaps: f32,
}

impl Default for PitchParams {
    fn default() -> Self {
        Self {
            mode: 0,
            grain_size_cyclic: 0.5,
            link_formant: true,
            fill_gaps: 1.0,
        }
    }
}

/// The shifter of a mode: `(vocals, sync)` for PSOLA, `None` for Cyclic.
fn psola_mode(mode: i32, sr: f32) -> Option<(bool, i64)> {
    match mode {
        3 => None,
        2 => Some((false, (0.01 * sr as f64) as i64)),
        // Mode 0 (WSOLA) is not ported; it runs PSOLA Vocals.
        _ => Some((true, 0)),
    }
}

/// The dry-path latency of a mode in samples: the PSOLA latency `max(win + fade + sync + 7,
/// sync + ceil(2 half))` with `win = ceil(512 sr / 11025)`, `fade = int(0.003 sr)`,
/// `half = win / 2` and `sync = int(0.01 sr)` in Drums (0 in Vocals): 2381 / 2861 at 48 kHz;
/// 0 for Cyclic.
pub fn latency(sr: f32, mode: i32) -> usize {
    psola_mode(mode, sr).map_or(0, |(_, sync)| PsolaSizes::new(sr, sync).lat as usize)
}

/// The samples `a..b` of a wave.
fn sub_wave(w: Wave<'_>, a: usize, b: usize) -> Wave<'_> {
    match w {
        Wave::PerSample(s) => Wave::PerSample(&s[a..b]),
        c => c,
    }
}

/// One Pitch band (`PitchShaperModulation`).
pub struct PitchBand {
    sr: f32,
    tab: &'static Tables,
    max_block: usize,
    mode: i32,
    cyclic: Cyclic,
    psola: Box<Psola>,
    transients: TransientDetector,
    dc: [OnePole; 2],
    dc_k: f32,
    grain: Smoother,
    ratio: Vec<f32>,
    formant: Vec<f32>,
    trans: Vec<i64>,
    dry: Vec<[f32; 2]>,
    dry_w: usize,
}

impl PitchBand {
    pub fn new(sr: f32, max_block: usize, p: &PitchParams) -> Self {
        let mut grain = Smoother::new(p.grain_size_cyclic, sr as f64, 0.0);
        grain.buf.reserve(max_block);
        let (vocals, sync) = psola_mode(p.mode, sr).unwrap_or((true, 0));
        let max_lat = latency(sr, 2).max(latency(sr, 1));
        Self {
            sr,
            tab: tables(),
            max_block,
            mode: p.mode,
            cyclic: Cyclic::new(sr),
            psola: Box::new(Psola::new(sr, vocals, sync)),
            transients: TransientDetector::new(sr),
            dc: [OnePole::default(); 2],
            dc_k: tpt_k(10.0, sr),
            grain,
            ratio: vec![1.0; max_block],
            formant: vec![1.0; max_block],
            trans: Vec::with_capacity(max_block),
            dry: vec![[0.0; 2]; next_pow2(max_lat + 1)],
            dry_w: 0,
        }
    }

    /// The latency of the dry path in samples for the mode of the last block ([`latency`]).
    pub fn latency(&self) -> usize {
        latency(self.sr, self.mode)
    }

    /// Processes the band audio `buf` in place. `pitch` and `formant` are the section ports
    /// (after their envelope followers); `transient_src` is the unsplit shaper input that the
    /// transient detector of modes 0 and 2 listens to (`None`: the band audio).
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        pitch: Wave,
        formant: Wave,
        p: &PitchParams,
        transient_src: Option<&[[f32; 2]]>,
    ) {
        let mut a = 0;
        while a < buf.len() {
            let b = buf.len().min(a + self.max_block);
            let src = transient_src.map(|s| &s[a..b]);
            let (pw, fw) = (sub_wave(pitch, a, b), sub_wave(formant, a, b));
            self.process_block(&mut buf[a..b], pw, fw, p, src);
            a = b;
        }
    }

    fn process_block(
        &mut self,
        buf: &mut [[f32; 2]],
        pitch: Wave,
        formant: Wave,
        p: &PitchParams,
        transient_src: Option<&[[f32; 2]]>,
    ) {
        let n = buf.len();
        self.grain.process(p.grain_size_cyclic, n);
        for (dst, w) in [(&mut self.ratio, pitch), (&mut self.formant, formant)] {
            match w {
                Wave::Constant(c) => dst[..n].fill(ratio_from_wave(c)),
                Wave::PerSample(s) => {
                    for (d, v) in dst[..n].iter_mut().zip(s) {
                        *d = ratio_from_wave(*v);
                    }
                }
            }
        }
        let mode = p.mode.clamp(0, 3);
        self.mode = mode;
        if mode == 0 || mode == 2 {
            self.transients
                .process(transient_src.unwrap_or(&*buf), &mut self.trans);
        } else {
            self.trans.clear();
        }
        match psola_mode(mode, self.sr) {
            None => self
                .cyclic
                .process(buf, &self.ratio[..n], &self.grain, self.tab),
            Some((vocals, sync)) => {
                let ps = &mut self.psola;
                if ps.vocals != vocals || ps.sync != sync {
                    ps.set_mode(vocals, sync);
                }
                ps.process(
                    buf,
                    &self.ratio[..n],
                    &self.formant[..n],
                    &self.trans,
                    p.link_formant,
                    p.fill_gaps,
                    self.tab,
                );
            }
        }
        if mode == 2 && !p.link_formant {
            // The 10 Hz DC blocker of Drums without formant link.
            for f in buf.iter_mut() {
                for (x, op) in f.iter_mut().zip(&mut self.dc) {
                    *x -= op.lp(*x, self.dc_k);
                }
            }
        }
    }

    /// Delays the band's dry signal by [`Self::latency`] so the band mix lines up; mix it with
    /// the send-style law (`super::reverb::send_mix`).
    pub fn delay_dry(&mut self, dry: &mut [[f32; 2]]) {
        let lat = self.latency();
        let mask = self.dry.len() - 1;
        for f in dry.iter_mut() {
            self.dry[self.dry_w] = *f;
            *f = self.dry[(self.dry_w + self.dry.len() - lat) & mask];
            self.dry_w = (self.dry_w + 1) & mask;
        }
    }
}
