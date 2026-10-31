//! Reverb: the convolution reverb (shapers_b.md section 5; `ReverbShaperModulation::process`
//! 0x540574, `ConvolutionReverb::process` 0x2d277c, `FastConvolution` 0x2e8324, IR build
//! 0x2d5348). Per band: send wave, integer pre-delay, zero-latency partitioned convolution,
//! width, volume wave, then the send-style mix ([`send_mix`]). The IR build mirrors
//! `.amp/in/sb/data/sb_reverb_exact.py` bit for bit; the convolution uses `realfft` instead of
//! pffft, so it agrees with the plugin to FFT rounding (about 1e-8 here).
//!
//! The IR is built when the band is built (off the audio thread). Processing is allocation-free.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::Wave;
use crate::smoother::Smoother;

mod source;

pub use source::{
    SourceIr, decode_wav, default_ir_paths, hall_ir, hall_ir_from_plugin, init_hall_ir,
    load_hall_ir, procedural_hall,
};

/// `DAT_01328d28` (sqrt 2 as f32).
const E_SQRT2: u32 = 0x3fb5_04f3;
/// `DAT_01328d2c` = 2^-20, the silence threshold of the IR build.
const ZERO_THRESH: u32 = 0x3580_0000;
const PI_F: f32 = std::f32::consts::PI;
const THIRD_F: u32 = 0x3eaa_aaab;
/// The convolver marks a block live when any input sample is above this.
const LIVE_THRESH: f32 = 1e-6;
/// The largest chunk the band processes at once (the graph's sub-block).
const MAX_CHUNK: usize = 512;
/// The slowest tempo the pre-delay ring is sized for: bar/4 at 20 bpm is 3 s.
const MIN_BPM: f64 = 20.0;

fn fl(bits: u32) -> f32 {
    f32::from_bits(bits)
}

/// The reference model's `fma`: `f32(a * b + c)` evaluated in double (the IR build constants such
/// as 1.8 are doubles there).
fn fma_d(a: f32, b: f64, c: f64) -> f32 {
    (a as f64 * b + c) as f32
}

fn clamp01(x: f32) -> f32 {
    if x > 1.0 {
        1.0
    } else if x >= 0.0 {
        x
    } else {
        0.0
    }
}

/// The IR-dependent band settings (the loader inputs) and the pre-delay settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    pub stretch: f32,
    pub decay: f32,
    pub gated: bool,
    pub delay_time: f32,
    pub delay_sync: i32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            stretch: 0.5,
            decay: 1.0,
            gated: false,
            delay_time: 0.0,
            delay_sync: 0,
        }
    }
}

/// The built IR of one band: the processed channels and the partition size `N`.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltIr {
    pub chans: [Vec<f32>; 2],
    pub n: usize,
}

impl BuiltIr {
    /// The IR build of the loader thread (`FUN_002d5348`): stereo, resample to the stretched
    /// rate, level, tone correction, decay or gate. `N` comes from the original length.
    pub fn new(src: &SourceIr, sr: f64, p: &ReverbParams) -> Self {
        let orig_len = src.chans.first().map_or(0, Vec::len);
        let mut chans = match src.chans.as_slice() {
            [m] => [m.clone(), m.clone()],
            [l, r, ..] => [l.clone(), r.clone()],
            [] => [Vec::new(), Vec::new()],
        };
        let ratio = stretch_ratio(p.stretch);
        let target = ratio as f64 * sr;
        let mut g = (44100.0 / src.sr) as f32;
        if src.sr != target {
            let step = src.sr / target;
            for c in &mut chans {
                *c = resample_lagrange(c, step);
            }
            g *= (src.sr / target) as f32;
        }
        if ((1.0 - g).abs() as f64) > 0.0001 && g != 1.0 {
            for v in chans.iter_mut().flatten() {
                *v *= g;
            }
        }
        tone_correction(&mut chans, sr, p.stretch, src.sr);
        decay_or_gate(&mut chans, sr, p.decay, p.gated);
        Self {
            chans,
            n: partition_size(orig_len),
        }
    }
}

