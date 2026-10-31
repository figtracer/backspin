//! QuickFontCache fonts (gfx.md section 5.3): the face at the QuickFont size, the metrics record
//! (`FUN_001408c0` / `FUN_00062c30`), glyph records (`FUN_000623e0`: render, the filters of
//! `FUN_00061050`, the trim of `FUN_000613b0`, the kerning table of `FUN_000628a0`) and the
//! fallback fonts.

use crate::ft;
use crate::raster::{self, Affine, Rasterizer};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The fallback font list `DAT_00241c38` (`FUN_000602e0`): files of the system font folder
/// (`CSIDL_FONTS`) and face indices; only the files that exist are registered.
pub const FALLBACK_FONTS: [(&str, i64); 6] = [
    ("Arial.ttf", 0),
    ("Tahoma.ttf", 0),
    ("PingFang.ttc", 2),
    ("Songti.ttc", 6),
    ("AppleGothic.ttf", 0),
    ("AppleMyungjo.ttf", 0),
];

/// `TQFHinting` to `FT_Load_Glyph` flags.
const LOAD_FLAGS: [i32; 5] = [0x8002, 0x20, 0x10020, 0x8000, 0x30020];

/// The 36-byte font descriptor (`TQuickFont +0x38..+0x5b`), the cache key with the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Desc {
    /// Height in pixels.
    pub height: f32,
    pub aspect: f32,
    pub weight: f32,
    pub weight_aspect: f32,
    /// Shear (radians of the skew).
    pub italic: f32,
    pub sharpness: f32,
    /// `TQFFilter`: 0 none, 1 default, 2 sharper, 3 smoother, 4 greyscale.
    pub filter: u16,
    /// `TQFHinting`: 0 none, 1 default, 2 light, 3 internal, 4 LCD.
    pub hinting: u16,
    /// Descriptor flags (bit 3: no fallback, bit 4: bbox metrics, bit 5: icon font, ...).
    pub flags: u32,
}

impl Default for Desc {
    /// `FUN_00154e20`: 16 px, aspect 1, sharpness 1, default filter and hinting.
    fn default() -> Self {
        Self {
            height: 16.0,
            aspect: 1.0,
            weight: 0.0,
            weight_aspect: 0.0,
            italic: 0.0,
            sharpness: 1.0,
            filter: 1,
            hinting: 1,
            flags: 0,
        }
    }
}

/// A font file and the face index in it.
#[derive(Clone, Debug)]
pub struct Source {
    pub data: Arc<[u8]>,
    pub index: i64,
}

impl Source {
    /// Reads a font file.
    pub fn read(path: &std::path::Path, index: i64) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        Some(Self {
            data: data.into(),
            index,
        })
    }
}

/// One FreeType face with its own library (`FT_New_Memory_Face` + charmap + pixel sizes).
struct Face {
    lib: ft::FtLibrary,
    face: ft::FtFace,
    _data: Arc<[u8]>,
}

// The face and its library are owned by one `Font` and used from one thread at a time.
unsafe impl Send for Face {}

impl Face {
    fn open(src: &Source, height: f32, aspect: f32, scale: f64) -> Option<Self> {
        let mut lib = std::ptr::null_mut();
        if unsafe { ft::FT_Init_FreeType(&mut lib) } != 0 {
            return None;
        }
        let mut face = std::ptr::null_mut();
        let err = unsafe {
            ft::FT_New_Memory_Face(
                lib,
                src.data.as_ptr(),
                src.data.len() as _,
                src.index as _,
                &mut face,
            )
        };
        if err != 0 || face.is_null() {
            unsafe { ft::FT_Done_FreeType(lib) };
            return None;
        }
        let f = Self {
            lib,
            face,
            _data: src.data.clone(),
        };
        // FUN_00140cc0: 'symb' when a (3, 0) charmap exists, else 'unic'.
        let rec = f.rec();
        let maps = if rec.charmaps.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(rec.charmaps, rec.num_charmaps.max(0) as usize) }
        };
        let symbol = maps.iter().any(|&m| {
            let m = unsafe { &*m };
            m.platform_id == 3 && m.encoding_id == 0
        });
        let enc = if symbol {
            ft::ENCODING_MS_SYMBOL
        } else {
            ft::ENCODING_UNICODE
        };
        unsafe { ft::FT_Select_Charmap(face, enc) };
        // FUN_0014df90: m_width = trunc(w * 64), m_height = trunc(h * 64), then >> 6.
        let w = (height as f64 * aspect as f64 * scale * 3.0 * 64.0) as i64;
        let h = (height as f64 * 64.0) as i64;
        unsafe { ft::FT_Set_Pixel_Sizes(face, (w >> 6) as _, (h >> 6) as _) };
        Some(f)
    }

    fn rec(&self) -> &ft::FaceRec {
        unsafe { &*self.face }
    }

    fn slot(&self) -> &ft::GlyphSlotRec {
        unsafe { &*self.rec().glyph }
    }

    fn char_index(&self, code: u32) -> u32 {
        unsafe { ft::FT_Get_Char_Index(self.face, code as _) }
    }

    fn load(&self, index: u32, hinting: u16) -> bool {
        let flags = LOAD_FLAGS.get(hinting as usize).copied().unwrap_or(0x20);
        unsafe { ft::FT_Load_Glyph(self.face, index, flags) == 0 }
    }

    // `FT_Pos` is a C long: 32-bit on Windows.
    #[allow(clippy::useless_conversion)]
    fn kerning(&self, left: u32, right: u32) -> i64 {
        let mut v = ft::Vector::default();
        unsafe { ft::FT_Get_Kerning(self.face, left, right, ft::KERNING_DEFAULT, &mut v) };
        i64::from(v.x)
    }
}

