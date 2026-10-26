//! Drive (DriveShaperModulation::process 0x4b4844; shapers_a.md section 9): per band, input gain
//! (0x4b658c), the grip stage (0x5030b8) and the style curve (0x4fc0f8), the tone tilt
//! (0x4be158), a 10 Hz DC blocker and the output gain (0x4bea54). The curves are the closed forms
//! of `.amp/in/sb/scripts/drive_curves.py`, recovered from the disassembly and checked against
//! the measured tables to 1e-5.
//!
//! Below 88.2 kHz the plugin runs the curve 4x oversampled (0x4baaec), and the filters run even
//! when the curve is bypassed. Their coefficients were not extracted: `OS_FIR` is fitted by
//! `.amp/in/sb/scripts/drive_os_fit.py` so that the whole path reproduces the measured linear
//! response (flat to 14 kHz, -6 dB near 19.5 kHz) to 4e-4. The path has `LATENCY` samples of
//! delay, which the plugin hides inside its 16-sample base latency. The 2x path between 88.2 and
//! 176.4 kHz is not modelled (no oversampling there).

use super::{OnePole, Wave, tpt_k};
use crate::smoother::Smoother;

/// The oversampling factor and the fitted interpolation and decimation filter at the 4x rate.
const OS: usize = 4;
const OS_FIR: [f32; 45] = [
    -4.9322117e-03,
    5.436647e-03,
    5.61359e-03,
    1.3297825e-03,
    -1.2623847e-03,
    -5.943905e-03,
    -7.2553083e-03,
    -5.905892e-03,
    -1.3516288e-03,
    5.404911e-03,
    1.3686406e-02,
    1.7521964e-02,
    1.0354355e-02,
    -1.5014623e-03,
    -2.1486435e-02,
    -3.7217166e-02,
    -3.5280135e-02,
    -1.8215664e-02,
    2.5652777e-02,
    8.7875165e-02,
    1.5452556e-01,
    2.0268688e-01,
    2.2090913e-01,
    2.0268688e-01,
    1.5452556e-01,
    8.7875165e-02,
    2.5652777e-02,
    -1.8215664e-02,
    -3.5280135e-02,
    -3.7217166e-02,
    -2.1486435e-02,
    -1.5014623e-03,
    1.0354355e-02,
    1.7521964e-02,
    1.3686406e-02,
    5.404911e-03,
    -1.3516288e-03,
    -5.905892e-03,
    -7.2553083e-03,
    -5.943905e-03,
    -1.2623847e-03,
    1.3297825e-03,
    5.61359e-03,
    5.436647e-03,
    -4.9322117e-03,
];

/// The delay of the oversampled path in base-rate samples (`2 * 22 / 4`).
pub const LATENCY: usize = (OS_FIR.len() - 1) / OS;

/// Taps per polyphase branch of the interpolator.
const BRANCH: usize = OS_FIR.len().div_ceil(OS);

/// One channel of the 4x interpolator and decimator around the curve.
#[derive(Clone, Copy)]
struct Oversampler {
    /// The last `BRANCH` base-rate inputs, newest first.
    x: [f32; BRANCH],
    /// The last `OS_FIR.len()` 4x-rate curve outputs, newest first.
    v: [f32; OS_FIR.len()],
}

impl Oversampler {
    fn new() -> Self {
        Self {
            x: [0.0; BRANCH],
            v: [0.0; OS_FIR.len()],
        }
    }

    /// Pushes one input, maps the four interpolated samples through `f` and returns the
    /// decimated output.
    fn tick(&mut self, x: f32, mut f: impl FnMut(f32) -> f32) -> f32 {
        self.x.copy_within(..BRANCH - 1, 1);
        self.x[0] = x;
        let mut out = 0.0;
        for p in 0..OS {
            let mut u = 0.0;
            for (j, xj) in self.x.iter().enumerate() {
                if let Some(h) = OS_FIR.get(p + OS * j) {
                    u += h * xj;
                }
            }
            self.v.copy_within(..OS_FIR.len() - 1, 1);
            self.v[0] = f(u * OS as f32);
            if p == 0 {
                // The decimated sample lines up with phase 0: a whole-sample latency.
                out = OS_FIR.iter().zip(&self.v).map(|(h, v)| h * v).sum();
            }
        }
        out
    }
}