/// One channel of `FastConvolution` (0x2e8324): zero-latency uniformly partitioned overlap-add.
/// The current, partly filled block is transformed on every chunk and multiplied with
/// partition 0; partitions 1.. are summed once per block from the earlier blocks' spectra.
struct FastConvolution {
    n: usize,
    segs: usize,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// IR partition spectra, `segs` rows of `n + 1` bins.
    ir: Vec<Complex32>,
    /// The frequency-domain delay line of the input blocks.
    spec: Vec<Complex32>,
    valid: Vec<bool>,
    cur: usize,
    fill: usize,
    inbuf: Vec<f32>,
    in_live: bool,
    tail: Vec<Complex32>,
    tail_live: bool,
    /// The spectrum of the current output block (consumed by the inverse transform).
    acc: Vec<Complex32>,
    /// The current output block in time (`+0x98`).
    out: Vec<f32>,
    out_live: bool,
    /// The previous block's second half.
    ovl: Vec<f32>,
    ovl_live: bool,
    time: Vec<f32>,
    scratch: Vec<Complex32>,
}

impl FastConvolution {
    /// Splits `chan` into `n`-sample partitions (`FUN_002d6afc`): trailing `|x| < 2^-20` is
    /// trimmed, each partition is zero-padded to `2n` and transformed once.
    fn new(
        chan: &[f32],
        n: usize,
        fwd: Arc<dyn RealToComplex<f32>>,
        inv: Arc<dyn ComplexToReal<f32>>,
    ) -> Self {
        let mut len = chan.len();
        while len > 1 && chan[len - 1].abs() < fl(ZERO_THRESH) {
            len -= 1;
        }
        let segs = len.div_ceil(n);
        let bins = n + 1;
        let scratch = vec![Complex32::default(); fwd.get_scratch_len().max(inv.get_scratch_len())];
        let mut c = Self {
            n,
            segs,
            ir: vec![Complex32::default(); segs * bins],
            spec: vec![Complex32::default(); segs * bins],
            valid: vec![false; segs],
            cur: 0,
            fill: 0,
            inbuf: vec![0.0; 2 * n],
            in_live: false,
            tail: vec![Complex32::default(); bins],
            tail_live: false,
            acc: vec![Complex32::default(); bins],
            out: vec![0.0; 2 * n],
            out_live: false,
            ovl: vec![0.0; n],
            ovl_live: false,
            time: vec![0.0; 2 * n],
            scratch,
            fwd,
            inv,
        };
        for (s, dst) in c.ir.chunks_exact_mut(bins).enumerate() {
            let part = &chan[s * n..len.min(s * n + n)];
            c.time.fill(0.0);
            c.time[..part.len()].copy_from_slice(part);
            let _ = c.fwd.process_with_scratch(&mut c.time, dst, &mut c.scratch);
        }
        c
    }

