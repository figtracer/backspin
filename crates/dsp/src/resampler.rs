//! The read head and its windowed-sinc interpolator (TBRResampler, dsp.md sections 3.4 to 4).

use crate::{OnePole, Ring, fmaxnm, vforce};
use std::f64::consts::PI;

/// pi/2 (0x354c48).
const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;
/// The lower clamp of the fractional read position (0x3abef2d0f5da7dd9).
const MIN_FRAC: f64 = f64::from_bits(0x3abe_f2d0_f5da_7dd9);
/// 2pi as f32 (0x40c90fdb).
const TWO_PI_F32: f32 = f32::from_bits(0x40c9_0fdb);
/// The window constants 0.36 and 0.64 as f32 (0x3eb851ec, 0x3f23d70a).
const W036: f32 = f32::from_bits(0x3eb8_51ec);
const W064: f32 = f32::from_bits(0x3f23_d70a);

/// The read head: position, kernel settings and work buffers.
#[derive(Clone, Debug)]
pub struct Resampler {
    /// Kernel half-width `N` (+0xec): 8 or 16 taps per side.
    taps: usize,
    /// `pi / N` (+0x128).
    pi_n: f64,
    /// `2 cos(pi / N)` (+0x140).
    c2n: f64,
    /// Cutoff `omega` (+0x130) and `2 cos(omega)` (+0x138) of the constant mode.
    omega: f64,
    c2w: f64,
    /// Per-sample cutoff mode (+0x178).
    pub per_sample: bool,
    /// Read position in ring frames (+0x170), advanced by one per rendered sample.
    pub pos: u32,
    /// Ring length (+0x174).
    pub len: u32,
    /// The speed smoother of the HQ cutoff (+0x190).
    pub speed: OnePole,
    /// Per-sample cutoffs and `2 cos` values for the per-sample mode (+0x180, +0x188).
    pub omegas: Vec<f64>,
    pub c2s: Vec<f64>,
    frac: Vec<f64>,
    idx: Vec<usize>,
    args: Vec<f64>,
    sines: Vec<f64>,
}

impl Resampler {
    /// Creates a read head with `taps` taps per side (FUN_0006bd10).
    pub fn new(taps: usize) -> Self {
        let mut r = Self {
            taps: 0,
            pi_n: 0.0,
            c2n: 0.0,
            omega: 0.0,
            c2w: 0.0,
            per_sample: false,
            pos: 0,
            len: 0,
            speed: OnePole::default(),
            omegas: Vec::new(),
            c2s: Vec::new(),
            frac: Vec::new(),
            idx: Vec::new(),
            args: Vec::new(),
            sines: Vec::new(),
        };
        r.set_taps(taps);
        r.set_ratio(1.0);
        r
    }

    /// Grows the work buffers to `n` samples.
    pub fn reserve(&mut self, n: usize) {
        if self.frac.len() < n {
            self.omegas.resize(n, 0.0);
            self.c2s.resize(n, 0.0);
            self.frac.resize(n, 0.0);
            self.idx.resize(n, 0);
            self.args.resize(4 * n, 0.0);
            self.sines.resize(4 * n, 0.0);
        }
    }

    /// The kernel half-width.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Sets the kernel half-width (FUN_001e4b00).
    pub fn set_taps(&mut self, taps: usize) {
        if self.taps != taps {
            self.taps = taps;
            if taps > 6 {
                self.pi_n = PI / taps as f64;
                self.c2n = std::hint::black_box(self.pi_n).cos() * 2.0;
            }
        }
    }