/// Internal style indices (the switch of 0x4fc0f8).
pub const SOFT_CLIP: i32 = 0;
pub const SMOOTH_CLIP: i32 = 1;
pub const HARD_CLIP: i32 = 2;
pub const SOFT_SQUARE: i32 = 3;
pub const HARD_SQUARE: i32 = 4;
pub const SOFT_RECTIFY: i32 = 5;
pub const HARD_RECTIFY: i32 = 6;
pub const SKEW_FOLD: i32 = 7;
pub const SINE_FOLD: i32 = 8;
pub const TRIANGLE_FOLD: i32 = 9;
pub const SAW_FOLD: i32 = 10;
pub const EXTREME_FOLD: i32 = 11;

/// The preset `style` value to the internal style: presets skip Smooth Clip, so 0 is Soft Clip
/// and `s >= 1` is internal `s + 1`; preset 11 (internal 12) is the identity.
pub fn internal_style(preset: i32) -> i32 {
    if preset <= 0 { preset } else { preset + 1 }
}

/// `u (27 + u^2) / (27 + 9 u^2)` on `u` clamped to [-3, 3], clamped to [-1, 1].
fn pade(u: f64) -> f64 {
    let u = u.clamp(-3.0, 3.0);
    (u * (27.0 + u * u) / (27.0 + 9.0 * u * u)).clamp(-1.0, 1.0)
}

/// `copysign(1, t)`.
fn sgn(t: f64) -> f64 {
    1f64.copysign(t)
}

/// The shared output normalisation: `c = 0.23 / ((a + b) scale)`, `g = c (gain - 1)`.
fn norm_k(c: f64, gain: f64, atan: bool) -> f64 {
    let g = c * (gain - 1.0);
    let l = g.ln_1p();
    let m = if atan { l + (g.atan() - l) * 0.8 } else { l };
    1.0 / (m / c + 1.0)
}

/// `a + b` of the accent: `max(0.5, 1.5 - 2 accent) + min(0.5, 1 - accent)`.
fn accent_ab(accent: f64) -> f64 {
    (1.5 - 2.0 * accent).max(0.5) + (1.0 - accent).min(0.5)
}

fn accent_c(accent: f64, scale: f64) -> f64 {
    0.23 / (accent_ab(accent) * scale)
}

/// 0x5030b8, skipped at grip 0.
fn grip_stage(x: f64, w: f64, grip: f64) -> f64 {
    if grip == 0.0 {
        return x;
    }
    let m = (0.5 * grip * (1.0 + w.clamp(0.0, 1.0))).min(1.0);
    let ax = x.abs();
    let lim = if m > 1e-12 {
        (m + 1.0) * m * (ax / m).powf(3.0 * m + 1.0)
    } else {
        ax
    };
    (1.0 + 0.25 * m) * ax.min(lim).copysign(x)
}

/// First-order antiderivative anti-aliased hard clip with one double of state (styles 2, 4).
fn adaa(prev: &mut f64, v: f64) -> f64 {
    let p = *prev;
    *prev = v;
    let d = v - p;
    let direct = v.clamp(-1.0, 1.0);
    if d == 0.0 {
        return direct;
    }
    let f = |t: f64| t.abs().max(1.0) + 0.5 * (t * t).min(1.0) - 1.0;
    let a = (f(v) - f(p)) / d;
    a + (direct - a) / ((d.abs() as f32) as f64 * 10000.0 + 1.0)
}

fn soft_rect_shape(t: f64) -> f64 {
    let t = t * pade(3.0 * t);
    let t = t + 0.3324 * t.powi(3) + 0.217 * t.powi(5);
    pade(t)
}

fn skew_shape(t: f64) -> f64 {
    let t = t * 1.0098;
    let t = t + 0.3324 * t.powi(3) + 0.217 * t.powi(5);
    let mut a = pade(t) * 0.990_295_2;
    for _ in 0..4 {
        a = 4.0 * a * (a.abs() - 1.0);
    }
    a
}

fn sine_shape(t: f64) -> f64 {
    if t.abs() <= 1.0 {
        t
    } else {
        sgn(t) + (t - sgn(t)).sin()
    }
}

fn tri_shape(t: f64) -> f64 {
    sgn(t) * (1.0 - (2.0 * (t - t.floor()) - 1.0).abs())
}