    fn process(&mut self, buf: &mut [f32]) {
        if self.segs == 0 {
            buf.fill(0.0);
            return;
        }
        let (n, bins) = (self.n, self.n + 1);
        let mut i = 0;
        while i < buf.len() {
            let chunk = (buf.len() - i).min(n - self.fill);
            let fill0 = self.fill;
            let src = &buf[i..i + chunk];
            self.inbuf[fill0..fill0 + chunk].copy_from_slice(src);
            self.in_live |= src.iter().any(|v| v.abs() > LIVE_THRESH);
            let cur = self.cur;
            if self.in_live {
                self.time.copy_from_slice(&self.inbuf);
                let dst = &mut self.spec[cur * bins..(cur + 1) * bins];
                let _ = self
                    .fwd
                    .process_with_scratch(&mut self.time, dst, &mut self.scratch);
                self.valid[cur] = true;
            }
            let tail_path = if fill0 == 0 {
                self.tail.fill(Complex32::default());
                self.tail_live = false;
                for k in 1..self.segs {
                    let idx = (cur + self.segs - k) % self.segs;
                    if self.valid[idx] {
                        zconv_acc(
                            &self.ir[k * bins..(k + 1) * bins],
                            &self.spec[idx * bins..(idx + 1) * bins],
                            &mut self.tail,
                        );
                        self.tail_live = true;
                    }
                }
                self.tail_live
            } else {
                self.tail_live
            };
            let bcur = self.valid[cur];
            let wrote = if tail_path || bcur {
                if tail_path {
                    self.acc.copy_from_slice(&self.tail);
                } else {
                    self.acc.fill(Complex32::default());
                }
                if bcur {
                    zconv_acc(
                        &self.ir[..bins],
                        &self.spec[cur * bins..(cur + 1) * bins],
                        &mut self.acc,
                    );
                }
                self.acc[0].im = 0.0;
                self.acc[n].im = 0.0;
                let _ =
                    self.inv
                        .process_with_scratch(&mut self.acc, &mut self.out, &mut self.scratch);
                let scale = 1.0 / (2 * n) as f32;
                for v in &mut self.out {
                    *v *= scale;
                }
                self.out_live = true;
                true
            } else {
                if self.out_live {
                    self.out.fill(0.0);
                    self.out_live = false;
                }
                self.ovl_live
            };
            if wrote && (self.out_live || self.ovl_live) {
                let o = &self.out[fill0..fill0 + chunk];
                let p = &self.ovl[fill0..fill0 + chunk];
                for ((d, o), p) in buf[i..i + chunk].iter_mut().zip(o).zip(p) {
                    *d = o + p;
                }
            }
            self.fill = fill0 + chunk;
            if self.fill == n {
                if self.out_live {
                    self.ovl.copy_from_slice(&self.out[n..]);
                    self.ovl_live = true;
                } else if self.ovl_live {
                    self.ovl.fill(0.0);
                    self.ovl_live = false;
                }
                if self.in_live {
                    self.inbuf.fill(0.0);
                    self.in_live = false;
                }
                self.cur = (cur + 1) % self.segs;
                self.fill = 0;
                self.spec[self.cur * bins..(self.cur + 1) * bins].fill(Complex32::default());
                self.valid[self.cur] = false;
            }
            i += chunk;
        }
    }
}

/// The `5Delay` pre-delay with a constant time port: whole samples, no interpolation; a read
/// before the oldest written sample gives 0.
struct PreDelay {
    ring: Vec<[f32; 2]>,
    mask: usize,
    wpos: usize,
    filled: usize,
}

impl PreDelay {
    fn new(max_seconds: f64, sr: f64) -> Self {
        let size = ((sr * max_seconds) as usize + MAX_CHUNK + 1).next_power_of_two();
        Self {
            ring: vec![[0.0; 2]; size],
            mask: size - 1,
            wpos: 0,
            filled: 0,
        }
    }

    /// Writes the block, then reads it back `k` samples late.
    fn process(&mut self, buf: &mut [[f32; 2]], k: usize) {
        let n = buf.len();
        let size = self.ring.len();
        for (j, f) in buf.iter().enumerate() {
            self.ring[(self.wpos + j) & self.mask] = *f;
        }
        self.wpos = (self.wpos + n) & self.mask;
        self.filled = (self.filled + n).min(size);
        let k = k.min(size - MAX_CHUNK);
        for (i, f) in buf.iter_mut().enumerate() {
            let remaining = n - i;
            *f = if k + remaining > self.filled {
                [0.0; 2]
            } else {
                self.ring[(self.wpos + size - remaining - k) & self.mask]
            };
        }
    }
}

/// One Reverb band.
pub struct ReverbBand {
    sr: f64,
    delay_time: f32,
    delay_sync: i32,
    pre: Option<PreDelay>,
    /// The two channel convolvers; `None` for the "Off" (or a missing) IR: a silent wet path.
    conv: Option<Box<[FastConvolution; 2]>>,
    ch: [Vec<f32>; 2],
}

