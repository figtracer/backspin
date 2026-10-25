//! Per-band shaper DSP (shapers_a.md, shapers_b.md). Every band processor takes stereo audio and
//! the band's wave `w[i]` in [0, 1] (0.5 is the wave's centre line, 1 the top).

pub mod liquid;
pub mod pan;
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

/// `DryWet` mix law 0 (0x2dcd10): `out = wet*m + dry*(1 - m)` per sample.
pub fn dry_wet(wet: &mut [[f32; 2]], dry: &[[f32; 2]], m: impl Fn(usize) -> f32) {
    for (i, (o, d)) in wet.iter_mut().zip(dry).enumerate() {
        let m = m(i);
        for c in 0..2 {
            o[c] = o[c] * m + d[c] * (1.0 - m);
        }
    }
}