impl Drop for Face {
    fn drop(&mut self) {
        unsafe {
            ft::FT_Done_Face(self.face);
            ft::FT_Done_FreeType(self.lib);
        }
    }
}

/// A glyph record (`FUN_000623e0`): the trimmed, filtered bitmap in rows of `stride = w + 3`
/// bytes with three zero columns on the left (plus four bytes of tail), three columns per pixel.
#[derive(Debug)]
pub struct Glyph {
    pub code: u32,
    pub index: u32,
    /// `None` for a blank glyph.
    pub bits: Option<Vec<u8>>,
    pub stride: usize,
    pub w: i32,
    pub h: i32,
    /// Left bearing in 1/3 px.
    pub bearing: f32,
    pub top: i32,
    /// Advance in 1/3 px.
    pub adv: f64,
    /// Vertical advance (`max(1, Round(descent))`).
    pub vadv: i32,
    /// Kerning with the previous code (`< 0x100`), in 1/3 px; `None` when all zero.
    pub kern: Option<Box<[f32; 256]>>,
    /// The font that produced the glyph (kerning applies within one font only).
    pub font: u32,
}

impl Glyph {
    fn blank(code: u32, index: u32, font: u32) -> Self {
        Self {
            code,
            index,
            bits: None,
            stride: 0,
            w: 0,
            h: 0,
            bearing: 0.0,
            top: 0,
            adv: 0.0,
            vadv: 1,
            kern: None,
            font,
        }
    }
}

/// The metrics record (`font +0x18..`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    /// `+0x18` box height.
    pub h: i32,
    /// `+0x1c`.
    pub ascent: i32,
    /// `+0x20` (a float holding an integer).
    pub lineheight: f64,
    /// `+0x24` x-height, `+0x28` cap height.
    pub xheight: f32,
    pub capheight: f32,
    /// `+0x2c`, `+0x30`: underline position and thickness.
    pub ul_pos: f32,
    pub ul_thick: f32,
    ul_w: i32,
    ul_h: i32,
    ul_top: i32,
    /// `+0x34`: the spacing unit `height * 3 / 15`.
    pub xscale: f32,
    /// `+0x40`: the ellipsis width (two dots), `+0x48`: the soft hyphen advance.
    pub ellipsis_w: f64,
    pub hyphen_w: f64,
}

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// One `TQuickInternalFont`: a face at a descriptor with its glyph cache.
pub struct Font {
    id: u32,
    face: Face,
    pub desc: Desc,
    scale: f64,
    kernel: Option<Vec<f32>>,
    pub metrics: Metrics,
    glyphs: HashMap<u32, Arc<Glyph>>,
    fallbacks: Arc<[(PathBuf, i64)]>,
    fallback: Option<Box<Font>>,
    fallback_failed: bool,
}