/// The per-sample parameters of one curve evaluation, shared by both channels.
#[derive(Clone, Copy, Debug)]
pub struct Curve {
    style: i32,
    w: f64,
    gain: f64,
    k: f64,
    off: f64,
    grip: f64,
    push: f64,
}

impl Curve {
    /// Precomputes the gain, normalisation and offset of `style` at drive `w`.
    pub fn new(style: i32, w: f32, accent: f32, grip: f32, push: f32) -> Self {
        let (w, accent, push) = (w as f64, accent as f64, push as f64);
        let c = |scale| accent_c(accent, scale);
        let (gain, k, off) = match style {
            SOFT_CLIP | HARD_CLIP => {
                let g = (1.0 + w).powi(8);
                (g, norm_k(c(0.5), g, true), 0.0)
            }
            SMOOTH_CLIP => {
                let g = (8.0 * w).exp2();
                (g, norm_k(c(0.5), g, true), 0.0)
            }
            SOFT_SQUARE => {
                let g = 1.0 + 6.0 * w * w;
                (g, norm_k(c(0.4), g, true), 0.0)
            }
            HARD_SQUARE => (1.0, norm_k(c(1.0), 1.0 + 4.0 * w * w, true), 0.0),
            SOFT_RECTIFY | HARD_RECTIFY => {
                let g = (1.0 + w).powi(4);
                (g, norm_k(c(0.5), g, true), -0.5 * push)
            }
            SKEW_FOLD => {
                let g1 = 0.995 * w.powi(3) + 0.005;
                let cs = 0.23 / ((1.0 - 0.1 * push) * 0.43 * accent_ab(accent));
                (g1, norm_k(cs, g1 / 0.005, true), (g1 + 0.035) * 0.1 * push)
            }
            SINE_FOLD => {
                let g = (1.0 + 1.798_787_8 * w).powi(4);
                (g, norm_k(c(0.5), g, true), 5.712_389 * push)
            }
            TRIANGLE_FOLD => {
                let g = (1.0 + 1.5 * w).powi(4);
                (g, norm_k(c(0.5), g, true), push)
            }
            SAW_FOLD => {
                let g = 1.0 + 21.627_417 * w * w;
                let we = w + (0.3 * push - w) * (1.0 - w).powi(8);
                let k = norm_k(c(0.25), 1.0 + 21.627_417 * we * we, false);
                (g, k, 0.99 * (1.0 - (1.0 - push) * (1.0 - push)))
            }
            EXTREME_FOLD => {
                let g = 1.0 + 6.5 * w * w;
                let gk = g + 0.5 * push * (w + 0.8);
                (g, norm_k(c(0.333), gk.powi(3), false), 0.0)
            }
            _ => (1.0, 1.0, 0.0),
        };
        Self {
            style,
            w,
            gain,
            k,
            off,
            grip: grip as f64,
            push,
        }
    }

    /// One sample; `adaa_state` is the channel's ADAA memory (styles 2 and 4).
    pub fn apply(&self, x: f32, adaa_state: &mut f64) -> f32 {
        if !(0..=11).contains(&self.style) {
            return x;
        }
        let x = grip_stage(x as f64, self.w, self.grip);
        let (g, k, o) = (self.gain, self.k, self.off);
        let y = match self.style {
            SOFT_CLIP => {
                let v = x * g;
                let u = if v > 0.0 {
                    v * 0.666_666_7
                } else {
                    let m = -v;
                    -(m + m * m + m.powi(3) + m.powi(4) + m.powi(5))
                };
                let s = u / (u.abs() + 1.0);
                k * if s <= 0.0 { s } else { 1.5 * s }
            }
            SMOOTH_CLIP => {
                let v = x * g;
                let t = 0.85 + 0.12 * self.w;
                let av = v.abs();
                if av >= t {
                    k * (1.0 - (1.0 - t) * (1.0 - t) / (av + 2.0 * (1.0 - t) - 1.0)).copysign(v)
                } else {
                    k * v
                }
            }
            HARD_CLIP => k * adaa(adaa_state, ((x * g) as f32) as f64),
            SOFT_SQUARE => k * x.abs().powf(1.0 / g).copysign(x),
            HARD_SQUARE => {
                let sq = adaa(adaa_state, ((x * 10000.0) as f32) as f64);
                k * pade(x + sq * self.w * self.w)
            }
            SOFT_RECTIFY => {
                2.0 * k * (soft_rect_shape(o + 0.5 * g * x + 0.5) - soft_rect_shape(o + 0.5))
            }
            HARD_RECTIFY => {
                2.0 * k * ((o + 0.5 * g * x + 0.5).abs().min(1.0) - (o + 0.5).abs().min(1.0))
            }
            SKEW_FOLD => k * (skew_shape(o + g * x) - skew_shape(o)),
            SINE_FOLD => 0.5 * k * (sine_shape(o + 2.0 * g * x) - sine_shape(o)),
            TRIANGLE_FOLD => k * (tri_shape(o + 0.5 * g * x) - tri_shape(o)),
            SAW_FOLD => {
                // f32 like the plugin, so the wrap points land on the same samples.
                let t = (g as f32).mul_add(x as f32, o as f32);
                k * ((t - t.trunc()) as f64 - o)
            }
            _ => {
                let gs = g * 0.325_732_92;
                let mut v = (x + pade(10.0 * x) * 0.5 * self.push) * gs;
                for stage in 0..3 {
                    v += 0.21 * v.powi(3);
                    let q = 3.07 * pade(v);
                    v = q - 0.15 * q.powi(3);
                    if stage < 2 {
                        v *= gs;
                    }
                }
                k * v
            }
        };
        y as f32
    }
}

