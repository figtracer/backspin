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
    /// Ring frames written since the ring was last all zero (at most `len`): they end at
    /// `write`, so [`Ring::zero`] clears only them.
    dirty: usize,
}

impl Ring {
    /// Sizes the ring for `block` frames and a delay range of `delay` frames (0x24a230 with
    /// guard 16). The ring has `delay + block + 2 * GUARD` frames, all zero, and the write
    /// position is `GUARD + delay`.
    pub fn resize(&mut self, block: usize, delay: usize) {
        let total = 4 * GUARD + delay + block;
        self.len = total - 2 * GUARD;
        // A fresh zeroed allocation: the system maps its pages as the ring first writes them,
        // so the 72 s ring does not take its full size in memory until it has been played
        // through (`resize` would write every frame up front).
        self.data = vec![[0.0; 2]; self.len + 2 * GUARD];
        self.write = GUARD + delay;
        self.dirty = 0;
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

    /// Zeroes every frame. The write position is kept (0x249d70). Only the frames written
    /// since the last zero are cleared (the rest are zero already), so a flush or a stop does
    /// not write the whole 72 s ring on the audio thread.
    pub fn zero(&mut self) {
        let (len, w, n) = (self.len, self.write, self.dirty);
        if n >= len {
            self.data.fill([0.0; 2]);
        } else if n > 0 {
            if n <= w {
                self.data[GUARD + w - n..GUARD + w].fill([0.0; 2]);
            } else {
                self.data[GUARD..GUARD + w].fill([0.0; 2]);
                self.data[GUARD + len - (n - w)..GUARD + len].fill([0.0; 2]);
            }
            self.data[..GUARD].fill([0.0; 2]);
            self.data[GUARD + len..].fill([0.0; 2]);
        }
        self.dirty = 0;
    }

    /// Appends `src` at the write position, wrapping at the end, then refreshes both mirrors
    /// (0x24a310).
    pub fn record(&mut self, src: &[[f32; 2]]) {
        self.dirty = (self.dirty + src.len()).min(self.len);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Clearing only the written frames leaves the whole ring zero, mirrors included, for any
    /// mix of block sizes and wraps.
    #[test]
    fn zero_clears_every_written_frame() {
        let mut ring = Ring::default();
        ring.resize(64, 1000);
        let mut seed = 7u32;
        let mut next = |m: u32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) % m
        };
        for round in 0..400 {
            for _ in 0..next(40) {
                let n = 1 + next(64) as usize;
                let block: Vec<[f32; 2]> = (0..n).map(|k| [k as f32 + 1.0, -1.0]).collect();
                ring.record(&block);
            }
            ring.zero();
            assert!(
                ring.data.iter().all(|f| *f == [0.0; 2]),
                "round {round}: frames left after zero"
            );
        }
    }
}
