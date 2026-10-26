//! Compares the port with FL's own libblend2d.dylib (Agg2D through its vtable) where it is
//! installed. Skipped otherwise. macOS only: it loads the library with `dlopen`.
#![cfg(target_os = "macos")]

use backspin_agg2d::Agg2D;
use std::ffi::c_void;
/// The float32 pi the plugin passes (DAT_003545f0).
const PI: f64 = std::f32::consts::PI as f64;

const FL_BLEND2D: &str =
    "/Applications/FL Studio 2026.app/Contents/Resources/FL/Shared/libblend2d.dylib";

unsafe extern "C" {
    fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
    fn dlsym(h: *mut c_void, name: *const i8) -> *mut c_void;
}

struct Original(*mut c_void);

impl Original {
    fn new() -> Option<Self> {
        unsafe {
            let p = std::ffi::CString::new(FL_BLEND2D).unwrap();
            let h = dlopen(p.as_ptr(), 2);
            if h.is_null() {
                return None;
            }
            let f = dlsym(h, c"AggCreateInstance".as_ptr());
            if f.is_null() {
                return None;
            }
            let f: extern "C" fn() -> *mut c_void = std::mem::transmute(f);
            Some(Self(f()))
        }
    }

    fn slot(&self, off: usize) -> usize {
        unsafe { *(*(self.0 as *const *const usize)).add(off / 8) }
    }

    fn reset_path(&self) {
        let f: extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(self.slot(0x290)) };
        f(self.0)
    }

    fn move_to(&self, x: f64, y: f64) {
        let f: extern "C" fn(*mut c_void, f64, f64) =
            unsafe { std::mem::transmute(self.slot(0x298)) };
        f(self.0, x, y)
    }

    fn line_to(&self, x: f64, y: f64) {
        let f: extern "C" fn(*mut c_void, f64, f64) =
            unsafe { std::mem::transmute(self.slot(0x2a8)) };
        f(self.0, x, y)
    }

    #[allow(clippy::too_many_arguments)]
    fn arc_to(&self, rx: f64, ry: f64, a: f64, large: i32, sweep: i32, x: f64, y: f64) {
        let f: extern "C" fn(*mut c_void, f64, f64, f64, i32, i32, f64, f64) =
            unsafe { std::mem::transmute(self.slot(0x2d8)) };
        f(self.0, rx, ry, a, large, sweep, x, y)
    }

    fn attach(&self, bits: *mut u8, w: u32, h: u32, stride: i32) {
        let f: extern "C" fn(*mut c_void, *mut u8, u32, u32, i32) =
            unsafe { std::mem::transmute(self.slot(0x8)) };
        f(self.0, bits, w, h, stride)
    }

    fn fill_color(&self, c: i32) {
        let f: extern "C" fn(*mut c_void, i32) = unsafe { std::mem::transmute(self.slot(0xd0)) };
        f(self.0, c)
    }

    fn draw_path(&self, m: u32) {
        let f: extern "C" fn(*mut c_void, u32) = unsafe { std::mem::transmute(self.slot(0x338)) };
        f(self.0, m)
    }

    fn line_width(&self, w: f64) {
        let f: extern "C" fn(*mut c_void, f64) = unsafe { std::mem::transmute(self.slot(0x150)) };
        f(self.0, w)
    }

    fn line_color(&self, c: i32) {
        let f: extern "C" fn(*mut c_void, i32) = unsafe { std::mem::transmute(self.slot(0xe8)) };
        f(self.0, c)
    }

    fn linear_gradient(&self, x1: f64, y1: f64, x2: f64, y2: f64, slot: u32) {
        let f: extern "C" fn(*mut c_void, f64, f64, f64, f64, u32) =
            unsafe { std::mem::transmute(self.slot(0x3c8)) };
        f(self.0, x1, y1, x2, y2, slot)
    }

    fn gradient_buffer(&self, slot: u32) -> *mut [i16; 4] {
        let f: extern "C" fn(*mut c_void, u32) -> *mut [i16; 4] =
            unsafe { std::mem::transmute(self.slot(0x3c0)) };
        f(self.0, slot)
    }

    /// The working path at `this + 0x140`: BLPathImpl { commandData, vertexData, size, ... }.
    fn path_vertices(&self) -> Vec<(u8, f64, f64)> {
        unsafe {
            let imp = *((self.0 as *const u8).add(0x140) as *const *const usize);
            let (c, v, n) = (*imp as *const u8, *imp.add(1) as *const f64, *imp.add(2));
            (0..n)
                .map(|i| (*c.add(i), *v.add(2 * i), *v.add(2 * i + 1)))
                .collect()
        }
    }
}