    /// Sets the constant cutoff for a resampling ratio (FUN_0006bfd0): `pi / |ratio|` above 1,
    /// else `pi` with `2 cos = -2`.
    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio.abs() <= 1.0 {
            self.omega = PI;
            self.c2w = -2.0;
        } else {
            self.omega = PI / ratio.abs();
            self.c2w = std::hint::black_box(self.omega).cos() * 2.0;
        }
    }

    /// Keeps `pos` in `[len, 2 len)` (the start of each block).
    pub fn wrap_pos(&mut self) {
        self.pos = self.len + self.pos % self.len;
    }

    /// Reads one output frame per offset (FUN_0006be70). `off[i]` is the read offset in frames
    /// relative to the read position; the position advances by one frame per sample.
    pub fn read(&mut self, ring: &Ring, off: &[f64], out: &mut [[f32; 2]]) {
        let n = off.len();
        self.reserve(n);
        for ((&x, frac), idx) in off.iter().zip(&mut self.frac).zip(&mut self.idx) {
            let ip = x as i64;
            *frac = x - ip as f64;
            *idx = ((ip as u64).wrapping_add(self.pos as u64) % self.len as u64) as usize;
            self.pos = self.pos.wrapping_add(1);
        }
        if !self.per_sample {
            self.kernel(ring, 0, n, out);
        } else {
            for i in 0..n {
                self.omega = self.omegas[i];
                self.c2w = self.c2s[i];
                self.kernel(ring, i, 1, &mut out[i..i + 1]);
            }
        }
    }

    /// The windowed-sinc kernel for samples `start..start + n` (FUN_001e42b0): clamp the
    /// fractions, build the four sine arguments per sample, one `vvsin` over all of them, then the
    /// NEON tap loop per sample.
    fn kernel(&mut self, ring: &Ring, start: usize, n: usize, out: &mut [[f32; 2]]) {
        let frac = &mut self.frac[start..start + n];
        for f in frac.iter_mut() {
            *f = fmaxnm(*f, MIN_FRAC);
        }
        let args = &mut self.args[..4 * n];
        for (i, &f) in frac.iter().enumerate() {
            let a = HALF_PI + f * -self.pi_n;
            args[4 * i] = a;
            args[4 * i + 1] = a - self.pi_n;
            let c = f * self.omega;
            args[4 * i + 2] = c;
            args[4 * i + 3] = c + self.omega;
        }
        let sines = &mut self.sines[..4 * n];
        vforce::sin(args, sines);
        let c2w = self.c2w as f32;
        let c2n = self.c2n as f32;
        for (i, o) in out.iter_mut().enumerate() {
            let s = &sines[4 * i..4 * i + 4];
            let (sa, sb, sc, sd) = (s[0], s[1], s[2], s[3]);
            let p7 = sa as f32;
            let p3 = sb as f32;
            let t = self.c2n * sa - sb;
            let p6 = t as f32;
            let p2 = (self.c2n * t - sa) as f32;
            let p5 = sc as f32;
            let p1 = sd as f32;
            let u = self.c2w * sc - sd;
            let p4 = u as f32;
            let p0 = (self.c2w * u - sc) as f32;
            let x = frac[i] * PI;
            let p13 = x as f32;
            let p15 = (x + PI) as f32;
            let x2 = x - PI;
            let p12 = x2 as f32;
            let p14 = (x2 - PI) as f32;
            let v = Taps {
                v7: [p0, p1, p2, p3],
                v16: [p4, p5, p6, p7],
                v4: [p12, p13, p14, p15],
                v3: [c2w, c2w, c2n, c2n],
            };
            *o = taps(&v, ring, self.idx[start + i], self.taps / 2);
        }
    }
}

/// The NEON register setup of one interpolated sample.
struct Taps {
    v7: [f32; 4],
    v16: [f32; 4],
    v4: [f32; 4],
    v3: [f32; 4],
}

