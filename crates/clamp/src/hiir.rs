//! HIIR 2x polyphase IIR half-band filters by Laurent de Soras, as iPlug2 ships them in
//! `IPlug/Extras/HIIR`: `Upsampler2xFPU` and `Downsampler2xFPU`, both built on
//! `StageProcFPU::process_sample_pos`.
//!
//! The two channels run together as the two lanes of each value. The lanes are independent, so
//! the core runs them in parallel. Each lane keeps the operation order of `StageProcFPU`, so it
//! gives the same doubles as the C++. The speed limit is the latency of each section's
//! recurrence, which a different operation order could shorten only by changing the results.

/// Two lanes: left and right.
type Lr = [f64; 2];

/// One first-order allpass section on both lanes: `y = (s - y1) c + x1`.
#[inline(always)]
fn section(s: &mut Lr, c: f64, x: &mut Lr, y: &mut Lr) {
    let t = [(s[0] - y[0]) * c + x[0], (s[1] - y[1]) * c + x[1]];
    *x = *s;
    *y = t;
    *s = t;
}

/// A chain of `NC` first-order allpass sections for two channels. Even coefficients filter path
/// 0, odd coefficients path 1.
#[derive(Clone)]
pub struct Stage<const NC: usize> {
    coef: [f64; NC],
    x: [Lr; NC],
    y: [Lr; NC],
}

impl<const NC: usize> Stage<NC> {
    /// `set_coefs`.
    pub fn new(coef: [f64; NC]) -> Self {
        Self {
            coef,
            x: [[0.0; 2]; NC],
            y: [[0.0; 2]; NC],
        }
    }

    /// `clear_buffers`.
    pub fn clear(&mut self) {
        self.x = [[0.0; 2]; NC];
        self.y = [[0.0; 2]; NC];
    }

    /// `process_sample_pos` on both channels: path 0 in `s0`, path 1 in `s1`.
    #[inline(always)]
    fn run(&mut self, s0: &mut Lr, s1: &mut Lr) {
        let mut i = 0;
        while i + 1 < NC {
            section(s0, self.coef[i], &mut self.x[i], &mut self.y[i]);
            section(s1, self.coef[i + 1], &mut self.x[i + 1], &mut self.y[i + 1]);
            i += 2;
        }
        if NC % 2 == 1 {
            section(
                s0,
                self.coef[NC - 1],
                &mut self.x[NC - 1],
                &mut self.y[NC - 1],
            );
        }
    }

    /// `Upsampler2xFPU::process_block` on both channels: `out` gets `2 inp.len()` frames.
    #[inline(always)]
    pub fn upsample(&mut self, out: &mut [Lr], inp: &[Lr]) {
        for (o, &i) in out.as_chunks_mut::<2>().0.iter_mut().zip(inp) {
            let (mut s0, mut s1) = (i, i);
            self.run(&mut s0, &mut s1);
            *o = [s0, s1];
        }
    }

    /// `Downsampler2xFPU::process_block` on both channels: `out` gets `inp.len() / 2` frames.
    #[inline(always)]
    pub fn downsample(&mut self, out: &mut [Lr], inp: &[Lr]) {
        for (o, &[a, b]) in out.iter_mut().zip(inp.as_chunks::<2>().0) {
            // Path 0 takes the second sample of the pair.
            let (mut s0, mut s1) = (b, a);
            self.run(&mut s0, &mut s1);
            *o = [0.5 * (s0[0] + s1[0]), 0.5 * (s0[1] + s1[1])];
        }
    }
}