/// `2^(dB / 6)` with `dB = hi max(v, 0) + lo min(v, 0)`, `v = 2 (p - 0.5)` (0x4b658c, 0x4bea54).
fn gain(p: f32, hi: f32, lo: f32) -> f32 {
    let v = (p - 0.5) + (p - 0.5);
    ((v.max(0.0) * hi + v.min(0.0) * lo) * 0.166_666_67).exp2()
}

/// The band parameters other than the drive wave.
#[derive(Clone, Copy, Debug)]
pub struct DriveParams {
    /// The internal style (`internal_style` of the preset value).
    pub style: i32,
    pub input_gain: f32,
    pub output_gain: f32,
    pub push: f32,
    pub grip: f32,
    pub tone: f32,
    pub accent: f32,
}

/// One Drive band.
pub struct DriveBand {
    sr: f32,
    os: Option<[Oversampler; 2]>,
    /// The band's dry signal delayed by the oversampling latency, for the band mix.
    dry: Vec<[f32; 2]>,
    adaa: [f64; 2],
    tone_lp: [[OnePole; 2]; 2],
    dc: [OnePole; 2],
    input: Smoother,
    output: Smoother,
    push: Smoother,
    grip: Smoother,
    tone: Smoother,
    accent: Smoother,
}

impl DriveBand {
    pub fn new(sr: f32, p: DriveParams) -> Self {
        let s = |v| Smoother::new(v, sr as f64, 0.0);
        Self {
            sr,
            os: (sr < 88200.0).then(|| [Oversampler::new(); 2]),
            dry: Vec::new(),
            adaa: [0.0; 2],
            tone_lp: [[OnePole::default(); 2]; 2],
            dc: [OnePole::default(); 2],
            input: s(p.input_gain),
            output: s(p.output_gain),
            push: s(p.push),
            grip: s(p.grip),
            tone: s(p.tone),
            accent: s(p.accent),
        }
    }