impl Font {
    /// Opens `src` at `desc`. `fallbacks` is the fallback font list (`DAT_00241c38`: existing
    /// files only), used for codes the face lacks.
    pub fn new(src: &Source, desc: Desc, fallbacks: Arc<[(PathBuf, i64)]>) -> Option<Self> {
        let scale = if desc.flags & 1 != 0 {
            1.0 / 3.0
        } else if desc.hinting == 4 {
            1.0
        } else {
            32.0
        };
        let face = Face::open(src, desc.height, desc.aspect, scale)?;
        let mut f = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            face,
            desc,
            scale,
            kernel: filter_kernel(desc.filter, desc.sharpness),
            metrics: Metrics::default(),
            glyphs: HashMap::with_capacity(128),
            fallbacks,
            fallback: None,
            fallback_failed: false,
        };
        f.init_metrics();
        Some(f)
    }

    /// `FUN_001408c0` + `FUN_00062c30`.
    fn init_metrics(&mut self) {
        let rec = self.face.rec();
        let size = unsafe { &*rec.size };
        let scale = size.metrics.y_ppem as f64 / rec.units_per_em as f64;
        let mut m = [0.0f64; 7];
        if self.desc.flags & 0x10 != 0 {
            m[0] = (rec.bbox.y_max as f64 * scale).ceil();
            m[1] = f32r(m[0] - (rec.bbox.y_min as f64 * scale).floor());
            m[2] = f32r(size.metrics.height as f64 * 0.015625);
        } else {
            m[0] = (rec.ascender as f64 * scale).ceil();
            m[1] = f32r(m[0] - (rec.descender as f64 * scale).floor());
            m[2] = f32r(rec.height as f64 * scale);
        }
        m[5] = f32r(-(rec.underline_position as f64) * scale);
        m[6] = f32r(rec.underline_thickness as f64 * scale);
        if self.desc.flags & 0x20 != 0 {
            m[3] = f32r(m[0] * 2.0 - m[1]);
            m[4] = m[3];
        } else {
            let idx = self.face.char_index(0x76);
            m[3] = if idx != 0 && self.face.load(idx, self.desc.hinting) {
                f32r(self.face.slot().metrics.hori_bearing_y as f64 * 0.015625)
            } else {
                f32r(m[0] * 2.0 - m[1])
            };
            let idx = self.face.char_index(0x56);
            m[4] = if idx != 0 && self.face.load(idx, self.desc.hinting) {
                f32r(self.face.slot().metrics.hori_bearing_y as f64 * 0.015625)
            } else {
                f32r(m[3] * 1.4285715)
            };
        }
        let ul_thick = m[6] as f32;
        let mt = &mut self.metrics;
        mt.h = m[1].ceil() as i32;
        mt.ascent = rne(m[0]);
        mt.lineheight = rne(m[2]) as f64;
        mt.xheight = m[3] as f32;
        mt.capheight = m[4] as f32;
        mt.ul_pos = if m[5] > 2.0 { m[5] as f32 } else { 2.0 };
        mt.ul_thick = ul_thick;
        mt.ul_w = rne(f32r(f32r(ul_thick as f64 * 3.0) * 0.6666667)).max(1);
        mt.ul_h = rne(f32r(ul_thick as f64 + 0.25)).max(1);
        mt.ul_top = rne(mt.ul_pos as f64);
        mt.xscale = f32r(f32r(self.desc.height as f64 * 3.0) / 15.0) as f32;
        mt.ellipsis_w = 1.0;
        mt.hyphen_w = 1.0;
        if self.desc.flags & 0x8000_0000 == 0 {
            let dot = self.glyph(0x2e).adv;
            self.metrics.ellipsis_w = dot * 2.0;
            self.metrics.hyphen_w = self.glyph(0xad).adv;
        }
    }

    /// `FUN_00062220`: the accelerator underline glyph for a width in 1/3 px.
    pub fn underline(&self, width: i32) -> Glyph {
        let m = &self.metrics;
        let thick = m.ul_w;
        let w = width.max(9);
        let period = thick * 2 + 4;
        let count = ((period + w - 1) / period).max(1);
        let total = period * count - thick;
        let stride = (total + 3) as usize;
        let mut buf = vec![0u8; stride * m.ul_h as usize + 4];
        for y in 0..m.ul_h as usize {
            let mut p = y * stride + 3;
            for _ in 0..count {
                let t = thick as usize;
                buf[p] = 0x48;
                buf[p + 1] = 0x90;
                for k in 0..t {
                    buf[p + 2 + k] = 0xd8;
                }
                buf[p + t + 2] = 0x90;
                buf[p + t + 3] = 0x48;
                p += period as usize;
            }
        }
        Glyph {
            code: u32::MAX,
            index: 0,
            bits: Some(buf),
            stride,
            w: total,
            h: m.ul_h,
            bearing: 0.0,
            top: m.ul_top,
            adv: total as f64,
            vadv: 1,
            kern: None,
            font: self.id,
        }
    }

    /// `FUN_000623e0(font, code, 0xf)`: the cached glyph record.
    pub fn glyph(&mut self, code: u32) -> Arc<Glyph> {
        if let Some(g) = self.glyphs.get(&code) {
            return g.clone();
        }
        let g = self.make_glyph(code, 0);
        self.glyphs.insert(code, g.clone());
        g
    }

    /// Produces a glyph at fallback `depth`. A code the face lacks comes from the next fallback
    /// font with the same descriptor, flags `& !2 | 0x80000000`, unless descriptor flag bit 3.
    fn make_glyph(&mut self, code: u32, depth: usize) -> Arc<Glyph> {
        let index = self.face.char_index(code);
        if index == 0 && self.desc.flags & 8 == 0 && depth < self.fallbacks.len() {
            if self.fallback.is_none() && !self.fallback_failed {
                let (path, idx) = &self.fallbacks[depth];
                let desc = Desc {
                    flags: (self.desc.flags & 0xffff_fffd) | 0x8000_0000,
                    ..self.desc
                };
                self.fallback = Source::read(path, *idx)
                    .and_then(|s| Font::new(&s, desc, self.fallbacks.clone()))
                    .map(Box::new);
                self.fallback_failed = self.fallback.is_none();
            }
            if let Some(fb) = &mut self.fallback {
                return fb.make_glyph(code, depth + 1);
            }
        }
        let mut g = Glyph::blank(code, index, self.id);
        if self.face.load(index, self.desc.hinting) {
            let slot = self.face.slot();
            g.adv = (slot.advance.x as f64 / 64.0) / self.scale;
            let descent = (slot.metrics.height - slot.metrics.hori_bearing_y) as f64 * 0.015625;
            g.vadv = rne(descent).max(1);
            if let Some(mut b) = self.render(slot) {
                if let Some(k) = &self.kernel {
                    b = apply_filter(b, k);
                }
                trim(&mut g, &b);
            }
        }
        if depth == 0 {
            // The fallback request uses 0x19: no kerning table there.
            self.kerning_table(&mut g);
        }
        Arc::new(g)
    }

    /// `FUN_0013fdd0` for the outline renderer: the unfiltered bitmap, three columns per pixel.
    fn render(&self, slot: &ft::GlyphSlotRec) -> Option<Bitmap> {
        let mut path = raster::decompose(&slot.outline)?;
        path = raster::conv_curve(&path, 3.0);
        let m = Affine::IDENTITY
            .multiply(&Affine::scaling(1.0 / self.scale, -1.0))
            .multiply(&Affine::skewing(-(self.desc.italic as f64), 0.0));
        raster::transform(&mut path, &m);
        if self.desc.weight != 0.0 {
            let d = &self.desc;
            let width = f32r(f32r(f32r(d.weight as f64 * d.height as f64) * 3.0) / 15.0);
            let aspect = f32r(d.weight_aspect as f64 / 3.0);
            let join = if d.flags & 0x40 != 0 {
                0
            } else if d.flags & 0x80 != 0 {
                4
            } else {
                3
            };
            path = raster::contour(&path, width, aspect, join);
        }
        let (x1, y1, x2, y2) = raster::bounding_rect(&path).unwrap_or((1.0, 1.0, 0.0, 0.0));
        let bx0 = x1.floor() as i64 - 6;
        let by0 = y1.floor() as i64 - 2;
        let bx1 = x2.ceil() as i64 + 6;
        let by1 = y2.ceil() as i64 + 2;
        let w = ((bx1 - bx0) + 3).div_euclid(3) * 3;
        let h = (by1 - by0) + 1;
        if w <= 0 || h <= 0 || w * h > 1 << 24 {
            return None;
        }
        raster::transform(&mut path, &Affine::translation(-bx0 as f64, -by0 as f64));
        let mut ras = Rasterizer::default();
        ras.add_path(&path);
        Some(Bitmap {
            bits: ras.render_gray8(w as usize, h as usize),
            w: w as usize,
            h: h as usize,
            x0: bx0 as f64,
            y0: by0 as i32 + 1,
        })
    }

    /// `FUN_000628a0`: kerning of every code `< 0x100` before `g`, as float.
    fn kerning_table(&mut self, g: &mut Glyph) {
        if g.index == 0 {
            return;
        }
        let mut table = [0.0f32; 256];
        let mut any = false;
        for code in 0..0x100u32 {
            let idx = match self.glyphs.get(&code) {
                Some(o) => o.index,
                None => self.face.char_index(code),
            };
            if idx == 0 {
                continue;
            }
            let k = self.engine_kerning(idx, g.index);
            if k != 0.0 {
                table[code as usize] = k as f32;
                any = true;
            }
        }
        if any {
            g.kern = Some(Box::new(table));
        }
    }

    /// `FUN_00140740`: `FT_Get_Kerning(first, second).x / 64 / scale`.
    fn engine_kerning(&self, first: u32, second: u32) -> f64 {
        (self.face.kerning(first, second) as f64 / 64.0) / self.scale
    }

    /// `FUN_000629a0(font, glyph, previous glyph)`.
    pub fn kerning(&self, g: &Glyph, prev: &Glyph) -> f64 {
        if g.font != prev.font {
            return 0.0;
        }
        if prev.code < 0x100 {
            return g
                .kern
                .as_ref()
                .map_or(0.0, |k| k[prev.code as usize] as f64);
        }
        if self.desc.flags & 4 == 0 {
            return 0.0;
        }
        self.engine_kerning(prev.index, g.index) as f32 as f64
    }
}

