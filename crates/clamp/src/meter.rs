//! The original's level meter (`ClampMeter`, plugin + 0xbe8, update 0x180549360): a peak envelope
//! with instant attack and a one-pole release, averaged over 2.5 ms windows of the oversampled
//! signal. Single precision, as the original computes it.

#[derive(Clone)]
pub struct Meter {
    window: i32,
    release: f32,
    count: i32,
    envelope: [f32; 2],
    sum: [f32; 2],
    /// The last `(post, pre)` window means sent.
    last: (f32, f32),
}

impl Default for Meter {
    /// Constructor values of the original (5 ms at 48 kHz). The first block replaces them.
    fn default() -> Self {
        Self {
            window: 240,
            release: 5.0 / 48.0,
            count: 0,
            envelope: [0.0; 2],
            sum: [0.0; 2],
            last: (0.0, 0.0),
        }
    }
}

impl Meter {
    /// Window length and release for the oversampled rate, as 0x180548eb0 sets them.
    pub fn set_rate(&mut self, oversampled_rate: f64) {
        self.window = ((oversampled_rate * 0.0025) as i32).max(1);
        self.release = ((1.0 / (oversampled_rate / 1000.0)) * 0.015) as f32;
    }

    /// Adds one oversampled sample. At the start of each window, sends the window means.
    #[inline(always)]
    pub fn add(&mut self, post: f64, pre: f64) {
        if self.count == 0 {
            let n = self.window as f32;
            self.last = (self.sum[0] / n, self.sum[1] / n);
            self.sum = [0.0; 2];
        }
        // `(count + 1) % window`. The count can be past a window that a rate change shortened,
        // so the remainder stays, but only off the common path.
        let c = self.count + 1;
        self.count = if c < self.window { c } else { c % self.window };
        self.follow(0, post);
        self.follow(1, pre);
    }

    #[inline(always)]
    fn follow(&mut self, c: usize, value: f64) {
        let mut x = (value as f32).abs();
        if x < self.envelope[c] {
            x = (x - self.envelope[c]) * self.release + self.envelope[c];
        }
        self.envelope[c] = x;
        self.sum[c] += x;
    }

    /// The last `(post, pre)` window means.
    pub fn last(&self) -> (f32, f32) {
        self.last
    }
}