impl ReverbBand {
    /// Builds the band and its IR from `ir` (`None` = silent). Allocates; call it off the audio
    /// thread.
    pub fn new(sr: f32, ir: Option<&SourceIr>, p: ReverbParams) -> Self {
        let sr = sr as f64;
        let conv = ir
            .filter(|s| s.chans.first().is_some_and(|c| !c.is_empty()))
            .map(|src| {
                let built = BuiltIr::new(src, sr, &p);
                let mut planner = RealFftPlanner::<f32>::new();
                let fwd = planner.plan_fft_forward(2 * built.n);
                let inv = planner.plan_fft_inverse(2 * built.n);
                let [l, r] = &built.chans;
                Box::new([
                    FastConvolution::new(l, built.n, fwd.clone(), inv.clone()),
                    FastConvolution::new(r, built.n, fwd, inv),
                ])
            });
        let max_seconds = if p.delay_sync == 0 && p.delay_time <= 0.5 {
            predelay_seconds(p.delay_time, 0, 120.0) as f64
        } else {
            60.0 / MIN_BPM
        };
        Self {
            sr,
            delay_time: p.delay_time,
            delay_sync: p.delay_sync,
            pre: (conv.is_some() && max_seconds > 0.0).then(|| PreDelay::new(max_seconds, sr)),
            conv,
            ch: [vec![0.0; MAX_CHUNK], vec![0.0; MAX_CHUNK]],
        }
    }

    /// Processes one band block in place up to the wet signal (before the band mix): send wave,
    /// pre-delay, convolution, width (the smoothed `width` port), volume wave.
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        send: Wave,
        volume: Wave,
        width: &Smoother,
        bpm: f64,
    ) {
        let Some(conv) = self.conv.as_mut() else {
            buf.fill([0.0; 2]);
            return;
        };
        if !send.is_constant(1.0) {
            for (i, f) in buf.iter_mut().enumerate() {
                let s = send.at(i);
                *f = [f[0] * s, f[1] * s];
            }
        }
        if let Some(pre) = self.pre.as_mut() {
            let t = predelay_seconds(self.delay_time, self.delay_sync, bpm);
            let d = self.sr * t as f64;
            pre.process(buf, if d > 0.0 { d as usize } else { 0 });
        }
        for block in buf.chunks_mut(MAX_CHUNK) {
            let n = block.len();
            for c in 0..2 {
                let ch = &mut self.ch[c][..n];
                for (d, f) in ch.iter_mut().zip(block.iter()) {
                    *d = f[c];
                }
                conv[c].process(ch);
                for (f, d) in block.iter_mut().zip(ch.iter()) {
                    f[c] = *d;
                }
            }
        }
        if width.ramping || width.value != 0.5 {
            for (i, f) in buf.iter_mut().enumerate() {
                let d = (f[0] - f[1]) * (width.at(i) - 0.5);
                *f = [f[0] + d, f[1] - d];
            }
        }
        if !volume.is_constant(1.0) {
            for (i, f) in buf.iter_mut().enumerate() {
                let v = volume.at(i);
                *f = [f[0] * v, f[1] * v];
            }
        }
    }
}

/// `DryWet` mix law 1 (0x2dd16c), the Reverb's send-style band mix:
/// `out = fma(dry, gd, wet gw)`, `gd = min(1, 2 (1 - m))`, `gw = min(1, 2 m)`.
pub fn send_mix(wet: &mut [[f32; 2]], dry: &[[f32; 2]], m: f32) {
    let gd = if m <= 0.5 { 1.0 } else { m.mul_add(-2.0, 2.0) };
    let gw = if m >= 0.5 { 1.0 } else { m + m };
    for (o, d) in wet.iter_mut().zip(dry) {
        *o = [d[0].mul_add(gd, o[0] * gw), d[1].mul_add(gd, o[1] * gw)];
    }
}

