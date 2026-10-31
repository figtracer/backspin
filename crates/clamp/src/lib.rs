//! The Clamp clipper by Outobugi, ported from the C++ reconstruction in `clamp-mac`
//! (`Clamp/ClampDSP.h`): IN and OUT gain, the TYPE A and TYPE B waveshapers in six channel modes,
//! the iPlug2 HIIR oversampler (off to 16x) and the level meter.
//!
//! The audio path computes in double precision, as the C++ does. [`Clamp::process`] takes and
//! gives `f32`, [`Clamp::process_f64`] takes and gives `f64`. Neither allocates.

mod hiir;
mod meter;
mod oversampler;
mod shaper;

use meter::Meter;
use oversampler::OverSampler;
use shaper::{BLOCK_FNS, Shaper};

/// The plugin parameters, with the original's ranges and defaults. The values are not smoothed;
/// the DSP reads them once per block, as the original does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// BYPASS: copies input to output and stops the meter.
    pub bypass: bool,
    /// OVERSAMPLING: 0 (off), 1 (2x), 2 (4x), 3 (8x), 4 (16x). Larger values act as 4.
    pub oversampling: u8,
    /// IN gain in dB, -24 to 24 (step 0.5).
    pub in_db: f64,
    /// OUT gain in dB, -24 to 24 (step 0.5).
    pub out_db: f64,
    /// CEILING in dB, -48 to 0 (step 0.5).
    pub ceiling_db: f64,
    /// SHAPE, 0 to 1 (step 0.01): soft to hard.
    pub shape: f64,
    /// TYPE: 0 (TYPE A, tanh and hard clip crossfade), otherwise TYPE B (soft-knee gain computer).
    pub kind: u8,
    /// MODE: 0 (LR), 1 (L), 2 (R), 3 (MS), 4 (M), 5 (S). Larger values act as 5.
    pub mode: u8,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            bypass: false,
            oversampling: 0,
            in_db: 0.0,
            out_db: 0.0,
            ceiling_db: 0.0,
            shape: 1.0,
            kind: 0,
            mode: 0,
        }
    }
}

/// [`Clamp::group_delay`] for each OVERSAMPLING value, measured by `group_delay_matches`.
const GROUP_DELAY: [f64; 5] = [0.0, 3.4717, 4.5796, 5.0090, 5.1481];

/// A sample type that the audio path converts to and from `f64`.
trait Sample: Copy {
    fn to_f64(self) -> f64;
    fn from_f64(x: f64) -> Self;
}

impl Sample for f32 {
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }

    #[inline(always)]
    fn from_f64(x: f64) -> Self {
        x as f32
    }
}

impl Sample for f64 {
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self
    }

    #[inline(always)]
    fn from_f64(x: f64) -> Self {
        x
    }
}

/// One stereo Clamp instance (`ClampDSP`).
pub struct Clamp {
    sample_rate: f64,
    /// Interleaved frames of the block being processed, `max_block` long.
    io: Vec<[f64; 2]>,
    os: OverSampler,
    /// OVERSAMPLING as the last block saw it, bypassed or not.
    oversampling: u8,
    /// Set when OVERSAMPLING changes, cleared by the next block that is not bypassed.
    reset_pending: bool,
    shaper: Shaper,
    meter: Meter,
    meter_rate: f64,
}