/// A gray8 glyph bitmap and its origin (x in 1/3 px, y = top row + 1).
struct Bitmap {
    bits: Vec<u8>,
    w: usize,
    h: usize,
    x0: f64,
    y0: i32,
}

/// The 1-D kernel of a `TQFFilter` (the second filter record), normalised to sum 1 in float32
/// (`FUN_00060e10`). The radial kernels of filters 5..9 are not ported.
fn filter_kernel(filter: u16, sharpness: f32) -> Option<Vec<f32>> {
    let s = sharpness.abs();
    let k: Vec<f32> = match filter {
        0 | 5..=9 => return None,
        2 => vec![0.6666667, s, 0.6666667],
        3 => vec![0.33333334, 0.6666667, s, 0.6666667, 0.33333334],
        4 => vec![0.14285715, s, 0.14285715],
        _ => vec![0.14285715, 0.5, s, 0.5, 0.14285715],
    };
    let mut sum = 0.0f32;
    for &v in &k {
        sum += v;
    }
    if sum <= 5.9604645e-08 {
        sum = 5.9604645e-08;
    }
    let m = 1.0f32 / sum;
    Some(k.iter().map(|&v| v * m).collect())
}

/// `FUN_00061050` with a `kw x 1` kernel through `ippiFilter_Round32f_8u_C1R`: int16 taps
/// `trunc(k * 32767)` and `vImageConvolve_Planar8` (divisor 32767, edge extend) on the valid
/// region; the output loses `kw - 1` columns and the origin moves by `kw >> 1`.
fn apply_filter(b: Bitmap, kernel: &[f32]) -> Bitmap {
    let kw = kernel.len();
    if b.w < kw || b.h == 0 {
        return b;
    }
    let w2 = b.w - kw + 1;
    let q: Vec<i64> = kernel.iter().map(|&v| (v * 32767.0) as i64).collect();
    let r = kw >> 1;
    let mut out = vec![0u8; w2 * b.h];
    for y in 0..b.h {
        let roi = &b.bits[y * b.w + r..y * b.w + r + w2];
        for x in 0..w2 {
            let mut acc = 0i64;
            for (i, &qi) in q.iter().enumerate() {
                let j = (x + kw - 1 - i) as isize - r as isize;
                let s = roi[j.clamp(0, w2 as isize - 1) as usize] as i64;
                acc += s * qi;
            }
            out[y * w2 + x] = (acc + 16383).div_euclid(32767).clamp(0, 255) as u8;
        }
    }
    Bitmap {
        bits: out,
        w: w2,
        h: b.h,
        x0: b.x0 + r as f64,
        y0: b.y0,
    }
}