/// The tap loop (NEON core at 0x407c), on the real NEON unit.
#[cfg(target_arch = "aarch64")]
fn taps(v: &Taps, ring: &Ring, idx: usize, half: usize) -> [f32; 2] {
    use std::arch::aarch64::*;
    // SAFETY: NEON is always present on aarch64; the loads read 4-element arrays.
    unsafe {
        let mut v7 = vld1q_f32(v.v7.as_ptr());
        let mut v16 = vld1q_f32(v.v16.as_ptr());
        let mut v4 = vld1q_f32(v.v4.as_ptr());
        let v3 = vld1q_f32(v.v3.as_ptr());
        let v2 = vld1q_f32([-TWO_PI_F32, TWO_PI_F32, -TWO_PI_F32, TWO_PI_F32].as_ptr());
        let v0 = vld1q_f32([W036, W036, 0.0, 0.0].as_ptr());
        let v1 = vld1q_f32([W064, W064, 1.0, 1.0].as_ptr());
        let zero = vdup_n_f32(0.0);
        let mut acc = vdupq_n_f32(0.0);
        for it in 0..half as isize {
            let e = vrecpeq_f32(v4);
            let s = vrecpsq_f32(e, v4);
            let r1 = vmulq_f32(e, s);
            let t = vmulq_f32(v4, r1);
            let t = vmulq_f32(r1, t);
            let r = vsubq_f32(vaddq_f32(r1, r1), t);
            let v19 = vmulq_f32(v3, v7);
            let v20 = vcombine_f32(vget_high_f32(v16), zero);
            let v18 = vmulq_f32(v16, r);
            let v16n = vsubq_f32(v19, v16);
            let mut w = vmulq_f32(v1, v20);
            w = vmulq_f32(v20, w);
            w = vaddq_f32(v20, w);
            w = vaddq_f32(v0, w);
            v4 = vaddq_f32(v2, v4);
            let rhi = vcombine_f32(vget_high_f32(r), vget_high_f32(r));
            let v18 = vmulq_f32(w, v18);
            let fa = ring.frame(idx, 1 + 2 * it);
            let fd = ring.frame(idx, -2 * it);
            let f = vld1q_f32([fa[0], fa[1], fd[0], fd[1]].as_ptr());
            acc = vaddq_f32(acc, vmulq_f32(f, vzip1q_f32(v18, v18)));
            let v6 = vcombine_f32(vget_high_f32(v7), zero);
            let mut w2 = vmulq_f32(v1, v6);
            w2 = vmulq_f32(v6, w2);
            let v19 = vmulq_f32(v3, v16n);
            w2 = vaddq_f32(v6, w2);
            let v17 = vmulq_f32(v7, rhi);
            let v7n = vsubq_f32(v19, v7);
            w2 = vaddq_f32(v0, w2);
            let v17 = vmulq_f32(w2, v17);
            let fb = ring.frame(idx, 2 + 2 * it);
            let fc = ring.frame(idx, -1 - 2 * it);
            let f = vld1q_f32([fb[0], fb[1], fc[0], fc[1]].as_ptr());
            acc = vaddq_f32(acc, vmulq_f32(f, vzip1q_f32(v17, v17)));
            v7 = v7n;
            v16 = v16n;
        }
        let mut a = [0f32; 4];
        vst1q_f32(a.as_mut_ptr(), acc);
        [(a[0] + a[2]) * 0.5, (a[1] + a[3]) * 0.5]
    }
}

/// The tap loop with an emulated `FRECPE`/`FRECPS`, for targets without NEON.
#[cfg(not(target_arch = "aarch64"))]
fn taps(v: &Taps, ring: &Ring, idx: usize, half: usize) -> [f32; 2] {
    taps_emulated(v, ring, idx, half)
}

/// The tap loop in scalar f32 with the ARM reciprocal estimate emulated.
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
fn taps_emulated(v: &Taps, ring: &Ring, idx: usize, half: usize) -> [f32; 2] {
    let mul = |a: [f32; 4], b: [f32; 4]| [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]];
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    let (mut v7, mut v16, mut v4, v3) = (v.v7, v.v16, v.v4, v.v3);
    let v2 = [-TWO_PI_F32, TWO_PI_F32, -TWO_PI_F32, TWO_PI_F32];
    let v0 = [W036, W036, 0.0, 0.0];
    let v1 = [W064, W064, 1.0, 1.0];
    let mut acc = [0f32; 4];
    for it in 0..half as isize {
        let r = v4.map(recip);
        let v19 = mul(v3, v7);
        let v20 = [v16[2], v16[3], 0.0, 0.0];
        let v18 = mul(v16, r);
        let v16n = sub(v19, v16);
        let mut w = mul(v1, v20);
        w = mul(v20, w);
        w = add(v20, w);
        w = add(v0, w);
        v4 = add(v2, v4);
        let rhi = [r[2], r[3], r[2], r[3]];
        let v18 = mul(w, v18);
        let fa = ring.frame(idx, 1 + 2 * it);
        let fd = ring.frame(idx, -2 * it);
        acc = add(
            acc,
            mul(
                [fa[0], fa[1], fd[0], fd[1]],
                [v18[0], v18[0], v18[1], v18[1]],
            ),
        );
        let v6 = [v7[2], v7[3], 0.0, 0.0];
        let mut w2 = mul(v1, v6);
        w2 = mul(v6, w2);
        let v19 = mul(v3, v16n);
        w2 = add(v6, w2);
        let v17 = mul(v7, rhi);
        let v7n = sub(v19, v7);
        w2 = add(v0, w2);
        let v17 = mul(w2, v17);
        let fb = ring.frame(idx, 2 + 2 * it);
        let fc = ring.frame(idx, -1 - 2 * it);
        acc = add(
            acc,
            mul(
                [fb[0], fb[1], fc[0], fc[1]],
                [v17[0], v17[0], v17[1], v17[1]],
            ),
        );
        v7 = v7n;
        v16 = v16n;
    }
    [(acc[0] + acc[2]) * 0.5, (acc[1] + acc[3]) * 0.5]
}