    /// `w` is the drive wave; a constant `w <= 0` (preset point -1) bypasses the curve.
    pub fn process(&mut self, buf: &mut [[f32; 2]], w: Wave, p: DriveParams) {
        let n = buf.len();
        self.input.process(p.input_gain, n);
        self.output.process(p.output_gain, n);
        self.push.process(p.push, n);
        self.grip.process(p.grip, n);
        self.tone.process(p.tone, n);
        self.accent.process(p.accent, n);
        let ramp = |s: &Smoother, v: f32| s.ramping || s.value != v;
        let pos = |s: &Smoother| s.ramping || s.value > 0.0;
        let drive_on = !matches!(w, Wave::Constant(v) if v <= 0.0);
        let shaper_on = drive_on
            || ramp(&self.input, 0.5)
            || pos(&self.grip)
            || ((SOFT_RECTIFY..=EXTREME_FOLD).contains(&p.style) && pos(&self.push));
        let tone_on = ramp(&self.tone, 0.5) && !matches!(w, Wave::Constant(v) if v == 0.0);
        for (i, f) in buf.iter_mut().enumerate() {
            let g = gain(self.input.at(i), 36.0, 12.0);
            f[0] *= g;
            f[1] *= g;
        }
        if let Some(os) = self.os.as_mut() {
            let mut prev = None;
            for (i, f) in buf.iter_mut().enumerate() {
                let c = shaper_on.then(|| {
                    let key = (w.at(i), self.accent.at(i), self.grip.at(i), self.push.at(i));
                    let c = match prev {
                        Some((k, c)) if k == key => c,
                        _ => Curve::new(p.style, key.0, key.1, key.2, key.3),
                    };
                    prev = Some((key, c));
                    c
                });
                for ((v, os), adaa) in f.iter_mut().zip(os.iter_mut()).zip(&mut self.adaa) {
                    *v = os.tick(*v, |x| match &c {
                        Some(c) => c.apply(x, adaa),
                        None => x,
                    });
                }
            }
        } else if shaper_on {
            let mut prev = None;
            for (i, f) in buf.iter_mut().enumerate() {
                let key = (w.at(i), self.accent.at(i), self.grip.at(i), self.push.at(i));
                let c = match prev {
                    Some((k, c)) if k == key => c,
                    _ => Curve::new(p.style, key.0, key.1, key.2, key.3),
                };
                prev = Some((key, c));
                for (v, adaa) in f.iter_mut().zip(&mut self.adaa) {
                    *v = c.apply(*v, adaa);
                }
            }
        }
        if tone_on {
            self.tone_tilt(buf, w);
        } else if let Some(last) = buf.last() {
            // The skipped stage parks its states on the last sample.
            for (lp, v) in self.tone_lp.iter_mut().zip(last) {
                *lp = [OnePole::at(*v); 2];
            }
        }
        let k_dc = tpt_k(10.0, self.sr);
        for (i, f) in buf.iter_mut().enumerate() {
            let g = gain(self.output.at(i), 12.0, 36.0);
            for (v, dc) in f.iter_mut().zip(&mut self.dc) {
                *v = (*v - dc.lp(*v, k_dc)) * g;
            }
        }
    }

    /// The latency of this band in samples.
    pub fn latency(&self) -> usize {
        if self.os.is_some() { LATENCY } else { 0 }
    }

    /// Delays the band's dry signal by `latency` so that the band mix lines up.
    pub fn delay_dry(&mut self, dry: &mut [[f32; 2]]) {
        let l = self.latency();
        if l == 0 {
            return;
        }
        if self.dry.len() != l {
            self.dry = vec![[0.0; 2]; l];
        }
        for f in dry.iter_mut() {
            let out = self.dry[0];
            self.dry.copy_within(1.., 0);
            self.dry[l - 1] = *f;
            *f = out;
        }
    }

    /// 0x4be158: two one-poles at `20.6 * 2^(10 min(0.95, 1.8 t + 0.05))` and
    /// `20.6 * 2^(10 max(0.05, 1.8 t - 0.85))` Hz, a tilt `a = c(20 t - 9.5)` between them, mixed in
    /// by `|a - 0.5| * 2 w`.
    fn tone_tilt(&mut self, buf: &mut [[f32; 2]], w: Wave) {
        const F0: f64 = 20.601_722_307_054_37;
        let hz = |x: f32| (F0 * (1024f64.ln() * x as f64).exp()) as f32;
        let mut prev = f32::NAN;
        let (mut k1, mut k2, mut a) = (0.0, 0.0, 0.0);
        for (i, f) in buf.iter_mut().enumerate() {
            let t = self.tone.at(i);
            if t != prev {
                prev = t;
                k1 = tpt_k(hz((t * 1.8 + 0.05).min(0.95)), self.sr);
                k2 = tpt_k(hz((t * 1.8 - 0.85).max(0.05)), self.sr);
                a = (t * 20.0 - 9.5).clamp(0.0, 1.0);
            }
            let m = (a - 0.5).abs() * (w.at(i) + w.at(i));
            for (v, [l1, l2]) in f.iter_mut().zip(&mut self.tone_lp) {
                let x = *v;
                let y1 = l1.lp(x, k1);
                let y2 = l2.lp(x, k2);
                *v = x + m * ((y1 + a * ((x - y2) - y1)) - x);
            }
        }
    }
}
