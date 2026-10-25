//! Image-Line's `Agg2D` vector canvas over Blend2D 0.11.1, ported from `FL/Shared/libblend2d.dylib`
//! (gfx.md section 4). The Gross Beat editor draws every anti-aliased shape through it.
//!
//! Colours in `fill_color`/`line_color` are `0xAABBGGRR` (Delphi order), as the plugin passes them.
//! Agg2D gradient slot 1 is the fill, slot 0 the stroke.

use std::os::raw::{c_double, c_int};

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn agg_new() -> *mut Raw;
    fn agg_free(a: *mut Raw);
    fn agg_attach(a: *mut Raw, bits: *mut u8, w: u32, h: u32, stride: i32);
    fn agg_detach(a: *mut Raw);
    fn agg_fill_color(a: *mut Raw, c: i32);
    fn agg_fill_color_sep(a: *mut Raw, r: u8, g: u8, b: u8, al: u8);
    fn agg_no_fill(a: *mut Raw);
    fn agg_line_color(a: *mut Raw, c: i32);
    fn agg_line_color_sep(a: *mut Raw, r: u8, g: u8, b: u8, al: u8);
    fn agg_no_line(a: *mut Raw);
    fn agg_line_width(a: *mut Raw, w: c_double);
    fn agg_get_line_width(a: *const Raw) -> c_double;
    fn agg_line_cap(a: *mut Raw, cap: c_int);
    fn agg_line_join(a: *mut Raw, join: c_int);
    fn agg_fill_even_odd(a: *mut Raw, even_odd: c_int);
    fn agg_reset_transformations(a: *mut Raw);
    fn agg_translate(a: *mut Raw, dx: c_double, dy: c_double);
    fn agg_reset_path(a: *mut Raw);
    fn agg_move_to(a: *mut Raw, x: c_double, y: c_double);
    fn agg_line_to(a: *mut Raw, x: c_double, y: c_double);
    fn agg_arc_to(
        a: *mut Raw,
        rx: c_double,
        ry: c_double,
        angle: c_double,
        large: c_int,
        sweep: c_int,
        x: c_double,
        y: c_double,
    );
    fn agg_quadric_curve_to(a: *mut Raw, cx: c_double, cy: c_double, x: c_double, y: c_double);
    fn agg_cubic_curve_to(
        a: *mut Raw,
        c1x: c_double,
        c1y: c_double,
        c2x: c_double,
        c2y: c_double,
        x: c_double,
        y: c_double,
    );
    fn agg_close_polygon(a: *mut Raw);
    fn agg_draw_path(a: *mut Raw, flag: u32);
    fn agg_gradient_color_buffer(a: *mut Raw, slot: u32) -> *mut i16;
    fn agg_linear_gradient(
        a: *mut Raw,
        x1: c_double,
        y1: c_double,
        x2: c_double,
        y2: c_double,
        slot: u32,
    );
    fn agg_radial_gradient_ex(
        a: *mut Raw,
        cx: c_double,
        cy: c_double,
        r: c_double,
        angle: c_double,
        aspect: c_double,
        slot: u32,
    );
    fn agg_clip_box_int(a: *mut Raw, x0: c_int, y0: c_int, x1: c_int, y1: c_int);
    fn agg_line_width_ex(a: *mut Raw, w: c_double, offset: c_double);
    fn agg_path_view(a: *const Raw, cmds: *mut *const u8, verts: *mut *const f64) -> usize;
}

/// `drawPath` modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum DrawPath {
    Fill = 0,
    Stroke = 1,
    FillAndStroke = 2,
    FillWithLineColor = 3,
    StrokeWithFillColor = 4,
    StrokeThenFill = 5,
}

/// An Agg2D instance. `attach` binds it to 32 bpp premultiplied pixels the caller owns; the caller
/// must `detach` (or drop) before that memory goes away.
pub struct Agg2D {
    raw: *mut Raw,
    attached: bool,
}

// The instance owns its Blend2D objects; it is used from one thread at a time.
unsafe impl Send for Agg2D {}

impl Default for Agg2D {
    fn default() -> Self {
        Self::new()
    }
}

impl Agg2D {
    /// `AggCreateInstance`.
    pub fn new() -> Self {
        Self {
            raw: unsafe { agg_new() },
            attached: false,
        }
    }

    /// Binds the canvas to `h` rows of `stride` bytes at `bits` (`BL_FORMAT_PRGB32`).
    ///
    /// # Safety
    /// `bits` must stay valid for `h * stride` bytes until `detach` or drop.
    pub unsafe fn attach(&mut self, bits: *mut u8, w: u32, h: u32, stride: i32) {
        if self.attached {
            self.detach();
        }
        unsafe { agg_attach(self.raw, bits, w, h, stride) };
        self.attached = true;
    }

    pub fn detach(&mut self) {
        if self.attached {
            unsafe { agg_detach(self.raw) };
            self.attached = false;
        }
    }