/// The pre-delay in seconds (`ConvolutionReverb+0x6c`). `delaySync` 0: up to 0.5 the knob gives
/// `0.6 t^2` (0..150 ms), above it bar/16, bar/8, 3 bar/16, bar/4; `delaySync` 1..4 select
/// those directly.
pub fn predelay_seconds(delay_time: f32, delay_sync: i32, bpm: f64) -> f32 {
    let bar = (60.0 / bpm) as f32 * 4.0;
    let frac = match delay_sync {
        0 => {
            let t = delay_time;
            if t <= 0.5 {
                let tt = t + t;
                return tt * (tt * 0.15);
            }
            if t <= 0.625 {
                0.0625
            } else if t <= 0.75 {
                0.125
            } else if t <= 0.875 {
                0.1875
            } else if t <= 1.0 {
                0.25
            } else {
                0.0
            }
        }
        1 => 0.0625,
        2 => 0.125,
        3 => 0.1875,
        4 => 0.25,
        _ => 0.0,
    };
    bar * frac
}

/// The playback ratio `exp2f(2 stretch - 1)`, exactly 1 at 0.5.
fn stretch_ratio(stretch: f32) -> f32 {
    if stretch == 0.5 {
        1.0
    } else {
        stretch.mul_add(2.0, -1.0).exp2()
    }
}

/// The resampler of `FUN_002d5348`: 5-point Lagrange on a ring of the last five input samples,
/// read position advancing by `step` from 1.0, so the output is delayed by two input samples.
fn resample_lagrange(x: &[f32], step: f64) -> Vec<f32> {
    let n_out = (x.len() as f64 / step).ceil() as usize;
    let third = fl(THIRD_F);
    let mut ring = [0.0f32; 5];
    let (mut w, mut rd, mut acc) = (0usize, 0usize, 1.0f64);
    let mut out = Vec::with_capacity(n_out);
    for _ in 0..n_out {
        while acc >= 1.0 {
            ring[w] = x.get(rd).copied().unwrap_or(0.0);
            rd += 1;
            w = if w == 4 { 0 } else { w + 1 };
            acc -= 1.0;
        }
        let f = acc as f32;
        let r = |j: usize| ring[(w + j) % 5];
        let (r0, r1, r2, r3, r4) = (r(0), r(1), r(2), r(3), r(4));
        let am1 = -1.0 - f;
        let a1 = 1.0 - f;
        let a2 = 2.0 - f;
        let am2 = -2.0 - f;
        let p2 = -am2;
        let nf = -f;
        let hf = nf * 0.5;
        let t0 = (a2 * 0.25) * ((third * a1) * (hf * (am1 * r0)));
        let t1 = (third * a2) * ((0.5 * a1) * ((r1 * p2) * nf));
        let t2 = (a1 * (((am2 * -0.5) * r2) * am1)) * (a2 * 0.5);
        let t3 = a2 * (f * ((0.5 * am1) * ((third * am2) * r3)));
        let t4 = a1 * (hf * ((third * am1) * ((0.25 * am2) * r4)));
        out.push(((((t0 + 0.0) + t1) - t2) + t3) + t4);
        acc += step;
    }
    out
}

