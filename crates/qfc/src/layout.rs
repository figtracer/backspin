//! The text call of QuickFontCache (`FUN_00063ac0`, gfx.md sections 5.2 to 5.5): tokenising,
//! layout with kerning, spacing, snapping, the end ellipsis, justification and alignment (the
//! line helpers `FUN_00066150`, `FUN_00066040`, `FUN_000666e0`), vertical placement, the
//! subpixel compositing `FUN_00061790` / `FUN_00004638` with the gamma LUTs, and measurement.
//!
//! Word and character wrapping, the middle ellipsis modes, palette codes and character
//! positions are not ported (the editor never requests them); such requests draw without them.

use crate::font::{Font, Glyph, f32r, rne};
use std::sync::{Arc, OnceLock};

/// The gamma LUTs of `FUN_0005fe70` (contrast 1.4): `alpha[i] = Round((i/255)^1.4 * 4095)`
/// forced `>= i` for `i = 0..=0x110`, and `inv[j] = Round((j/4095)^(1/1.4) * 255)` padded with
/// `INV_BASE` clamped entries on both sides.
struct Luts {
    alpha: [i32; 0x111],
    inv: Vec<u8>,
}

const INV_BASE: i32 = 0x2aa;

fn luts() -> &'static Luts {
    static L: OnceLock<Luts> = OnceLock::new();
    L.get_or_init(|| {
        let gamma = 1.4f64;
        let mut alpha = [0; 0x111];
        for (i, a) in alpha.iter_mut().enumerate() {
            *a = rne((i as f64 / 255.0).powf(gamma) * 4095.0).max(i as i32);
        }
        let mut inv = vec![0u8; INV_BASE as usize];
        inv.extend((0..4096).map(|j| rne((j as f64 / 4095.0).powf(1.0 / gamma) * 255.0) as u8));
        inv.extend(std::iter::repeat_n(255u8, INV_BASE as usize));
        Luts { alpha, inv }
    })
}

/// The destination window: 32 bpp pixels (`0xAARRGGBB`), `stride` pixels per row, and the
/// window rect `(x, y, w, h)` in pixels of `px` that the text may touch.
pub struct Target<'a> {
    pub px: &'a mut [u32],
    pub stride: usize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The request record (`FUN_001819f0`): the rect in doubles relative to the window, the text,
/// flags, colour `0xRRGGBB`, alpha (`0x100` opaque, up to `0x110`) and the font's spacings.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub rect: [f64; 4],
    pub text: &'a [u16],
    pub flags: u32,
    pub colour: u32,
    pub alpha: i32,
    pub xspacing: f64,
    pub yspacing: f64,
    pub max_xspacing: f64,
}

/// One laid out character.
#[derive(Clone, Debug, Default)]
pub struct Item {
    code: u32,
    glyph: Option<Arc<Glyph>>,
    x: f64,
    y: i32,
    flag: u8,
    lut_a: i32,
}

fn item_end(it: &Item) -> f64 {
    it.x + it.glyph.as_ref().map_or(0.0, |g| g.adv)
}

/// Draws (or measures) `req` with `font` into `t`. `items` is scratch space kept by the caller.
/// Returns the rect, replaced by the measured box when the flags ask for it.
pub fn draw(font: &mut Font, items: &mut Vec<Item>, t: &mut Target, req: &Request) -> [f64; 4] {
    items.clear();
    let mut l = Layout {
        f: font,
        items,
        req,
        rect: req.rect,
        flags: req.flags,
        lineh: 0,
        x0_3: 0.0,
        maxw: 0.0,
        xsp_extra: 0.0,
        last_y: 0,
        alpha: 0,
        count: 0,
        cur: 0,
        line_start: 0,
        prev: None,
        x: 0.0,
        y: 0,
        first_y: 0,
        last_flag: false,
        dy: 0,
    };
    l.run(t);
    l.rect
}

