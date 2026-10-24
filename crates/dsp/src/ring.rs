//! Stereo ring buffer addressed by absolute frame index.

use crate::Frame;

/// A stereo ring buffer. Frames are addressed by their absolute index since the last clear.
///
/// Reads outside the recorded range return silence.
#[derive(Clone, Debug)]
pub struct Ring {
    buf: Vec<Frame>,
    mask: usize,
    written: u64,
}

impl Ring {
    /// Creates a ring that holds at least `min_capacity` frames.
    pub fn new(min_capacity: usize) -> Self {
        let cap = min_capacity.max(16).next_power_of_two();
        Self { buf: vec![[0.0; 2]; cap], mask: cap - 1, written: 0 }
    }

    /// Returns the number of frames the ring holds.
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Returns the absolute index of the next frame to write.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Forgets all recorded audio.
    pub fn clear(&mut self) {
        self.buf.fill([0.0; 2]);
        self.written = 0;
    }

    /// Appends a frame.
    #[inline]
    pub fn push(&mut self, frame: Frame) {
        self.buf[self.written as usize & self.mask] = frame;
        self.written += 1;
    }

    /// Returns true if the absolute index `i` holds recorded audio.
    #[inline]
    pub fn contains(&self, i: i64) -> bool {
        i >= 0 && (i as u64) < self.written && self.written - (i as u64) <= self.buf.len() as u64
    }

    /// Returns the frame at absolute index `i`, or silence if it is not recorded.
    #[inline]
    pub fn frame(&self, i: i64) -> Frame {
        if self.contains(i) {
            self.buf[i as usize & self.mask]
        } else {
            [0.0; 2]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_read_back() {
        let mut ring = Ring::new(16);
        for i in 0..40 {
            ring.push([i as f32, -(i as f32)]);
        }
        assert_eq!(ring.frame(39), [39.0, -39.0]);
        assert_eq!(ring.frame(24), [24.0, -24.0]);
        // Overwritten, future, and negative frames are silent.
        assert_eq!(ring.frame(23), [0.0, 0.0]);
        assert_eq!(ring.frame(40), [0.0, 0.0]);
        assert_eq!(ring.frame(-1), [0.0, 0.0]);
    }
}
