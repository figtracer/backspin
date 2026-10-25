//! Liquid: the FilterWrapper's phaser and flanger, types 21..30 (shapers_b.md section 1; wrapper
//! process 0x380a38, allpass kernels 0x30ed48.., comb kernels 0x3cc5e0 / 0x3cc3f4).

use super::Wave;
use crate::smoother::Smoother;

/// `DAT_01328cc0`.
const E0: f64 = 20.601_722_307_054_37;
/// `DAT_0130dc00` = ln(1024).
const LN1024: f64 = 6.931_471_805_599_453;
/// The clamp of `pi f / sr` before the tangent.
const WMAX: f64 = 1.570_482_167_529_537_5;

fn clamp01(x: f32) -> f32 {
    if x.is_nan() || x < 0.0 {
        0.0
    } else if x <= 1.0 {
        x
    } else {
        1.0
    }
}

/// The plugin's Pade tangent in float32.
fn tan_pade(w: f32) -> f32 {
    let w2 = w * w;
    let num = ((((w2 - 378.0) * w2) + 17325.0) * w2 - 135135.0) * w;
    let den = (((28.0 * w2) - 3150.0) * w2 + 62370.0) * w2 - 135135.0;
    num / den
}

/// `(tan w - 1) / (tan w + 1)` as the plugin computes it.
fn allpass_coef(w: f64) -> f32 {
    let t = tan_pade(w as f32);
    let t2 = t + t;
    (t2 - 2.0) / (t2 + 2.0)
}

fn freq_w(c: f32, base_w: f64) -> f64 {
    (base_w * (LN1024 * c as f64).exp()).clamp(0.0, WMAX)
}

/// The cubic soft clip `u (u^2 + 27) / (9 u^2 + 27)` of `clamp(v, -3, 3)`, clamped to [-1, 1]
/// (fused as in the kernels: `c * fma(c, c, 27) / fma(9c, c, 27)`).
fn sat(v: f32) -> f32 {
    let c = v.clamp(-3.0, 3.0);
    let p = (c * c.mul_add(c, 27.0)) / (c * 9.0).mul_add(c, 27.0);
    p.clamp(-1.0, 1.0)
}

/// `fma(sat(v), 0.5, v * 0.5) / v`, 1 for `v == 0`.
fn soft_gain(v: f32) -> f32 {
    if v == 0.0 {
        1.0
    } else {
        sat(v).mul_add(0.5, v * 0.5) / v
    }
}

fn soft_clip(v: f32) -> f32 {
    sat(v).mul_add(0.5, v * 0.5)
}

/// `x = 2s - 1`, `d = copysign(x^2 / 2, x)`: the L/R exponent offset.
fn spread(s: f32) -> f32 {
    let x = s * 2.0 - 1.0;
    ((x * x) * 0.5).copysign(x)
}

/// N first-order allpasses with zero-delay feedback and soft limiting.
#[derive(Clone)]
struct AllpassChain {
    s: Vec<f32>,
    y1: f32,
}

impl AllpassChain {
    fn new(n: usize) -> Self {
        Self {
            s: vec![0.0; n],
            y1: 0.0,
        }
    }

    /// One sample, in the fused order of the kernels (0x30ed48 and the 8/12/16-stage variants).
    fn process(&mut self, x: f32, a: f32, g: f32) -> f32 {
        let n = self.s.len();
        let s = &mut self.s;
        let a2 = a * a;
        let a4 = a2 * a2;
        let an = match n {
            4 => a4,
            8 => a4 * a4,
            12 => a4 * (a4 * a4),
            _ => {
                let a8 = a4 * a4;
                a8 * a8
            }
        };
        let k1 = soft_gain(self.y1 * g);
        let mut acc = s[0];
        for v in &s[1..n] {
            acc = a.mul_add(acc, *v);
        }
        let yhat = an.mul_add(x, acc);
        let ang = an * g;
        let t = (yhat / ang.mul_add(k1, 1.0)) * g;
        let k2 = soft_gain(t);
        let y = yhat / ang.mul_add(k2, 1.0);
        let u = (-(k2 * g)).mul_add(y, x);
        let mut ts = [0.0f32; 16];
        let mut prev = u;
        for i in 0..n - 1 {
            ts[i] = a.mul_add(prev, s[i]);
            prev = ts[i];
        }
        self.y1 = y;
        s[0] = (-a).mul_add(ts[0], u);
        for i in 1..n - 1 {
            s[i] = (-a).mul_add(ts[i], ts[i - 1]);
        }
        s[n - 1] = (-a).mul_add(y, ts[n - 2]);
        y
    }
}

