//! The ShaperBox section under the Gross Beat form (a Backspin addition). It reuses the form's
//! painters and palette: the gradient header of the bottom panel with a pill, the combos, the
//! slot-grid cells for the shaper rack, the side tabs for the bands and an envelope-style view of
//! the selected LFO wave.

use crate::painters::{TOGGLE, combo_at, control, h_button, rect, side_tab, v_button};
use crate::{Font, Text};
use backspin_gfx::dib::lighten;
use backspin_gfx::{Dib, DrawPath, PI, Rect, gradient_lut};

/// The section height under the form.
pub const SB_HEIGHT: i32 = 200;
/// The section accent (the third group colour) and its fill variant.
pub const ACCENT: u32 = 0x7fd6ff;
pub const ACCENT_FILL: u32 = 0x6cc4ef;

/// Glyph code points of the section (private use; the text renderer maps them).
pub const GLYPH_WAVE: &str = "\u{e000}";
pub const GLYPH_PREV: &str = "\u{e001}";
pub const GLYPH_NEXT: &str = "\u{e002}";
pub const GLYPH_POWER: &str = "\u{e003}";

/// Control rects in section coordinates (the plugin hit-tests the same rects).
pub mod layout {
    use super::rect;
    use backspin_gfx::Rect;

    pub const HEADER: Rect = rect(0, 0, 830, 40);
    pub const PILL: Rect = rect(10, 6, 124, 28);
    pub const GB_PRESET: Rect = rect(140, 11, 170, 19);
    pub const GB_PREV: Rect = rect(314, 9, 22, 24);
    pub const GB_NEXT: Rect = rect(337, 9, 22, 24);
    pub const PACK: Rect = rect(372, 11, 170, 19);
    pub const PRESET: Rect = rect(546, 11, 196, 19);
    pub const PREV: Rect = rect(746, 9, 22, 24);
    pub const NEXT: Rect = rect(769, 9, 22, 24);
    pub const POWER: Rect = rect(800, 9, 22, 24);
    pub const RACK: Rect = rect(10, 50, 400, 144);
    pub const BANDS: Rect = rect(418, 50, 23, 81);
    pub const SHAPER_ON: Rect = rect(418, 140, 23, 27);
    pub const WAVE: Rect = rect(448, 50, 376, 144);
    /// Rack cells: 4 columns, 4 rows.
    pub const COLS: i32 = 4;
    pub const ROWS: i32 = 4;
}

/// One shaper of the rack.
#[derive(Clone, Debug, Default)]
pub struct ShaperCell {
    pub name: String,
    pub visible: bool,
    pub on: bool,
    /// Its DSP is ported (the others pass audio through).
    pub ported: bool,
}

/// What the section shows.
#[derive(Clone, Debug, Default)]
pub struct SbModel {
    pub enabled: bool,
    pub gb_preset: String,
    pub pack: String,
    pub preset: String,
    pub shapers: Vec<ShaperCell>,
    pub selected: usize,
    /// The selected band 0..2 and which bands the split makes active.
    pub band: usize,
    pub bands_active: [bool; 3],
    /// The selected band's first LFO: its table (one period) and points (`x` in [0, 1),
    /// `y` in [-1, 1]); empty when the shaper has no LFO.
    pub table: Vec<f32>,
    pub points: Vec<(f32, f32)>,
    /// The info line in the wave view.
    pub info: String,
}

/// `0xRRGGBB` with alpha `a` as the canvas takes colours (`0xAABBGGRR`).
fn agg_colour(c: u32, a: u32) -> u32 {
    a << 24 | (c & 0xff) << 16 | (c & 0xff00) | (c >> 16 & 0xff)
}

