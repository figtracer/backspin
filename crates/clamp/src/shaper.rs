//! The `Waveshapers` namespace of Clamp (`Clamp/Waveshapers.h` of the C++ port): gain staging,
//! channel modes (`Waveshaper::Process`, 0x180542380) and the two shape functions.
//!
//! The C++ selects the shaper with a virtual call for each sample. The port selects one
//! monomorphized block function for each TYPE and MODE once per block.

use crate::meter::Meter;
use crate::oversampler::OverSampler;

/// Soft-knee width in dB at SHAPE 0 (Clamp member at offset 0x870).
const MAX_KNEE_DB: f64 = 24.0;

/// `20 / ln(10)`, as the original has it.
const LN_TO_DB: f64 = 8.685889638065037;

/// `ln(10) / 20` cut to 13 digits, as the original has it.
const DB_TO_LN: f64 = 0.1151292546497;

/// Same conversion as the original: `exp(dB * ln(10) / 20)`.
pub fn db_to_amp(db: f64) -> f64 {
    (db * DB_TO_LN).exp()
}

/// The per-block settings of both shapers, set as `ClampDSP::Process` sets them.
#[derive(Clone, Copy, Default)]
pub struct Shaper {
    in_gain: f64,
    out_gain: f64,
    /// TYPE A: linear ceiling and SHAPE.
    ceiling: f64,
    shape: f64,
    /// TYPE B: `Clipper::SetThreshold`.
    threshold_db: f64,
    knee_db: f64,
    knee_lo_db: f64,
    knee_hi_db: f64,
}

impl Shaper {
    pub fn set(&mut self, in_db: f64, out_db: f64, ceiling_db: f64, shape: f64) {
        self.in_gain = db_to_amp(in_db);
        self.out_gain = db_to_amp(out_db);
        self.ceiling = db_to_amp(ceiling_db);
        self.shape = shape;
        let knee_db = (1.0 - shape) * MAX_KNEE_DB;
        self.threshold_db = ceiling_db;
        self.knee_db = knee_db;
        self.knee_lo_db = ceiling_db - knee_db * 0.5;
        self.knee_hi_db = ceiling_db + knee_db * 0.5;
    }

    /// TYPE A (`NaiveClipper`, 0x1805425b0): crossfade between tanh saturation (SHAPE 0) and a
    /// hard clip (SHAPE 1).
    #[inline(always)]
    fn naive(&self, x: f64) -> f64 {
        let c = self.ceiling;
        let soft = (x / c).tanh() * c;
        // `std::clamp(x, -c, c)`, NaN passes through.
        let hard = x.clamp(-c, c);
        (hard - soft) * self.shape + soft
    }

    /// TYPE B (`Clipper`, 0x180542660): infinite-ratio gain computer with a quadratic soft knee
    /// in dB.
    #[inline(always)]
    fn clipper(&self, x: f64) -> f64 {
        let level_db = x.abs().ln() * LN_TO_DB;
        let gain_db =
            if self.knee_db > 0.0 && level_db > self.knee_lo_db && level_db < self.knee_hi_db {
                ((level_db - self.knee_lo_db) / self.knee_db) * 0.5 * (self.knee_lo_db - level_db)
            } else {
                // `std::min(a, 0.)` returns `a` unless `0 < a`, so NaN passes through.
                let a = self.threshold_db - level_db;
                if 0.0 < a { 0.0 } else { a }
            };
        (gain_db * DB_TO_LN).exp() * x
    }

    /// `ProcessOne`: returns the shaped sample, `|x|` after IN gain, and `|x|` after OUT gain.
    #[inline(always)]
    fn one<const KIND: u8>(&self, x: f64) -> (f64, f64, f64) {
        let x = x * self.in_gain;
        let pre = x.abs();
        let x = if KIND == 0 {
            self.naive(x)
        } else {
            self.clipper(x)
        };
        let x = x * self.out_gain;
        (x, pre, x.abs())
    }
}

/// Oversamples, shapes and meters one block of frames in place.
pub type BlockFn = fn(&mut OverSampler, &mut [[f64; 2]], &Shaper, &mut Meter);

/// The block functions by TYPE (0, 1) and MODE (LR, L, R, MS, M, S).
pub const BLOCK_FNS: [[BlockFn; 6]; 2] = [
    [
        run::<0, 0>,
        run::<0, 1>,
        run::<0, 2>,
        run::<0, 3>,
        run::<0, 4>,
        run::<0, 5>,
    ],
    [
        run::<1, 0>,
        run::<1, 1>,
        run::<1, 2>,
        run::<1, 3>,
        run::<1, 4>,
        run::<1, 5>,
    ],
];

/// The oversampler with [`block`] as its process function.
fn run<const KIND: u8, const MODE: u8>(
    os: &mut OverSampler,
    io: &mut [[f64; 2]],
    sh: &Shaper,
    meter: &mut Meter,
) {
    os.process(io, |frames| block::<KIND, MODE>(frames, sh, meter));
}

/// `Waveshaper::Process` for each frame, then the meter update of the oversampler lambda
/// (0x18055be60).
#[inline(always)]
fn block<const KIND: u8, const MODE: u8>(frames: &mut [[f64; 2]], sh: &Shaper, meter: &mut Meter) {
    for f in frames {
        let [dry_l, dry_r] = *f;
        let (mut l, mut r) = (dry_l, dry_r);
        // A channel that is not processed meters 0.
        let (pre, post);
        if MODE < 3 {
            let (mut pre_l, mut post_l, mut pre_r, mut post_r) = (0.0, 0.0, 0.0, 0.0);
            if MODE == 0 || MODE == 1 {
                (l, pre_l, post_l) = sh.one::<KIND>(l);
            }
            if MODE == 0 || MODE == 2 {
                (r, pre_r, post_r) = sh.one::<KIND>(r);
            }
            // `std::fmax`, which drops a NaN like `f64::max`.
            pre = f64::max(pre_l, pre_r);
            post = f64::max(post_l, post_r);
        } else {
            let (mut m, mut s) = ((l + r) * 0.5, (l - r) * 0.5);
            let (mut pre_m, mut post_m, mut pre_s, mut post_s) = (0.0, 0.0, 0.0, 0.0);
            if MODE == 3 || MODE == 4 {
                (m, pre_m, post_m) = sh.one::<KIND>(m);
            }
            if MODE == 3 || MODE == 5 {
                (s, pre_s, post_s) = sh.one::<KIND>(s);
            }
            l = m + s;
            r = m - s;
            // The original meters `|M| + |S|` on both sides.
            pre = pre_m + pre_s;
            post = post_m + post_s;
        }
        // The original's dry/wet stage, `(x - dry) * mix + dry`, with its mix always 1. Multiplying
        // by 1 is exact, but the subtraction and addition can round, so both stay.
        *f = [(l - dry_l) + dry_l, (r - dry_r) + dry_r];
        meter.add(post, pre);
    }
}
