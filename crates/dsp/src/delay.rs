//! The dry signal delay (TBasicDelay_32FS with 15 frames).

/// The plugin latency in frames: the dry delay length.
pub const LATENCY: usize = 15;

/// A fixed 15-frame stereo delay line.
#[derive(Clone, Debug, Default)]
pub struct DryDelay {
    buf: [[f32; 2]; LATENCY],
    pos: usize,
}

impl DryDelay {
    /// Zeroes the line.
    pub fn reset(&mut self) {
        self.buf = [[0.0; 2]; LATENCY];
    }

    /// Delays `buf` in place by 15 frames.
    pub fn process(&mut self, buf: &mut [[f32; 2]]) {
        for f in buf {
            std::mem::swap(f, &mut self.buf[self.pos]);
            self.pos = (self.pos + 1) % LATENCY;
        }
    }
}