/// The header: the bottom panel's gradient, the pill and the preset combos.
fn header(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let w = layout::HEADER.x1;
    let h = layout::HEADER.y1;
    let a = dib.agg();
    a.reset_path();
    a.line_width(1.0);
    a.line_color(0xff323a40);
    a.fill_color(0xff746e66);
    a.move_to(0.0, 0.0);
    a.line_to((w + 1) as f64, 0.0);
    a.line_to((w + 1) as f64, h as f64);
    a.line_to(0.0, h as f64);
    a.close_polygon();
    a.linear_gradient((w / 2) as f64, 0.0, (w / 2) as f64, h as f64, 1);
    gradient_lut(a, 72.0, -48.0, 0xff746e66, 1);
    a.draw_path(DrawPath::FillAndStroke);
    let p = layout::PILL;
    let (x0, y0, x1, y1) = (p.x0 as f64, p.y0 as f64, p.x1 as f64, p.y1 as f64);
    let r = (y1 - y0) * 0.5;
    a.reset_path();
    a.fill_color(0xff3a352c);
    a.move_to(x0, y0);
    a.line_to(x1 - r, y0);
    a.arc_to(r, r, PI, false, true, x1 - r, y1);
    a.line_to(x0, y1);
    a.close_polygon();
    a.draw_path(DrawPath::Fill);
    let caption = Rect::new(p.x0, p.y0, p.x1 - 14, p.y1);
    let colour = if m.enabled { ACCENT } else { 0x8595a0 };
    text.draw(
        dib,
        Font::Caption,
        "shaperbox",
        caption,
        0x1a,
        colour,
        0x100,
    );
    text.draw(
        dib,
        Font::IlGlyphs,
        GLYPH_WAVE,
        Rect::new(p.x0, p.y0, p.x1 - 4, p.y1),
        0x19,
        colour,
        0x100,
    );
    control(dib, layout::GB_PRESET, |d, w, h| {
        combo_at(
            d,
            w,
            h,
            "PRESET:",
            &m.gb_preset,
            Rect::new(48, 1, w - 18, h),
            text,
        )
    });
    control(dib, layout::GB_PREV, |d, w, h| {
        h_button(d, w, h, GLYPH_PREV, false, text)
    });
    control(dib, layout::GB_NEXT, |d, w, h| {
        h_button(d, w, h, GLYPH_NEXT, false, text)
    });
    control(dib, layout::PACK, |d, w, h| {
        combo_at(d, w, h, "PACK:", &m.pack, Rect::new(38, 1, w - 18, h), text)
    });
    control(dib, layout::PRESET, |d, w, h| {
        combo_at(
            d,
            w,
            h,
            "SHAPER:",
            &m.preset,
            Rect::new(50, 1, w - 18, h),
            text,
        )
    });
    control(dib, layout::PREV, |d, w, h| {
        h_button(d, w, h, GLYPH_PREV, false, text)
    });
    control(dib, layout::NEXT, |d, w, h| {
        h_button(d, w, h, GLYPH_NEXT, false, text)
    });
    control(dib, layout::POWER, |d, w, h| {
        h_button(d, w, h, GLYPH_POWER, m.enabled, text)
    });
}

/// The rack: the slot grid's cells (TimeSlotSelPaint) with the section accent.
fn rack(dib: &mut Dib, w: i32, h: i32, m: &SbModel, text: &mut dyn Text) {
    let (cw, ch) = (w / layout::COLS, h / layout::ROWS);
    for (i, s) in m.shapers.iter().enumerate() {
        let (col, row) = (i as i32 % layout::COLS, i as i32 / layout::COLS);
        let r = Rect::new(col * cw, row * ch, col * cw + cw - 1, row * ch + ch - 1);
        let active = m.enabled && s.visible && s.on;
        dib.multiply_rect(r, if s.visible { 1.8 } else { 1.25 });
        if active {
            dib.fill_rect(Rect::new(r.x0, r.y1 - 3, r.x1, r.y1), ACCENT_FILL, 0x78);
        }
        if i == m.selected {
            dib.frame_rect(r, ACCENT_FILL, 0x50, 1);
        }
        let colour = if i == m.selected {
            lighten(ACCENT, 0x18) & 0xffffff
        } else if active && s.ported {
            ACCENT
        } else if active {
            0x8595a0
        } else if s.visible {
            0xd0dfea
        } else {
            0x485762
        };
        let tr = Rect::new(r.x0 + 3, r.y0 + 1, r.x1 - 2, r.y1);
        text.draw(dib, Font::SlotCell, &s.name, tr, 0x205a, colour, 0x110);
    }
}