struct Layout<'a, 'b> {
    f: &'a mut Font,
    items: &'a mut Vec<Item>,
    req: &'a Request<'b>,
    rect: [f64; 4],
    flags: u32,
    lineh: i32,
    x0_3: f64,
    maxw: f64,
    xsp_extra: f64,
    last_y: i32,
    alpha: i32,
    count: usize,
    cur: usize,
    line_start: usize,
    prev: Option<Arc<Glyph>>,
    x: f64,
    y: i32,
    first_y: i32,
    last_flag: bool,
    dy: i32,
}

impl Layout<'_, '_> {
    fn run(&mut self, t: &mut Target) {
        let m = self.f.metrics;
        let r = self.req;
        let flags = r.flags;
        self.lineh = rne(m.lineheight * r.yspacing);
        let s = r.text;
        let n = s.len();
        if n < 1 {
            if flags & 0x300000 != 0 {
                self.rect[2] = self.rect[0];
                self.rect[3] = self.rect[1];
            }
            return;
        }
        let snap = flags & 0xc00000;
        let xsp = r.xspacing * m.xscale as f64;
        self.xsp_extra = (r.max_xspacing - r.xspacing) * m.xscale as f64;
        self.x0_3 = r.rect[0] * 3.0;
        let mut y = rne(r.rect[1]);
        self.last_y = (m.ascent + (rne(r.rect[3]) - m.h) - 1) - self.lineh;
        if flags & 0x38 != 8 {
            y += m.ascent - 1;
        }
        if flags & 0x1c0 < 0xc0 {
            self.maxw = r.rect[2] * 3.0 - self.x0_3;
        } else {
            self.flags = flags & 0xffff_fe3f;
            self.maxw = 2147483647.0;
        }
        let flags = self.flags;
        let lut = &luts().alpha;
        self.alpha = r.alpha.clamp(0, 0x110);
        let mut lut_a = lut[self.alpha as usize];
        let mut i = 0;
        while i < n {
            let mut it = Item {
                code: s[i] as u32,
                ..Default::default()
            };
            if it.code < 0x27 {
                if it.code == 0x26 && flags & 0x8000 == 0 {
                    i += 1;
                    if i >= n {
                        break;
                    }
                    it.code = s[i] as u32;
                    if it.code != 0x26 {
                        it.flag = 1;
                    }
                } else if it.code == 9 {
                    it.code = 0x2001;
                } else if it.code < 0x20 && it.code != 0xd && it.code != 2 {
                    i += 1;
                    continue;
                }
            } else if it.code >= 0xd800 {
                if it.code >> 10 == 0x36 {
                    i += 1;
                    if i >= n {
                        break;
                    }
                    let lo = s[i] as u32;
                    if lo >> 10 == 0x37 {
                        it.code = (lo ^ 0xdc00) + (it.code - 0xd7c0) * 0x400;
                    } else {
                        continue;
                    }
                } else if it.code & 0xff00 == 0xe400 {
                    // Palette colour codes are not ported: the request colour stays.
                    i += 1;
                    continue;
                } else if it.code & 0xff00 == 0xe500 {
                    lut_a = lut[((self.alpha * ((it.code & 0xff) as i32 + 1)) >> 8) as usize];
                    i += 1;
                    continue;
                }
            }
            it.lut_a = lut_a;
            self.items.push(it);
            i += 1;
        }
        self.count = self.items.len();
        self.cur = 0;
        self.line_start = 0;
        self.prev = None;
        self.x = 0.0;
        self.y = y;
        self.first_y = y;
        self.last_flag = !(y < self.last_y && flags & 0xc00 != 0);
        while self.cur < self.count {
            let code = self.items[self.cur].code;
            if code < 0xe {
                let it = &mut self.items[self.cur];
                it.glyph = None;
                it.x = self.x;
                it.y = self.y;
                if code == 2 {
                    self.x = 0.0;
                } else {
                    self.last_flag = self.last_y <= self.y;
                    self.line_end(self.last_flag as u32 | 2);
                    self.new_line();
                    self.line_start += 1;
                }
                self.cur += 1;
                continue;
            }
            let g = self.f.glyph(code);
            self.items[self.cur].glyph = Some(g.clone());
            if self.cur == self.line_start && flags & 0x2000000 != 0 {
                self.x -= (g.bearing as f64).min(0.0);
            }
            if let Some(p) = &self.prev
                && flags & 0x40000000 == 0
            {
                self.x += self.f.kerning(&g, p);
            }
            if snap != 0 {
                self.x = if snap == 0x400000 {
                    (rne(self.x * 0.3333333333333333) * 3) as f64
                } else {
                    rne(self.x) as f64
                };
            }
            let mut fits = self.x + g.adv <= self.maxw;
            if !fits
                && code == 0x20
                && self.line_start < self.cur
                && self.items[self.cur - 1].code != 0x20
                && flags & 0x4000000 == 0
            {
                fits = true;
            }
            if fits {
                self.place(g, xsp);
                continue;
            }
            let mode = flags & 0xc00;
            let last2 = self.last_flag;
            if flags & 0x1c0 == 0 || !self.last_flag {
                if self.x <= 0.0 {
                    self.place(g, xsp);
                    continue;
                }
            } else {
                if self.maxw < m.ellipsis_w {
                    self.count = self.line_start;
                    break;
                }
                let mut j = self.cur;
                while self.line_start < j {
                    if item_end(&self.items[j - 1]) + m.ellipsis_w <= self.maxw {
                        break;
                    }
                    j -= 1;
                }
                self.cur = j;
                if flags & 0x1c0 != 0x80 {
                    self.line_end(self.last_flag as u32);
                    self.new_line();
                }
                // Wrapping (0x80 with 0x400/0x800) is not ported: the line ends here.
                self.count = self.cur;
                break;
            }
            if last2 || mode != 0 {
                // Without wrapping (not ported) the text stops at the overflow.
                self.count = self.cur;
                break;
            }
            self.place(g, xsp);
        }
        self.finish(t);
    }

