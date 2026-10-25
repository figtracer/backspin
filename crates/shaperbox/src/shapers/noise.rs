//! Noise (NoiseShaperModulation::process 0x528504; shapers_a.md section 7). Generated white noise
//! only: the sample-based noises are content ShaperBox downloads on first use, so a band that
//! selects one adds nothing.

use super::Wave;
use crate::bandsplit::F0;

/// `GaussianNoiseGenerator`: the Marsaglia polar method on the MS rand LCG, scaled by 0.0496.
#[derive(Clone)]
struct Gaussian {
    x: i32,
    spare: Option<f32>,
}

impl Gaussian {
    fn uniform(&mut self) -> f64 {
        self.x = self.x.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        self.x as f64 / 2_147_483_648.0
    }

    fn pair(&mut self) -> (f32, f32) {
        loop {
            let (u, v) = (self.uniform(), self.uniform());
            let s = u * u + v * v;
            if s != 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                return ((0.0496 * u * m) as f32, (0.0496 * v * m) as f32);
            }
        }
    }

    fn mono(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let (a, b) = self.pair();
        self.spare = Some(b);
        a
    }
}

/// Two cascaded TPT one-poles with one coefficient.
#[derive(Clone, Copy, Default)]
struct Cascade2 {
    s: [f32; 2],
}

impl Cascade2 {
    /// Two cascaded one-pole high-passes `h = x - lp(x)`.
    fn hp(&mut self, mut x: f32, k: f32) -> f32 {
        for s in &mut self.s {
            let v = k * (x - *s);
            let y = *s + v;
            *s = y + v;
            x -= y;
        }
        x
    }

    fn lp(&mut self, mut x: f32, k: f32) -> f32 {
        for s in &mut self.s {
            let v = k * (x - *s);
            let y = *s + v;
            *s = y + v;
            x = y;
        }
        x
    }
}

fn coef(x: f32, sr: f32) -> f32 {
    let fc = F0 * (1024f64.ln() * x.clamp(0.0, 1.0) as f64).exp();
    let g = ((fc / sr as f64).min(0.4999) * std::f64::consts::PI)
        .tan()
        .max(0.0) as f32;
    g / (g + 1.0)
}

/// One Noise band.
pub struct NoiseBand {
    sr: f32,
    generator: Gaussian,
    /// The 44.1 kHz source and the resampling position (4-point cubic Hermite above 44.1 kHz).
    hist: [[f32; 2]; 4],
    frac: f64,
    lp: [Cascade2; 2],
    hp: [Cascade2; 2],
}

impl NoiseBand {
    pub fn new(sr: f32, seed: i32) -> Self {
        Self {
            sr,
            generator: Gaussian {
                x: 0x368b22 ^ seed,
                spare: None,
            },
            hist: [[0.0; 2]; 4],
            frac: 1.0,
            lp: Default::default(),
            hp: Default::default(),
        }
    }

    fn source(&mut self, mono: bool) -> [f32; 2] {
        let draw = |g: &mut Gaussian| {
            if mono {
                let v = g.mono();
                [v, v]
            } else {
                let (a, b) = g.pair();
                [a, b]
            }
        };
        if self.sr <= 44100.0 {
            return draw(&mut self.generator);
        }
        let step = 44100.0 / self.sr as f64;
        self.frac += step;
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.hist.copy_within(1.., 0);
            self.hist[3] = draw(&mut self.generator);
        }
        let t = self.frac as f32;
        [0, 1].map(|c| {
            let (y0, y1, y2, y3) = (
                self.hist[0][c],
                self.hist[1][c],
                self.hist[2][c],
                self.hist[3][c],
            );
            let c1 = 0.5 * (y2 - y0);
            let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
            let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
            ((c3 * t + c2) * t + c1) * t + y1
        })
    }

    /// `generated` is false for the "Off" source and for sample-based sources.
    #[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        volume: Wave,
        gain: f32,
        lp_cut: f32,
        hp_cut: f32,
        mono: bool,
        solo: bool,
        generated: bool,
    ) {
        let g = ((gain * 60.0 - 48.0) / 6.0).exp2() * 0.5;
        let (klp, khp) = (coef(lp_cut, self.sr), coef(hp_cut, self.sr));
        let (rl, rh) = (
            (100.0 * lp_cut - 99.0).max(0.0),
            (1.0 - 100.0 * hp_cut).max(0.0),
        );
        for (i, f) in buf.iter_mut().enumerate() {
            let mut s = if generated {
                self.source(mono)
            } else {
                [0.0; 2]
            };
            let w = volume.at(i) * g;
            for c in 0..2 {
                let mut x = s[c] * w;
                if lp_cut < 1.0 {
                    let y = self.lp[c].lp(x, klp);
                    x = y + rl * rl * (x - y);
                }
                if hp_cut > 0.0 {
                    let h = self.hp[c].hp(x, khp);
                    x = h + rh * rh * (x - h);
                }
                s[c] = x;
            }
            if solo {
                *f = s;
            } else {
                f[0] += s[0];
                f[1] += s[1];
            }
        }
    }
}