/// The band tabs (SnapSel cells) and the shaper's on toggle.
fn bands(dib: &mut Dib, w: i32, m: &SbModel, text: &mut dyn Text) {
    const NAMES: [&str; 3] = ["L", "M", "H"];
    let order = (0..3).filter(|&b| b != m.band).chain([m.band]);
    for b in order {
        let y = b as i32 * 27;
        let on = b == m.band;
        side_tab(dib, y as f64, on);
        let colour = if !m.bands_active[b] {
            0x485762
        } else {
            TOGGLE[on as usize]
        };
        text.draw(
            dib,
            Font::Snap,
            NAMES[b],
            Rect::new(0, y, w, y + 27),
            0x1a,
            colour,
            0x110,
        );
    }
}

/// The envelope-style view of the selected LFO wave.
fn wave(dib: &mut Dib, w: i32, h: i32, m: &SbModel, text: &mut dyn Text) {
    dib.fill_rect(Rect::new(0, 0, w, h), 0xff2c3e4b, 0x100);
    for k in 1..16 {
        let x = k * (w - 1) / 16;
        let (c, a) = if k % 4 == 0 {
            (0x4a6273, 0x100)
        } else {
            (0x3a5060, 0x100)
        };
        dib.fill_rect(Rect::new(x, 0, x + 1, h), c, a);
    }
    for k in 1..4 {
        let y = k * (h - 1) / 4;
        dib.fill_rect(Rect::new(0, y, w, y + 1), 0x3a5060, 0x100);
    }
    let (fw, fh) = ((w - 1) as f64, (h - 1) as f64);
    let ys = |v: f32| fh * (1.0 - (v as f64 + 1.0) * 0.5);
    if !m.table.is_empty() {
        let n = m.table.len();
        let at = |x: i32| m.table[(x as usize * n / w as usize).min(n - 1)];
        let colour = if m.enabled { ACCENT } else { 0x8595a0 };
        let a = dib.agg();
        a.reset_path();
        a.fill_color(agg_colour(ACCENT_FILL, 0x30));
        a.move_to(0.0, fh);
        for x in 0..w {
            a.line_to(x as f64 + 0.5, ys(at(x)));
        }
        a.line_to(fw, fh);
        a.close_polygon();
        a.draw_path(DrawPath::Fill);
        a.reset_path();
        a.line_width(1.5);
        a.line_color(agg_colour(colour, 0xff));
        a.move_to(0.5, ys(at(0)));
        for x in 1..w {
            a.line_to(x as f64 + 0.5, ys(at(x)));
        }
        a.draw_path(DrawPath::Stroke);
        for &(px, py) in &m.points {
            let (cx, cy) = ((px as f64 * fw) as i32, ys(py) as i32);
            dib.fill_rect(Rect::new(cx - 2, cy - 2, cx + 3, cy + 3), 0xff171f24, 0x100);
            dib.frame_rect(Rect::new(cx - 2, cy - 2, cx + 3, cy + 3), colour, 0x100, 1);
        }
    }
    dib.frame_rect(Rect::new(0, 0, w, h), 0x171f24, 0x100, 1);
    text.draw(
        dib,
        Font::SlotCell,
        &m.info,
        Rect::new(6, 3, w - 6, 17),
        0x18,
        0xb0bdc6,
        0x110,
    );
}

/// Paints the section; `dib`'s origin is the section's top-left corner.
pub fn paint_sb(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let w = dib.width();
    dib.fill_rect(Rect::new(0, 0, w, crate::sb::SB_HEIGHT), 0xff171f24, 0x100);
    header(dib, m, text);
    control(dib, layout::RACK, |d, w, h| rack(d, w, h, m, text));
    control(dib, layout::BANDS, |d, w, _| bands(d, w, m, text));
    let on = m.shapers.get(m.selected).is_some_and(|s| s.visible && s.on);
    control(dib, layout::SHAPER_ON, |d, w, h| {
        v_button(d, w, h, GLYPH_POWER, on, text)
    });
    control(dib, layout::WAVE, |d, w, h| wave(d, w, h, m, text));
}