    fn place(&mut self, g: Arc<Glyph>, xsp: f64) {
        let m = self.f.metrics;
        let it = &mut self.items[self.cur];
        it.x = self.x;
        it.y = self.y;
        if self.flags & 0x38 == 0x28 {
            it.y += ((m.h - g.h) >> 1) - g.top - m.ascent + 1;
        }
        self.x = self.x + g.adv + xsp;
        self.prev = Some(g);
        self.cur += 1;
    }

    /// `FUN_00066040`.
    fn new_line(&mut self) {
        self.x = 0.0;
        self.y += self.lineh;
        self.last_flag = self.last_y <= self.y;
        self.prev = None;
        if self.flags & 0x4000000 == 0 && self.cur < self.count && self.items[self.cur].code == 0x20
        {
            self.items[self.cur].glyph = None;
            self.cur += 1;
        }
        self.line_start = self.cur;
    }

    /// `FUN_000666e0`: two '.' items at `j`.
    fn add_ellipsis(&mut self, mut j: usize) -> usize {
        let mut w = if self.line_start < j {
            item_end(&self.items[j - 1])
        } else {
            0.0
        };
        let lut_a = luts().alpha[self.alpha as usize];
        for _ in 0..2 {
            let g = self.f.glyph(0x2e);
            let adv = g.adv;
            let it = Item {
                code: 0x2e,
                glyph: Some(g),
                x: w,
                y: self.y,
                flag: 0,
                lut_a,
            };
            w += adv;
            if j < self.items.len() {
                self.items[j] = it;
            } else {
                self.items.push(it);
            }
            j += 1;
        }
        j
    }