    pub fn fill_color(&mut self, c: u32) {
        unsafe { agg_fill_color(self.raw, c as i32) }
    }

    pub fn fill_color_sep(&mut self, r: u8, g: u8, b: u8, a: u8) {
        unsafe { agg_fill_color_sep(self.raw, r, g, b, a) }
    }

    pub fn no_fill(&mut self) {
        unsafe { agg_no_fill(self.raw) }
    }

    pub fn line_color(&mut self, c: u32) {
        unsafe { agg_line_color(self.raw, c as i32) }
    }

    pub fn line_color_sep(&mut self, r: u8, g: u8, b: u8, a: u8) {
        unsafe { agg_line_color_sep(self.raw, r, g, b, a) }
    }

    pub fn no_line(&mut self) {
        unsafe { agg_no_line(self.raw) }
    }

    pub fn line_width(&mut self, w: f64) {
        unsafe { agg_line_width(self.raw, w) }
    }

    pub fn get_line_width(&self) -> f64 {
        unsafe { agg_get_line_width(self.raw) }
    }

    pub fn line_cap(&mut self, cap: i32) {
        unsafe { agg_line_cap(self.raw, cap) }
    }

    pub fn line_join(&mut self, join: i32) {
        unsafe { agg_line_join(self.raw, join) }
    }

    pub fn fill_even_odd(&mut self, even_odd: bool) {
        unsafe { agg_fill_even_odd(self.raw, even_odd as c_int) }
    }

    pub fn reset_transformations(&mut self) {
        unsafe { agg_reset_transformations(self.raw) }
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        unsafe { agg_translate(self.raw, dx, dy) }
    }

    pub fn reset_path(&mut self) {
        unsafe { agg_reset_path(self.raw) }
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        unsafe { agg_move_to(self.raw, x, y) }
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        unsafe { agg_line_to(self.raw, x, y) }
    }

    /// SVG elliptic arc to `(x, y)`.
    #[allow(clippy::too_many_arguments)]
    pub fn arc_to(
        &mut self,
        rx: f64,
        ry: f64,
        angle: f64,
        large_arc: bool,
        sweep: bool,
        x: f64,
        y: f64,
    ) {
        unsafe {
            agg_arc_to(
                self.raw,
                rx,
                ry,
                angle,
                large_arc as c_int,
                sweep as c_int,
                x,
                y,
            )
        }
    }

    pub fn quadric_curve_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        unsafe { agg_quadric_curve_to(self.raw, cx, cy, x, y) }
    }

    pub fn cubic_curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        unsafe { agg_cubic_curve_to(self.raw, c1x, c1y, c2x, c2y, x, y) }
    }

    pub fn close_polygon(&mut self) {
        unsafe { agg_close_polygon(self.raw) }
    }

    pub fn draw_path(&mut self, mode: DrawPath) {
        unsafe { agg_draw_path(self.raw, mode as u32) }
    }

    /// The 256 x (R, G, B, A) gradient buffer of a slot (1 = fill, 0 = stroke) in 1/128 units;
    /// marks it dirty.
    pub fn gradient_color_buffer(&mut self, slot: u32) -> &mut [[i16; 4]; 256] {
        let p = unsafe { agg_gradient_color_buffer(self.raw, slot) };
        assert!(!p.is_null(), "gradient slot {slot}");
        unsafe { &mut *(p as *mut [[i16; 4]; 256]) }
    }

    pub fn linear_gradient(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, slot: u32) {
        unsafe { agg_linear_gradient(self.raw, x1, y1, x2, y2, slot) }
    }

    pub fn radial_gradient_ex(
        &mut self,
        cx: f64,
        cy: f64,
        r: f64,
        angle: f64,
        aspect: f64,
        slot: u32,
    ) {
        unsafe { agg_radial_gradient_ex(self.raw, cx, cy, r, angle, aspect, slot) }
    }

    pub fn clip_box_int(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        unsafe { agg_clip_box_int(self.raw, x0, y0, x1, y1) }
    }

    /// The custom stroker: an outline from `offset` outside to `w` inside the path.
    pub fn line_width_ex(&mut self, w: f64, offset: f64) {
        unsafe { agg_line_width_ex(self.raw, w, offset) }
    }
}

impl Agg2D {
    /// The working path as (command, x, y) triples.
    pub fn path_vertices(&self) -> Vec<(u8, f64, f64)> {
        let (mut c, mut v) = (std::ptr::null(), std::ptr::null());
        let n = unsafe { agg_path_view(self.raw, &mut c, &mut v) };
        (0..n)
            .map(|i| unsafe { (*c.add(i), *v.add(2 * i), *v.add(2 * i + 1)) })
            .collect()
    }
}

impl Drop for Agg2D {
    fn drop(&mut self) {
        self.detach();
        unsafe { agg_free(self.raw) };
    }
}