/// `FUN_000613b0`: crops to the non-zero box with three zero columns of left padding.
fn trim(g: &mut Glyph, b: &Bitmap) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..b.h {
        let row = &b.bits[y * b.w..(y + 1) * b.w];
        if let (Some(a), Some(z)) = (
            row.iter().position(|&v| v != 0),
            row.iter().rposition(|&v| v != 0),
        ) {
            minx = minx.min(a);
            maxx = maxx.max(z);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    if minx == usize::MAX {
        g.bits = None;
        g.bearing = 0.0;
        g.top = 0;
        return;
    }
    let (w, h) = (maxx - minx + 1, maxy - miny + 1);
    let stride = w + 3;
    let mut buf = vec![0u8; stride * h + 4];
    for y in 0..h {
        let src = (y + miny) * b.w + minx;
        buf[y * stride + 3..y * stride + 3 + w].copy_from_slice(&b.bits[src..src + w]);
    }
    g.w = w as i32;
    g.h = h as i32;
    g.stride = stride;
    g.bits = Some(buf);
    g.bearing = (minx as f64 + b.x0) as f32;
    g.top = miny as i32 + b.y0;
}

/// One float32 rounding, widened back.
pub(crate) fn f32r(x: f64) -> f64 {
    x as f32 as f64
}

/// FPC `Round`: half to even.
pub(crate) fn rne(x: f64) -> i32 {
    x.round_ties_even() as i32
}