    /// `FUN_00066150`: drops a trailing space, adds the ellipsis, justifies and aligns the line.
    fn line_end(&mut self, p: u32) {
        let flags = self.flags;
        let mut j = self.cur;
        if flags & 0x4000000 == 0 && self.line_start < j && self.items[j - 1].code == 0x20 {
            self.items[j - 1].glyph = None;
            j -= 1;
        }
        if p & 1 != 0 && flags & 0x1c0 != 0 {
            j = self.add_ellipsis(j);
            self.cur = j;
            self.count = j;
        }
        if j <= self.line_start {
            let al = flags & 3;
            if al == 0 {
                return;
            }
            self.x = self.maxw;
            if al == 2 {
                self.x *= 0.5;
            }
            return;
        }
        let js = flags & 0x6000;
        if js != 0 && !(p & 2 != 0 && js < 0x2001) {
            let w = item_end(&self.items[j - 1]);
            let cnt = j - self.line_start;
            if self.maxw > w && cnt >= 2 {
                let mut d = (self.maxw - w) / (cnt - 1) as f64;
                if self.xsp_extra < d {
                    d = match js {
                        0x4000 => 0.0,
                        0x6000 => self.xsp_extra * 0.5,
                        _ => self.xsp_extra,
                    };
                }
                for k in self.line_start + 1..j {
                    self.items[k].x += d * (k - self.line_start) as f64;
                }
            }
        }
        let al = flags & 3;
        if al != 0 {
            let mut w = item_end(&self.items[j - 1]) - self.maxw;
            if al == 2 {
                w *= 0.5;
            }
            for k in self.line_start..j {
                self.items[k].x -= w;
            }
        }
        self.x = item_end(&self.items[j - 1]);
    }

    fn finish(&mut self, t: &mut Target) {
        let m = self.f.metrics;
        let flags = self.flags;
        self.line_end(0);
        let r = self.req;
        let vm = flags & 0x38;
        self.dy = match vm {
            0x10 => ((m.ascent - 1 + rne(r.rect[3])) - m.h) - self.first_y,
            0x18 | 0x28 => {
                let v = ((r.rect[3] - self.first_y as f64) - (m.ascent - 1) as f64)
                    + m.xheight as f64
                    + 131073.0;
                (rne(v) >> 1) - 0x10001
            }
            0x20 => {
                let v = ((r.rect[3] - self.first_y as f64) - (m.ascent - 1) as f64)
                    + m.capheight as f64
                    + 131072.0;
                (rne(v) >> 1) - 0x10001
            }
            _ => 0,
        };
        if flags & 0x80000 == 0 {
            let lut = &luts().alpha;
            let c = r.colour;
            let colour = [
                lut[(c & 0xff) as usize],
                lut[(c >> 8 & 0xff) as usize],
                lut[(c >> 16 & 0xff) as usize],
            ];
            for k in 0..self.count.min(self.items.len()) {
                let it = &self.items[k];
                let Some(g) = &it.glyph else { continue };
                if g.bits.is_none() {
                    continue;
                }
                composite(t, self.x0_3 + it.x, g, it.y + self.dy, &colour, it.lut_a);
                if it.flag & 1 != 0 {
                    let ul = self.f.underline(rne(g.adv - 6.0));
                    let x = self.x0_3 + it.x + g.bearing as f64 + f32r((g.w - ul.w) as f64 * 0.5);
                    composite(t, x, &ul, g.vadv + it.y + self.dy, &colour, it.lut_a);
                }
            }
        }
        if flags & 0x300000 != 0 {
            self.measure();
        }
    }

    fn measure(&mut self) {
        let m = self.f.metrics;
        let mode = self.flags & 0x300000;
        let items = &self.items[..self.count.min(self.items.len())];
        let (mut minx, mut maxx) = (2147483647.0f64, -2147483647.0f64);
        let (mut miny, mut maxy) = (i32::MAX, -i32::MAX);
        let r = &mut self.rect;
        r[2] = r[0];
        r[3] = r[1];
        let third = 0.3333333333333333;
        for it in items {
            let Some(g) = &it.glyph else { continue };
            match mode {
                0x100000 => {
                    minx = minx.min(it.x);
                    maxx = maxx.max(item_end(it));
                    miny = miny.min(it.y);
                    maxy = maxy.max(it.y);
                }
                0x200000 => {
                    if g.bits.is_none() {
                        continue;
                    }
                    minx = minx.min(it.x + g.bearing as f64);
                    maxx = maxx.max(it.x + g.bearing as f64 + g.w as f64);
                    miny = miny.min(g.top + it.y);
                    maxy = maxy.max(g.h + g.top + it.y);
                }
                _ => {
                    if g.bits.is_none() {
                        continue;
                    }
                    let a = it.x + g.bearing as f64;
                    minx = minx.min(a.min(it.x));
                    maxx = maxx.max((a + g.w as f64).max(item_end(it)));
                    miny = miny.min(it.y);
                    maxy = maxy.max(it.y);
                }
            }
        }
        if minx == 2147483647.0 {
            return;
        }
        r[0] = (self.x0_3 + minx.floor()) * third;
        r[2] = (self.x0_3 + maxx.ceil()) * third;
        if mode == 0x200000 {
            r[1] = (miny + self.dy) as f64;
            r[3] = (maxy + self.dy) as f64;
        } else {
            let base = (self.dy - (m.ascent - 1)) as f64;
            r[1] = base + miny as f64;
            r[3] = base + maxy as f64 + m.h as f64;
        }
    }
}