impl Clamp {
    /// Allocates the buffers for blocks of up to `max_block` frames.
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let max_block = max_block.max(1);
        Self {
            sample_rate: sample_rate as f64,
            io: vec![[0.0; 2]; max_block],
            os: OverSampler::new(max_block),
            oversampling: 0,
            reset_pending: true,
            shaper: Shaper::default(),
            meter: Meter::default(),
            meter_rate: 0.0,
        }
    }

    /// Grows the buffers for blocks of up to `max_block` frames. Call it outside the audio thread.
    /// It keeps the filter state, like `ClampDSP::SetBlockSize`.
    pub fn set_max_block(&mut self, max_block: usize) {
        if max_block > self.io.len() {
            self.io.resize(max_block, [0.0; 2]);
        }
        self.os.grow(max_block);
    }

    /// The latency in samples to report to the host. Like the original and iPlug2's
    /// `OverSampler`, Clamp reports none: the oversampler's IIR filters are not linear phase and
    /// are not compensated. [`Clamp::group_delay`] gives their delay.
    pub fn latency(&self, _p: &Params) -> u32 {
        0
    }

    /// The group delay at DC of the oversampler at the host rate, in samples. It increases towards
    /// Nyquist: at 10 kHz and 48 kHz it is about one sample more.
    pub fn group_delay(&self, p: &Params) -> f64 {
        GROUP_DELAY[p.oversampling.min(4) as usize]
    }

    /// The latest `(post, pre)` meter window means: the peak envelope after the shaper and OUT
    /// gain, and after IN gain.
    pub fn meter(&self) -> (f32, f32) {
        self.meter.last()
    }

    /// Processes a stereo block in place. `l` and `r` have the same length.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32], p: &Params) {
        self.run(l, r, p);
    }

    /// [`Clamp::process`] with double-precision input and output, as the C++ processes.
    pub fn process_f64(&mut self, l: &mut [f64], r: &mut [f64], p: &Params) {
        self.run(l, r, p);
    }

    /// `ClampDSP::Process` (the original's ProcessBlock, 0x180548b40).
    fn run<T: Sample>(&mut self, l: &mut [T], r: &mut [T], p: &Params) {
        debug_assert_eq!(l.len(), r.len());

        // The original arms the oversampler reset when OVERSAMPLING changes (OnParamChange,
        // 0x180549140), never on a host reset, and applies it on the audio thread at the next
        // block that is not bypassed. A change and a change back while bypassed still reset.
        let factor = p.oversampling.min(4);
        if factor != self.oversampling {
            self.oversampling = factor;
            self.reset_pending = true;
        }

        if p.bypass {
            return;
        }

        self.shaper.set(p.in_db, p.out_db, p.ceiling_db, p.shape);
        let block_fn = BLOCK_FNS[(p.kind != 0) as usize][p.mode.min(5) as usize];

        if self.reset_pending {
            self.reset_pending = false;
            self.os.reset(factor);
        }

        let meter_rate = (1u32 << factor) as f64 * self.sample_rate;
        if meter_rate != self.meter_rate {
            self.meter_rate = meter_rate;
            self.meter.set_rate(meter_rate);
        }

        // The C++ resets the oversampler for a block that is larger than its buffers. The port
        // processes such a block in parts instead, which gives the same output as one block.
        let n = l.len().min(r.len());
        let max = self.io.len().min(self.os.max_block());
        let (shaper, meter) = (&self.shaper, &mut self.meter);
        for start in (0..n).step_by(max) {
            let len = max.min(n - start);
            let (l, r) = (&mut l[start..start + len], &mut r[start..start + len]);
            let io = &mut self.io[..len];
            for ((f, &a), &b) in io.iter_mut().zip(l.iter()).zip(r.iter()) {
                *f = [a.to_f64(), b.to_f64()];
            }
            block_fn(&mut self.os, io, shaper, meter);
            for ((&[a, b], x), y) in io.iter().zip(l.iter_mut()).zip(r.iter_mut()) {
                *x = T::from_f64(a);
                *y = T::from_f64(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sum(n h[n]) / sum(h[n])` of the oversampler impulse response with nothing in between.
    #[test]
    fn group_delay_matches() {
        for factor in 0..5u8 {
            let n = 1 << 14;
            let mut os = OverSampler::new(n);
            os.reset(factor);
            let mut io = vec![[0.0; 2]; n];
            io[0] = [1.0, 1.0];
            os.process(&mut io, |_| {});
            let gain: f64 = io.iter().map(|x| x[0]).sum();
            let moment: f64 = io.iter().enumerate().map(|(i, x)| i as f64 * x[0]).sum();
            assert!((gain - 1.0).abs() < 1e-9);
            assert!(
                (moment / gain - GROUP_DELAY[factor as usize]).abs() < 1e-4,
                "{factor}: {}",
                moment / gain
            );
        }
    }
}
