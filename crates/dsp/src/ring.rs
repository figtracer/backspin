//! The recording ring buffer (TBRDelay_32FS, dsp.md section 2).

/// Frames mirrored on each side of the ring, so the interpolator can read past either end.
pub const GUARD: usize = 16;

/// A ring of stereo f32 frames with mirrored guard frames, laid out like the original:
/// `GUARD` mirror frames, `len` ring frames, `GUARD` mirror frames.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    data: Vec<[f32; 2]>,
    len: usize,
    /// Write position in ring frames, `0..=len`.
    write: usize,
}

impl Ring {
    /// Sizes the ring for `block` frames and a delay range of `delay` frames (0x24a230 with
    /// guard 16). The ring has `delay + block + 2 * GUARD` frames, all zero, and the write
    /// position is `GUARD + delay`.
    pub fn resize(&mut self, block: usize, delay: usize) {
        let total = 4 * GUARD + delay + block;
        self.len = total - 2 * GUARD;
        self.data.clear();
        self.data.resize(self.len + 2 * GUARD, [0.0; 2]);
        self.write = GUARD + delay;
    }

    /// The number of ring frames.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the ring has no frames.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The write position in ring frames.
    pub fn write_pos(&self) -> usize {
        self.write
    }

    /// Zeroes every frame. The write position is kept (0x249d70).
    pub fn zero(&mut self) {
        self.data.fill([0.0; 2]);
    }

    /// Appends `src` at the write position, wrapping at the end, then refreshes both mirrors
    /// (0x24a310).
    pub fn record(&mut self, src: &[[f32; 2]]) {
        let mut src = src;
        let room = self.len - self.write;
        if room < src.len() {
            if room > 0 {
                self.data[GUARD + self.write..GUARD + self.len].copy_from_slice(&src[..room]);
                src = &src[room..];
            }
            self.write = 0;
        }
        if !src.is_empty() {
            self.data[GUARD + self.write..GUARD + self.write + src.len()].copy_from_slice(src);
            self.write += src.len();
        }
        let len = self.len;
        self.data.copy_within(GUARD..2 * GUARD, GUARD + len);
        self.data.copy_within(len..len + GUARD, 0);
    }

    /// The frame at ring position `i` plus `j`, for `-GUARD < j <= GUARD` (the original reads
    /// the guard mirrors directly).
    #[inline]
    pub fn frame(&self, i: usize, j: isize) -> [f32; 2] {
        self.data[(GUARD + i).wrapping_add_signed(j)]
    }
}