/// `FUN_00061790` with the subpixel kernel `FUN_00004638`: blends glyph `g` at text position
/// `x` (1/3 px) and row `y` into the window. `colour` holds the gamma values of B, G, R.
fn composite(t: &mut Target, x: f64, g: &Glyph, y: i32, colour: &[i32; 3], alpha: i32) {
    let Some(bits) = &g.bits else { return };
    let l = luts();
    let x = x + g.bearing as f64;
    let x3 = x.floor() as i64;
    let frac = ((x - x3 as f64) * 65536.0).floor() as i64;
    let mut y = g.top as i64 + y as i64;
    let mut px = (x3 + 0x3000000).div_euclid(3) - 0x1000000;
    let mut phase = 2 - (x3 - px * 3);
    let mut skip = 0i64;
    let mut n = (g.w as i64 - phase + 5) / 3;
    let mut h = g.h as i64;
    if px < 0 {
        phase += -px * 3;
        n += px;
        px = 0;
    }
    if y < 0 {
        skip = -y;
        h += y;
        y = 0;
    }
    let (ww, wh) = (t.w as i64, t.h as i64);
    if ww < n + px {
        n = ww - px;
    }
    if wh < h + y {
        h = wh - y;
    }
    if n <= 0 || h <= 0 {
        return;
    }
    let a0 = (frac * alpha as i64) >> 14;
    let a1 = (alpha as i64 * (65536 - frac)) >> 14;
    let sat16 = |v: i64| v.clamp(-32768, 32767);
    let wrap16 = |v: i64| v as i16 as i64;
    let src = |k: i64| -> i64 {
        if k >= 0 && (k as usize) < bits.len() {
            bits[k as usize] as i64
        } else {
            0
        }
    };
    let inv_max = l.inv.len() as i64 - 1;
    for j in 0..h {
        let base = (skip + j) * g.stride as i64 + phase;
        let row = (t.y as i64 + y + j) as usize * t.stride + (t.x as i64 + px) as usize;
        let Some(row) = t.px.get_mut(row..row + n as usize) else {
            return;
        };
        for (i, p) in row.iter_mut().enumerate() {
            let i = i as i64;
            let d = [
                l.alpha[(*p & 0xff) as usize] as i64,
                l.alpha[(*p >> 8 & 0xff) as usize] as i64,
                l.alpha[(*p >> 16 & 0xff) as usize] as i64,
            ];
            let s = [
                src(base + 3 * i) << 8,
                src(base + 3 * i + 1) << 8,
                src(base + 3 * i + 2) << 8,
                src(base + 3 * i + 3) << 8,
            ];
            let mut v = [0i64; 3];
            for k in 0..3 {
                let t1 = wrap16((s[k] * a0) >> 16);
                let t0 = wrap16((s[k + 1] * a1) >> 16);
                v[2 - k] = sat16(t1 + t0);
            }
            let mut out = *p & 0xff00_0000;
            for k in 0..3 {
                let e = sat16(colour[k] as i64 - d[k]);
                let e2 = wrap16(e << 1);
                let r = wrap16((e2 * v[k] + 8192) >> 14);
                let r2 = (r + 1) >> 1;
                let dk = sat16(r2 + d[k]);
                let idx = (INV_BASE as i64 + dk).clamp(0, inv_max) as usize;
                out |= (l.inv[idx] as u32) << (8 * k);
            }
            *p = out;
        }
    }
}
