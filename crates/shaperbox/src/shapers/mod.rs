//! Per-band shaper DSP (shapers_a.md, shapers_b.md). Every band processor takes stereo audio and
//! the band's wave `w[i]` in [0, 1] (0.5 is the wave's centre line, 1 the top).

pub mod crush;
pub mod drive;
pub mod filter;
pub mod liquid;
pub mod noise;
pub mod pan;
pub mod reverb;
pub mod time;
pub mod volume;
pub mod width;

/// The wave of a band for one block: per sample, or one value for the whole block.
#[derive(Clone, Copy, Debug)]
pub enum Wave<'a> {
    Constant(f32),
    PerSample(&'a [f32]),
}

impl Wave<'_> {
    pub fn at(&self, i: usize) -> f32 {
        match self {
            Wave::Constant(w) => *w,
            Wave::PerSample(b) => b[i],
        }
    }

    pub fn is_constant(&self, v: f32) -> bool {
        matches!(self, Wave::Constant(w) if *w == v)
    }
}

/// `DryWet` mix law 0 (0x2dcd10): `out = fma(dry, 1 - m, wet * m)` per sample; `m == 1` is
/// the wet signal.
pub fn dry_wet(wet: &mut [[f32; 2]], dry: &[[f32; 2]], m: f32) {
    if m == 1.0 {
        return;
    }
    for (o, d) in wet.iter_mut().zip(dry) {
        for c in 0..2 {
            o[c] = d[c].mul_add(1.0 - m, o[c] * m);
        }
    }
}

/// The TPT one-pole coefficient `k = g / (1 + g)`, `g = tan(pi min(fc / sr, 0.4999))`, in f32
/// as the plugin's inlined filters compute it.
pub fn tpt_k(fc: f32, sr: f32) -> f32 {
    let r = fc / sr;
    let w = if r >= 0.0 {
        r.min(0.499_9) * std::f32::consts::PI
    } else {
        0.0
    };
    let g = w.tan().max(0.0);
    g / (g + 1.0)
}

/// A TPT one-pole low-pass: `v = k (x - s); y = s + v; s = y + v`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    s: f32,
}

impl OnePole {
    /// A filter whose state sits at `s`.
    pub fn at(s: f32) -> Self {
        Self { s }
    }

    pub fn lp(&mut self, x: f32, k: f32) -> f32 {
        let v = k * (x - self.s);
        let y = self.s + v;
        self.s = v + y;
        y
    }
}