#[test]
fn pill_path_matches_the_original() {
    let Some(o) = Original::new() else {
        eprintln!("FL's libblend2d.dylib not installed; skipped");
        return;
    };
    let mut p = Agg2D::new();
    o.reset_path();
    p.reset_path();
    o.move_to(36.0, 14.0);
    p.move_to(36.0, 14.0);
    o.line_to(118.0, 14.0);
    p.line_to(118.0, 14.0);
    o.arc_to(14.0, 14.0, PI, 0, 1, 118.0, 45.0);
    p.arc_to(14.0, 14.0, PI, false, true, 118.0, 45.0);
    let (a, b) = (o.path_vertices(), p.path_vertices());
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        if x != y {
            eprintln!("vertex {i}: original {x:?} port {y:?}");
        }
    }
    assert_eq!(a, b);
}

/// Draws the pill with both canvases and returns the differing pixels.
fn pill_pixels(o: &Original, solid_bg: u32) -> Vec<(usize, u32, u32)> {
    let (w, h) = (160usize, 64usize);
    let mut a = vec![solid_bg; w * h];
    let mut b = vec![solid_bg; w * h];
    o.attach(
        a.as_mut_ptr() as *mut u8,
        w as u32,
        h as u32,
        (w * 4) as i32,
    );
    let mut p = Agg2D::new();
    unsafe {
        p.attach(
            b.as_mut_ptr() as *mut u8,
            w as u32,
            h as u32,
            (w * 4) as i32,
        )
    };
    o.reset_path();
    p.reset_path();
    o.fill_color(0xff3a352cu32 as i32);
    p.fill_color(0xff3a352c);
    o.move_to(36.0, 14.0);
    p.move_to(36.0, 14.0);
    o.line_to(118.0, 14.0);
    p.line_to(118.0, 14.0);
    o.arc_to(14.0, 14.0, PI, 0, 1, 118.0, 45.0);
    p.arc_to(14.0, 14.0, PI, false, true, 118.0, 45.0);
    o.line_to(36.0, 45.0);
    p.line_to(36.0, 45.0);
    o.line_to(36.0, 15.0);
    p.line_to(36.0, 15.0);
    o.draw_path(0);
    p.draw_path(backspin_agg2d::DrawPath::Fill);
    p.detach();
    a.iter()
        .zip(&b)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, (x, y))| (i, *x, *y))
        .collect()
}

#[test]
fn pill_pixels_match_the_original() {
    let Some(o) = Original::new() else { return };
    let d = pill_pixels(&o, 0xff60686e);
    for (i, x, y) in d.iter().take(10) {
        eprintln!(
            "pixel ({}, {}): original {x:08x} port {y:08x}",
            i % 160,
            i / 160
        );
    }
    assert!(d.is_empty(), "{} pixels differ", d.len());
}

