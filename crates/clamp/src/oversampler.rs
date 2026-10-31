//! iPlug2 `OverSampler<double>` (`IPlug/Extras/Oversampler.h`) in block mode with two channels.
//!
//! Each factor cascades 2x HIIR stages with 12, 4, 3 and 2 coefficients. iPlug2 upsamples a block
//! to the full rate stage by stage, gives it to the process function, then downsamples it stage by
//! stage. Like iPlug2, the filters are not latency compensated and their state is cleared only by
//! [`OverSampler::reset`].
//!
//! Each filter streams sample by sample, so the order in which the port interleaves the stages
//! does not change the output. The port uses that for speed. At 2x it runs stage by stage over
//! the block, like iPlug2, so that the 12 sections keep their state in registers. At 4x to 16x it
//! runs all stages for one host frame at a time, so that the recurrences of the different stages
//! overlap instead of running one after the other.

use crate::hiir::Stage;

/// `coeffs2x`, `coeffs4x`, `coeffs8x` and `coeffs16x` of `OverSampler`, as written there.
#[allow(clippy::excessive_precision)]
const COEFS_2X: [f64; 12] = [
    0.036681502163648017,
    0.13654762463195794,
    0.27463175937945444,
    0.42313861743656711,
    0.56109869787919531,
    0.67754004997416184,
    0.76974183386322703,
    0.83988962484963892,
    0.89226081800387902,
    0.9315419599631839,
    0.96209454837808417,
    0.98781637073289585,
];
#[allow(clippy::excessive_precision)]
const COEFS_4X: [f64; 4] = [
    0.041893991997656171,
    0.16890348243995201,
    0.39056077292116603,
    0.74389574826847926,
];
#[allow(clippy::excessive_precision)]
const COEFS_8X: [f64; 3] = [
    0.055748680811302048,
    0.24305119574153072,
    0.64669913119268196,
];
#[allow(clippy::excessive_precision)]
const COEFS_16X: [f64; 2] = [0.10717745346023573, 0.53091435354504557];

/// One direction of the cascade: the 2x, 4x, 8x and 16x stages.
#[derive(Clone)]
struct Cascade {
    s2: Stage<12>,
    s4: Stage<4>,
    s8: Stage<3>,
    s16: Stage<2>,
}

impl Cascade {
    fn new() -> Self {
        Self {
            s2: Stage::new(COEFS_2X),
            s4: Stage::new(COEFS_4X),
            s8: Stage::new(COEFS_8X),
            s16: Stage::new(COEFS_16X),
        }
    }

    fn clear(&mut self) {
        self.s2.clear();
        self.s4.clear();
        self.s8.clear();
        self.s16.clear();
    }
}

/// Stereo block oversampler with iPlug2's stage structure.
pub struct OverSampler {
    /// `EFactor`: 0 (none) to 4 (16x).
    factor: u8,
    up: Cascade,
    down: Cascade,
    /// The 2x frames of a block.
    buf: Vec<[f64; 2]>,
}

impl OverSampler {
    pub fn new(max_block: usize) -> Self {
        let mut os = Self {
            factor: 0,
            up: Cascade::new(),
            down: Cascade::new(),
            buf: Vec::new(),
        };
        os.grow(max_block);
        os
    }

    /// Makes room for blocks of `max_block` frames. Keeps the filter state.
    pub fn grow(&mut self, max_block: usize) {
        let len = 2 * max_block.max(1);
        if self.buf.len() < len {
            self.buf.resize(len, [0.0; 2]);
        }
    }

    /// The largest block that [`OverSampler::process`] takes.
    pub fn max_block(&self) -> usize {
        self.buf.len() / 2
    }

    /// `SetOverSampling` followed by `Reset`: sets the factor (0 to 4) and clears all filters.
    pub fn reset(&mut self, factor: u8) {
        self.factor = factor.min(4);
        self.up.clear();
        self.down.clear();
    }

    /// `ProcessBlock`: upsamples `io`, calls `func` on the oversampled frames to change them in
    /// place, and downsamples the result back into `io`. `io` has at most `max_block` frames.
    /// `func` must process each frame on its own, in order: iPlug2 calls it once for each of the
    /// `rate` segments of the block, and the port calls it on other runs of the same frames.
    #[inline(always)]
    pub fn process(&mut self, io: &mut [[f64; 2]], mut func: impl FnMut(&mut [[f64; 2]])) {
        match self.factor {
            0 => func(io),
            1 => {
                let buf = &mut self.buf[..2 * io.len()];
                self.up.s2.upsample(buf, io);
                func(buf);
                self.down.s2.downsample(io, buf);
            }
            2 => self.per_frame::<2>(io, &mut func),
            3 => self.per_frame::<3>(io, &mut func),
            _ => self.per_frame::<4>(io, &mut func),
        }
    }

    /// Runs every stage of factor `F` for one host frame at a time.
    #[inline(always)]
    fn per_frame<const F: u8>(
        &mut self,
        io: &mut [[f64; 2]],
        func: &mut impl FnMut(&mut [[f64; 2]]),
    ) {
        let (up, down) = (&mut self.up, &mut self.down);
        for x in io {
            let mut b2 = [[0.0; 2]; 2];
            let mut b4 = [[0.0; 2]; 4];
            let mut b8 = [[0.0; 2]; 8];
            let mut b16 = [[0.0; 2]; 16];
            up.s2.upsample(&mut b2, std::slice::from_ref(x));
            up.s4.upsample(&mut b4, &b2);
            if F >= 3 {
                up.s8.upsample(&mut b8, &b4);
            }
            if F == 4 {
                up.s16.upsample(&mut b16, &b8);
            }
            match F {
                2 => func(&mut b4),
                3 => func(&mut b8),
                _ => func(&mut b16),
            }
            if F == 4 {
                down.s16.downsample(&mut b8, &b16);
            }
            if F >= 3 {
                down.s8.downsample(&mut b4, &b8);
            }
            down.s4.downsample(&mut b2, &b4);
            down.s2.downsample(std::slice::from_mut(x), &b2);
        }
    }
}
