//! `TDIB` (VMT `0x2fd840`): a 32 bpp bitmap with a movable origin and a clip rect, and the raster
//! primitives of the unit around `0x170000..0x187000` (gfx.md section 3).
//!
//! A pixel is a `u32` `0xAARRGGBB` (memory bytes B, G, R, A). Rects are `[x0, y0, x1, y1)` in
//! coordinates relative to the origin. The Agg2D canvas is created on first use and kept for the
//! life of the bitmap, so its state (colours, line width, joins, gradients) carries from one
//! painter to the next as in the original.

use backspin_agg2d::Agg2D;

/// A rect `[x0, y0, x1, y1)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn intersect(self, o: Rect) -> Rect {
        Rect::new(
            self.x0.max(o.x0),
            self.y0.max(o.y0),
            self.x1.min(o.x1),
            self.y1.min(o.y1),
        )
    }

    pub fn is_empty(self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn offset(self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
}

/// A 32 bpp `TDIB`.
pub struct Dib {
    w: i32,
    h: i32,
    px: Box<[u32]>,
    /// Origin offset in pixels (`+0x78/+0x7c`).
    ox: i32,
    oy: i32,
    /// Clip in origin coordinates (`+0xe0..+0xec`).
    clip: Rect,
    agg: Option<Agg2D>,
}

impl Dib {
    /// A bitmap filled with `colour`.
    pub fn new(w: i32, h: i32, colour: u32) -> Self {
        Self {
            w,
            h,
            px: vec![colour; (w.max(0) * h.max(0)) as usize].into_boxed_slice(),
            ox: 0,
            oy: 0,
            clip: Rect::new(0, 0, w, h),
            agg: None,
        }
    }

    pub fn width(&self) -> i32 {
        self.w
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    /// All pixels, row by row from the top.
    pub fn pixels(&self) -> &[u32] {
        &self.px
    }

    /// The pixel at absolute coordinates.
    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        self.px[(y * self.w + x) as usize]
    }

    pub fn origin(&self) -> (i32, i32) {
        (self.ox, self.oy)
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// `FUN_00186250(dib, dx, dy, 3)`: moves the origin, keeps the clip in place on the bitmap,
    /// and re-applies the Agg2D translation (`FUN_00182730`).
    pub fn move_origin(&mut self, dx: i32, dy: i32) {
        self.ox += dx;
        self.oy += dy;
        self.clip = self.clip.offset(-dx, -dy);
        let (ox, oy) = (self.ox, self.oy);
        if let Some(a) = &mut self.agg {
            a.reset_transformations();
            a.translate(ox as f64, oy as f64);
        }
    }

    /// `FUN_00185f10(dib, rect, 3)`: sets the clip (origin coordinates) and the Agg2D clip box.
    pub fn set_clip(&mut self, r: Rect) {
        self.clip = r;
        let (ox, oy) = (self.ox, self.oy);
        if let Some(a) = &mut self.agg {
            a.clip_box_int(ox + r.x0, oy + r.y0, ox + r.x1, oy + r.y1);
        }
    }

    /// The Agg2D canvas of this bitmap (`FUN_001825e0` on first use).
    pub fn agg(&mut self) -> &mut Agg2D {
        if self.agg.is_none() {
            let mut a = Agg2D::new();
            // SAFETY: `px` is a boxed slice that never moves or changes size while the canvas
            // is attached; the canvas is dropped (detached) with the bitmap.
            unsafe {
                a.attach(
                    self.px.as_mut_ptr() as *mut u8,
                    self.w as u32,
                    self.h as u32,
                    self.w * 4,
                )
            };
            let (ox, oy, c) = (self.ox, self.oy, self.clip);
            a.clip_box_int(ox + c.x0, oy + c.y0, ox + c.x1, oy + c.y1);
            a.reset_transformations();
            a.translate(ox as f64, oy as f64);
            self.agg = Some(a);
        }
        self.agg.as_mut().unwrap()
    }

    /// `FUN_00186040`: the rect clipped, in absolute pixels, or `None` if empty.
    fn clipped(&self, r: Rect) -> Option<Rect> {
        let r = r
            .intersect(self.clip)
            .offset(self.ox, self.oy)
            .intersect(Rect::new(0, 0, self.w, self.h));
        (!r.is_empty()).then_some(r)
    }

    fn rows(&mut self, r: Rect) -> impl Iterator<Item = &mut [u32]> {
        let w = self.w as usize;
        self.px
            .chunks_exact_mut(w)
            .skip(r.y0 as usize)
            .take((r.y1 - r.y0) as usize)
            .map(move |row| &mut row[r.x0 as usize..r.x1 as usize])
    }

    /// `FUN_0017f8e0(dib, rect, colour, alpha)`: `alpha >= 0x100` stores `colour`; below, every
    /// lane (B, G, R, A) becomes `(c*a + d*(256 - a)) >> 8`.
    pub fn fill_rect(&mut self, r: Rect, colour: u32, alpha: i32) {
        let Some(r) = self.clipped(r) else { return };
        if alpha >= 0x100 {
            for row in self.rows(r) {
                row.fill(colour);
            }
            return;
        }
        let a = alpha.max(0) as u32;
        let inv = 256 - a;
        let pre = [0, 8, 16, 24].map(|s| ((colour >> s) & 0xff) * a);
        for row in self.rows(r) {
            for d in row {
                let mut out = 0;
                for (i, s) in [0, 8, 16, 24].into_iter().enumerate() {
                    let v = (pre[i] + ((*d >> s) & 0xff) * inv) >> 8;
                    out |= v.min(255) << s;
                }
                *d = out;
            }
        }
    }

    /// `FUN_0017f320(dib, rect, colour, alpha, t)`: a frame of thickness `t`.
    pub fn frame_rect(&mut self, r: Rect, colour: u32, alpha: i32, t: i32) {
        let Rect { x0, y0, x1, y1 } = r;
        self.fill_rect(Rect::new(x0, y0, x1, (y0 + t).min(y1)), colour, alpha);
        self.fill_rect(Rect::new(x0, (y1 - t).max(y0), x1, y1), colour, alpha);
        self.fill_rect(
            Rect::new(x0, y0 + t, (x0 + t).min(x1), y1 - t),
            colour,
            alpha,
        );
        self.fill_rect(
            Rect::new((x1 - t).max(x0), y0 + t, x1, y1 - t),
            colour,
            alpha,
        );
    }

    /// `FUN_001812b0(factor, dib, rect)`: every lane (B, G, R, A) becomes
    /// `min(255, (d * trunc(factor * 256)) >> 8)`.
    pub fn multiply_rect(&mut self, r: Rect, factor: f32) {
        let Some(r) = self.clipped(r) else { return };
        let m = (factor * 256.0) as u32;
        for row in self.rows(r) {
            for d in row {
                let mut out = 0;
                for s in [0, 8, 16, 24] {
                    out |= ((((*d >> s) & 0xff) * m) >> 8).min(255) << s;
                }
                *d = out;
            }
        }
    }

    /// `FUN_00170ea0(dib, x, y, colour, a)`: per lane `d += ((c - d) * a) >> 8` (floor), `a` in
    /// 0..256, saturated.
    pub fn blend_pixel(&mut self, x: i32, y: i32, colour: u32, a: i32) {
        let c = self.clip;
        if x < c.x0 || x >= c.x1 || y < c.y0 || y >= c.y1 {
            return;
        }
        let (x, y) = (x + self.ox, y + self.oy);
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.px[i];
        let mut out = 0;
        for s in [0, 8, 16, 24] {
            let dc = ((d >> s) & 0xff) as i32;
            let cc = ((colour >> s) & 0xff) as i32;
            let v = dc + (((cc - dc) * a) >> 8);
            out |= (v.clamp(0, 255) as u32) << s;
        }
        self.px[i] = out;
    }
}

/// `FUN_001a1d70(c, d)`: adds `d` to the three colour channels, clamped (alpha untouched).
pub fn lighten(c: u32, d: i32) -> u32 {
    let mut out = c & 0xff00_0000;
    for s in [0, 8, 16] {
        out |= ((((c >> s) & 0xff) as i32 + d).clamp(0, 255) as u32) << s;
    }
    out
}

/// `FUN_00006c04(c0, c1, t)`: per channel `c1 + floor((c0 - c1) * t / 256)`, `t` in 0..256; the
/// alpha lane is `c1`'s.
pub fn lerp(c0: u32, c1: u32, t: i32) -> u32 {
    let mut out = c1 & 0xff00_0000;
    for s in [0, 8, 16] {
        let a = ((c0 >> s) & 0xff) as i32;
        let b = ((c1 >> s) & 0xff) as i32;
        out |= ((b + (((a - b) * t) >> 8)).clamp(0, 255) as u32) << s;
    }
    out
}

/// `FUN_000f1ae0`: Delphi `TColor` `0x00BBGGRR` to `0x00RRGGBB`.
pub fn tcolor(c: u32) -> u32 {
    ((c & 0xff00ff) >> 16) + ((c & 0xff00ff) << 16 & 0xff00ff) + (c & 0xff00ff00)
}