/// The top background of the editor (gfx.md 8.1) with both canvases; returns differing pixels.
fn top_bg(o: &Original) -> Vec<(usize, u32, u32)> {
    let (w, h) = (266usize, 91usize);
    let mut a = vec![0xff171f24u32; w * h];
    let mut b = a.clone();
    o.attach(
        a.as_mut_ptr() as *mut u8,
        w as u32,
        h as u32,
        (w * 4) as i32,
    );
    let mut p = Agg2D::new();
    unsafe {
        p.attach(
            b.as_mut_ptr() as *mut u8,
            w as u32,
            h as u32,
            (w * 4) as i32,
        )
    };
    let lut: Vec<[i16; 4]> = (0..256)
        .map(|i| {
            let t = (1.0f32 / 255.0) * i as f32;
            let base = [0x66, 0x6e, 0x74, 0xff].map(|c| c as f32 * 128.0);
            let mut out = [0i16; 4];
            for k in 0..4 {
                let (bb, d) = if k < 3 {
                    (base[k] + 72.0 * 128.0, -120.0 * 128.0)
                } else {
                    (base[k], 0.0)
                };
                out[k] = (bb + d * t).round_ties_even() as i16;
            }
            out
        })
        .collect();
    o.reset_path();
    p.reset_path();
    o.line_width(1.0);
    p.line_width(1.0);
    o.line_color(0xff323a40u32 as i32);
    p.line_color(0xff323a40);
    o.fill_color(0xff746e66u32 as i32);
    p.fill_color(0xff746e66);
    let pts = [
        (0.0, 0.0),
        (0.0, 61.0),
        (39.0, 61.0),
        (60.0, 53.0),
        (176.0, 53.0),
    ];
    o.move_to(0.0, 0.0);
    p.move_to(0.0, 0.0);
    for &(x, y) in &pts[1..] {
        o.line_to(x, y);
        p.line_to(x, y);
    }
    o.arc_to(5.0, 5.0, PI * 0.5, 0, 1, 181.0, 58.0);
    p.arc_to(5.0, 5.0, PI * 0.5, false, true, 181.0, 58.0);
    o.line_to(181.0, 85.0);
    p.line_to(181.0, 85.0);
    o.arc_to(5.0, 5.0, PI * 0.5, 0, 0, 186.0, 90.0);
    p.arc_to(5.0, 5.0, PI * 0.5, false, false, 186.0, 90.0);
    for &(x, y) in &[(267.0, 90.0), (267.0, 0.0), (0.0, 0.0)] {
        o.line_to(x, y);
        p.line_to(x, y);
    }
    o.linear_gradient(133.0, 0.0, 133.0, 90.0, 1);
    p.linear_gradient(133.0, 0.0, 133.0, 90.0, 1);
    let ob = o.gradient_buffer(1);
    for (i, c) in lut.iter().enumerate() {
        unsafe { *ob.add(i) = *c };
    }
    p.gradient_color_buffer(1).copy_from_slice(&lut);
    o.draw_path(2);
    p.draw_path(backspin_agg2d::DrawPath::FillAndStroke);
    {
        o.reset_path();
        p.reset_path();
        o.fill_color(0xff3a352cu32 as i32);
        p.fill_color(0xff3a352c);
        o.move_to(36.0, 14.0);
        p.move_to(36.0, 14.0);
        o.line_to(118.0, 14.0);
        p.line_to(118.0, 14.0);
        o.arc_to(14.0, 14.0, PI, 0, 1, 118.0, 45.0);
        p.arc_to(14.0, 14.0, PI, false, true, 118.0, 45.0);
        o.line_to(36.0, 45.0);
        p.line_to(36.0, 45.0);
        o.line_to(36.0, 15.0);
        p.line_to(36.0, 15.0);
        o.draw_path(0);
        p.draw_path(backspin_agg2d::DrawPath::Fill);
    }
    p.detach();
    a.iter()
        .zip(&b)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, (x, y))| (i, *x, *y))
        .collect()
}

#[test]
fn top_background_matches_the_original() {
    let Some(o) = Original::new() else { return };
    let d = top_bg(&o);
    for (i, x, y) in d.iter().take(10) {
        eprintln!(
            "pixel ({}, {}): original {x:08x} port {y:08x}",
            i % 266,
            i / 266
        );
    }
    assert!(d.is_empty(), "{} pixels differ", d.len());
}