/// `FUN_002d5fe0`: the spectral correction for the playback ratio, a high shelf above 15.4 kHz
/// when the IR plays faster, a 160 Hz low boost when slower. The filter state runs on from L
/// into R.
fn tone_correction(chans: &mut [Vec<f32>; 2], sr: f64, stretch: f32, ir_sr: f64) {
    if stretch == 0.5 {
        return;
    }
    let ratio = stretch_ratio(stretch);
    let rm1 = ratio - 1.0;
    if (rm1.abs() as f64) < 0.0001 {
        return;
    }
    if ratio >= 1.0 {
        if ir_sr > 48000.0 {
            return;
        }
        let e = ((rm1.max(0.0) as f64 * 15.72) as f32 as f64 * 0.166_666_67) as f32;
        let b = e.exp2();
        let wc = (15435.0 / sr) as f32 as f64;
        let wc = if wc > 0.499 { 0.499 } else { wc };
        let w = ((wc * PI_F as f64) as f32).tan();
        let g = b.sqrt() * w;
        let bb = b * b;
        let bs = b * fl(E_SQRT2);
        let k = g + fl(E_SQRT2);
        let d = 1.0 / fma_d(g, k as f64, 1.0);
        let gd = g * d;
        let ggd = g * gd;
        let kd = k * d;
        let (nkd, ngkd) = (-kd, -(g * kd));
        let (mut ic1, mut ic2) = (0.0f32, 0.0f32);
        for v in chans.iter_mut().flatten() {
            let x = *v - ic2;
            let hp = fma_d(d, x as f64, (ic1 * nkd) as f64);
            let v1 = fma_d(gd, x as f64, (ngkd * ic1) as f64);
            let v2 = fma_d(ggd, x as f64, (gd * ic1) as f64);
            let bp = ic1 + v1;
            let lp = ic2 + v2;
            ic1 = v1 + bp;
            ic2 = v2 + lp;
            *v = lp + fma_d(bb, hp as f64, (bs * bp) as f64);
        }
    } else {
        let x = (1.0 - ratio) + (1.0 - ratio);
        let boost = ((x.max(0.0) * 3.0) as f64 * 0.166_666_67) as f32;
        let boost = boost.exp2();
        let wc = (160.0 / sr) as f32 as f64;
        let wc = if wc > 0.4999 { 0.4999 } else { wc };
        let t = if wc >= 0.0 {
            ((wc * PI_F as f64) as f32).tan()
        } else {
            0.0
        };
        let t = if t < 0.0 { 0.0 } else { t };
        let big_g = t / (t + 1.0);
        let mut s = 0.0f32;
        for v in chans.iter_mut().flatten() {
            let x = *v;
            let d = big_g * (x - s);
            let lp = s + d;
            s = d + lp;
            *v = fma_d(boost, lp as f64, (x - lp) as f64);
        }
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = clamp01(t);
    (t * t) * fma_d(t, -2.0, 3.0)
}

/// fcvtas: round to nearest, ties away from zero.
fn round_away(x: f32) -> i64 {
    x.round() as i64
}

/// `FUN_002d6360`: the decay envelope (`decay < 1`) or the gate window (`gated`, which caps the
/// buffer at 2 s and zero-pads it).
fn decay_or_gate(chans: &mut [Vec<f32>; 2], sr: f64, decay: f32, gated: bool) {
    let total = chans[0].len();
    let thr = fl(ZERO_THRESH);
    let mut trimmed = usize::from(total > 0);
    for i in (2..=total).rev() {
        if chans[0][i - 1].abs().max(chans[1][i - 1].abs()) >= thr {
            trimmed = i;
            break;
        }
    }
    let sr = sr as f32;
    if gated {
        let fade_len = round_away((sr as f64 * 0.2) as f32);
        let g_end = (round_away(sr * fma_d(decay, 1.8, 0.2)) - fade_len).max(0) as usize;
        let cap = round_away(sr + sr) as usize;
        let n = total.min(cap);
        let mut pos = g_end;
        if g_end < n {
            let step = (1.0 / fade_len as f64) as f32;
            let mut fv = 1.0f32;
            loop {
                let w = smoothstep(fv);
                for c in chans.iter_mut() {
                    c[pos] *= w;
                }
                pos += 1;
                if pos >= n {
                    break;
                }
                fv -= step;
                if fv <= 0.0 {
                    break;
                }
            }
        }
        for c in chans.iter_mut() {
            if pos < n {
                c[pos..n].fill(0.0);
            }
            c.resize(cap, 0.0);
        }
        return;
    }
    if decay >= 1.0 {
        return;
    }
    let e = clamp01((sr as f64 * 0.1) as f32 * (1.0 / trimmed as f64) as f32);
    let e = if decay > e { decay } else { e };
    let e2 = e * e;
    let e2c = if (e2 as f64) >= 1e-12 { e2 } else { 1e-12 };
    let hold_f = ((e2 as f64 * 0.2) as f32 as f64 * total as f64) as f32;
    let h12 = (sr as f64 * 0.012) as f32;
    let hold = (if hold_f < h12 { h12 } else { hold_f }) as usize;
    let span = if total < hold + 2 { 1 } else { total - hold };
    let inv = (1.0 / e2c) - 1.0;
    let p = fma_d(e2, 2.0, 1.0);
    let pi_p = PI_F / p;
    let inv_span = (1.0 / span as f64) as f32;
    for i in hold..trimmed {
        let a = inv_span * (pi_p as f64 * (i - hold) as f64) as f32;
        let a = if a > 1.5 { 1.5 } else { a };
        let g = (a.tan().powf(p) * -inv).exp2();
        if g < thr {
            for c in chans.iter_mut() {
                c[i..trimmed].fill(0.0);
            }
            break;
        }
        for c in chans.iter_mut() {
            c[i] *= g;
        }
    }
}

/// `N = clamp(pow2ceil(len >> 7), 512, 8192)` from the original (unresampled) IR length.
fn partition_size(orig_len: usize) -> usize {
    (orig_len >> 7).max(1).next_power_of_two().clamp(512, 8192)
}

/// `ab += a b` over the bins (`pffft_zconvolve_accumulate` with scale 1).
fn zconv_acc(a: &[Complex32], b: &[Complex32], ab: &mut [Complex32]) {
    for ((a, b), o) in a.iter().zip(b).zip(ab) {
        *o += a * b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direct convolution of an impulse train with a short IR, against the convolver at
    /// block sizes that do and do not divide `N`.
    #[test]
    fn convolution_matches_direct_sum() {
        let n = 512;
        let ir: Vec<f32> = (0..1500)
            .map(|i| ((i * 7919) % 101) as f32 / 1000.0 - 0.05)
            .collect();
        let x: Vec<f32> = (0..5000)
            .map(|i| ((i * 104_729) % 17) as f32 / 17.0 - 0.5)
            .collect();
        let want: Vec<f32> = (0..x.len())
            .map(|i| {
                (0..ir.len().min(i + 1))
                    .map(|k| x[i - k] as f64 * ir[k] as f64)
                    .sum::<f64>() as f32
            })
            .collect();
        for block in [512, 100, 37] {
            let mut planner = RealFftPlanner::<f32>::new();
            let mut c = FastConvolution::new(
                &ir,
                n,
                planner.plan_fft_forward(2 * n),
                planner.plan_fft_inverse(2 * n),
            );
            let mut y = x.clone();
            for b in y.chunks_mut(block) {
                c.process(b);
            }
            let err = y
                .iter()
                .zip(&want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(err < 1e-5, "block {block}: max error {err}");
        }
    }

    #[test]
    fn partition_and_predelay() {
        assert_eq!(partition_size(145_688), 2048);
        assert_eq!(partition_size(1000), 512);
        assert_eq!(partition_size(10_000_000), 8192);
        assert_eq!(predelay_seconds(0.5, 0, 120.0), 0.15);
        assert_eq!(predelay_seconds(0.7, 0, 120.0), 0.25);
        assert_eq!(predelay_seconds(0.0, 1, 120.0), 0.125);
        let mut d = PreDelay::new(0.15, 48000.0);
        let mut buf: Vec<[f32; 2]> = (0..512).map(|i| [i as f32, -(i as f32)]).collect();
        d.process(&mut buf, 100);
        assert_eq!(buf[99], [0.0, 0.0]);
        assert_eq!(buf[100], [0.0, -0.0]);
        assert_eq!(buf[511], [411.0, -411.0]);
    }
}