/// `FRECPE` + `FRECPS` + the two Newton steps of the kernel, emulated.
fn recip(x: f32) -> f32 {
    let e = recip_estimate(x);
    let s = (-e).mul_add(x, 2.0);
    let r1 = e * s;
    let t = x * r1;
    let t = r1 * t;
    (r1 + r1) - t
}

/// The ARM `FPRecipEstimate` for single precision with the default FPCR (no flush to zero).
pub fn recip_estimate(x: f32) -> f32 {
    let b = x.to_bits();
    let sign = b >> 31;
    let mut exp = ((b >> 23) & 0xff) as i32;
    let mut frac = b & 0x7f_ffff;
    if exp == 0xff && frac != 0 {
        return x;
    }
    if exp == 0xff {
        return f32::from_bits(sign << 31);
    }
    if exp == 0 && frac == 0 {
        return f32::from_bits((sign << 31) | 0x7f80_0000);
    }
    if exp == 0 {
        if frac & 0x40_0000 != 0 {
            frac = (frac << 1) & 0x7f_ffff;
            exp = -1;
        } else {
            frac = (frac << 2) & 0x7f_ffff;
            exp = -2;
        }
    }
    let scaled = (1u32 << 8) | (frac >> 15);
    let mut result_exp = 253 - exp;
    let a = scaled * 2 + 1;
    let bq = (1u32 << 19) / a;
    let r = bq.div_ceil(2);
    let mut fraction = (r & 0xff) << 15;
    if result_exp == 0 {
        fraction = (1 << 22) | (fraction >> 1);
    } else if result_exp == -1 {
        fraction = (1 << 21) | (fraction >> 2);
        result_exp = 0;
    }
    if result_exp >= 0xff {
        return f32::from_bits((sign << 31) | 0x7f80_0000);
    }
    f32::from_bits((sign << 31) | ((result_exp as u32) << 23) | fraction)
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;

    /// The emulated tap loop must match the NEON one bit for bit.
    #[test]
    fn emulated_taps_match_neon() {
        let mut ring = Ring::default();
        ring.resize(64, 4096);
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let frames: Vec<[f32; 2]> = (0..4096).map(|_| [rnd(), rnd()]).collect();
        ring.record(&frames);
        for k in 0..2000 {
            let f = (rnd() as f64 + 1.0) * 0.5;
            let x = f * PI;
            let s = |a: f64| a.sin() as f32;
            let v = Taps {
                v7: [s(f - 2.0), s(f + 1.0), s(f + 0.3), s(f - 0.4)],
                v16: [s(f - 1.0), s(f), s(f + 0.2), s(f + 0.1)],
                v4: [
                    (x - PI) as f32,
                    x as f32,
                    (x - 2.0 * PI) as f32,
                    (x + PI) as f32,
                ],
                v3: [rnd() * 2.0, rnd() * 2.0, 1.96, 1.96],
            };
            let idx = 100 + k % 3000;
            let a = taps(&v, &ring, idx, 8);
            let b = taps_emulated(&v, &ring, idx, 8);
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "sample {k}");
        }
    }
}