/// The comb filter of the flanger.
#[derive(Clone)]
struct Comb {
    buf: [Vec<f32>; 2],
    mask: usize,
    w: [usize; 2],
    lp: [f32; 2],
    hp_g: f32,
}

impl Comb {
    fn new(sr: f32) -> Self {
        let n = (sr as f64 / E0) as usize + 0x1ff;
        let mut size = 1;
        while size < n + 1 {
            size *= 2;
        }
        let t = (100.0 / sr).min(0.4999);
        let t = ((t * std::f32::consts::PI) as f64).tan() as f32;
        let t = t.max(0.0);
        Self {
            buf: [vec![0.0; size], vec![0.0; size]],
            mask: size - 1,
            w: [0; 2],
            lp: [0.0; 2],
            hp_g: t / (t + 1.0),
        }
    }

    fn tap(&self, ch: usize, d: f32) -> f32 {
        let i = d as usize;
        let i1 = (i + 1).min(self.mask);
        let b = &self.buf[ch];
        let w = self.w[ch];
        let y0 = b[(w.wrapping_sub(i1)) & self.mask];
        let y1 = b[(w.wrapping_sub(i)) & self.mask];
        y0 + (((i as f32) - d) + 1.0) * (y1 - y0)
    }
}

/// One Liquid band.
pub struct LiquidBand {
    sr: f32,
    chains: Option<(usize, [AllpassChain; 2])>,
    comb: Option<Comb>,
}

impl LiquidBand {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            chains: None,
            comb: None,
        }
    }

    /// `type_with_bypass` 0 off, 1 flanger, 2 phaser; `stages` 0..3 = 4, 8, 12, 16.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        center: Wave,
        feedback: Wave,
        type_with_bypass: i32,
        mode: i32,
        stages: i32,
        stereo_offset: &Smoother,
    ) {
        let base_w = E0 * std::f64::consts::PI / self.sr as f64;
        match type_with_bypass {
            2 => {
                let n = [4, 8, 12, 16][stages.clamp(0, 3) as usize];
                if self.chains.as_ref().map(|c| c.0) != Some(n) {
                    self.chains = Some((n, [AllpassChain::new(n), AllpassChain::new(n)]));
                }
                let chains = &mut self.chains.as_mut().unwrap().1;
                let fbmax: f32 = if mode == 0 { 0.99 } else { 0.55 };
                for (i, f) in buf.iter_mut().enumerate() {
                    let d = spread(stereo_offset.at(i));
                    let c = center.at(i);
                    let al = allpass_coef(freq_w(c + d, base_w));
                    let ar = allpass_coef(freq_w(c - d, base_w));
                    let fbw = feedback.at(i);
                    if mode == 0 {
                        let g = clamp01(fbw) * fbmax;
                        let s = (g * -0.5).mul_add(g, 1.0);
                        f[0] = chains[0].process(f[0] * s, al, g);
                        f[1] = chains[1].process(f[1] * s, ar, g);
                    } else {
                        let g = clamp01(1.0 - fbw) * fbmax;
                        let yl = chains[0].process(f[0], al, g);
                        let yr = chains[1].process(f[1], ar, g);
                        let n1 = g * g + 1.0;
                        f[0] = (yl * 0.5) * n1 + f[0] * 0.5;
                        f[1] = (yr * 0.5) * n1 + f[1] * 0.5;
                    }
                }
            }
            1 => {
                let comb = self.comb.get_or_insert_with(|| Comb::new(self.sr));
                for (i, f) in buf.iter_mut().enumerate() {
                    let d = spread(stereo_offset.at(i));
                    let c = center.at(i);
                    let delay = |p: f32| {
                        let p = clamp01(p);
                        (self.sr / ((p as f64 * LN1024).exp() * E0) as f32).max(1.0)
                    };
                    let dl = [delay(c + d), delay(c - d)];
                    let fbg = clamp01(feedback.at(i)).sqrt() * 0.99;
                    let k = 0.75 / (fbg * 0.75 + 1.0);
                    for ch in 0..2 {
                        let tp = comb.tap(ch, dl[ch]);
                        let v = (tp - comb.lp[ch]) * comb.hp_g;
                        let lp = comb.lp[ch] + v;
                        comb.lp[ch] = v + lp;
                        let hp = tp - lp;
                        let x = f[ch];
                        let (fbin, o) = if mode == 0 {
                            let fbin = x + hp * fbg;
                            (fbin, k * (hp + fbin))
                        } else {
                            let fbin = x - hp * fbg;
                            (fbin, k * (fbin - hp))
                        };
                        comb.buf[ch][comb.w[ch]] = soft_clip(fbin);
                        comb.w[ch] = (comb.w[ch] + 1) & comb.mask;
                        f[ch] = o;
                    }
                }
            }
            _ => {}
        }
    }
}
