//! The shape section under the form (a Backspin addition: the per-band shapers and the Clamp
//! clipper). It copies the form's structure rather than its header: the left bevel column holds a
//! slot grid (the shaper rack), a row of small knobs (Clamp) and a gradient bar at the bottom with
//! a mix knob and a pill, like the volume group; the side column holds side tabs; the right panel
//! holds an envelope-style view and a bottom row of tabs and combos, like the envelope and the
//! HOLD / TRIG SYNC / POS SYNC row. Knobs and the level meter are drawn live by the editor.

use crate::painters::{TOGGLE, bottom_tab, combo_at, control, h_button, rect, side_tab, v_button};
use crate::{Font, PANEL, Text};
use backspin_gfx::dib::lighten;
use backspin_gfx::{Dib, DrawPath, PI, Rect, gradient_lut};

/// The section height under the form.
pub const SB_HEIGHT: i32 = 205;
/// The section's group colour (beside time's green and volume's orange) and its fill variant.
pub const ACCENT: u32 = 0x7fd6ff;
pub const ACCENT_FILL: u32 = 0x6cc4ef;

/// Glyph code points of the section (private use; the text renderer maps them).
pub const GLYPH_WAVE: &str = "\u{e000}";
pub const GLYPH_PREV: &str = "\u{e001}";
pub const GLYPH_NEXT: &str = "\u{e002}";
pub const GLYPH_POWER: &str = "\u{e003}";

/// Control rects in section coordinates (the editor hit-tests the same rects and draws the knobs
/// and the meter bars in them). The columns are the form's: the slot grids' x range, the side
/// column at 270 and the envelope at 298; the bottom row sits where the form's does relative to
/// the bottom edge.
pub mod layout {
    use super::rect;
    use backspin_gfx::Rect;

    pub const BEVEL: Rect = rect(0, 0, 266, 152);
    /// Rack cells: 4 columns, 4 rows of the form's 62 x 16 slot cells.
    pub const RACK: Rect = rect(10, 8, 248, 64);
    pub const COLS: i32 = 4;
    pub const ROWS: i32 = 4;
    /// Clamp's knobs: IN, OUT, CEIL, SOFT.
    pub const CLAMP_KNOBS: [Rect; 4] = [
        rect(19, 82, 34, 34),
        rect(71, 82, 34, 34),
        rect(123, 82, 34, 34),
        rect(175, 82, 34, 34),
    ];
    /// The level meter: its frame and the input and output bars.
    pub const METER: Rect = rect(222, 82, 36, 44);
    pub const METER_IN: Rect = rect(226, 84, 12, 40);
    pub const METER_OUT: Rect = rect(242, 84, 12, 40);
    /// The shape bar: the mix knob and the pill.
    pub const KNOB: Rect = rect(7, 149, 48, 48);
    pub const PILL: Rect = rect(36, 158, 96, 31);
    pub const BANDS: Rect = rect(270, 12, 23, 81);
    pub const SHAPER_ON: Rect = rect(270, 151, 23, 27);
    pub const WAVE: Rect = rect(298, 4, 526, 172);
    /// The bottom row.
    pub const PRESET: Rect = rect(300, 182, 150, 19);
    pub const PREV: Rect = rect(452, 182, 22, 24);
    pub const NEXT: Rect = rect(475, 182, 22, 24);
    pub const CLAMP: Rect = rect(538, 182, 48, 19);
    pub const TYPE: Rect = rect(590, 182, 68, 19);
    pub const MODE: Rect = rect(662, 182, 80, 19);
    pub const OVERSAMPLING: Rect = rect(746, 182, 78, 19);
}

/// One shaper of the rack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaperCell {
    pub name: String,
    pub visible: bool,
    pub on: bool,
    /// Its DSP is ported (the others pass audio through).
    pub ported: bool,
}

/// What the section shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SbModel {
    pub enabled: bool,
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
    /// The readout in the wave view's corner.
    pub readout: String,
    /// Clamp: on, and the TYPE, MODE and OVERSAMPLING values.
    pub clamp_on: bool,
    pub clamp_type: String,
    pub clamp_mode: String,
    pub clamp_oversampling: String,
}

/// `0xRRGGBB` with alpha `a` as the canvas takes colours (`0xAABBGGRR`).
fn agg_colour(c: u32, a: u32) -> u32 {
    a << 24 | (c & 0xff) << 16 | (c & 0xff00) | (c >> 16 & 0xff)
}

/// The shape bar: BottomBgPaintBoxPaint's notched gradient turned upside down (the notch at the
/// top, under the mix knob) and the time pill's shape.
fn shape_bar(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let (top, w, h) = (144.0, 266.0, SB_HEIGHT as f64);
    let a = dib.agg();
    a.reset_path();
    a.line_width(1.0);
    a.line_color(0xff323a40);
    a.fill_color(0xff746e66);
    a.move_to(0.0, top);
    a.line_to(39.0, top);
    a.line_to(60.0, top + 8.0);
    a.line_to(w + 1.0, top + 8.0);
    a.line_to(w + 1.0, h + 1.0);
    a.line_to(0.0, h + 1.0);
    a.close_polygon();
    a.linear_gradient(w / 2.0, top, w / 2.0, h, 1);
    gradient_lut(a, 72.0, -48.0, 0xff746e66, 1);
    a.draw_path(DrawPath::FillAndStroke);
    let p = layout::PILL;
    let (x0, y0, y1) = (p.x0 as f64, p.y0 as f64, p.y1 as f64);
    a.reset_path();
    a.fill_color(0xff3a352c);
    a.move_to(x0, y0);
    a.line_to(118.0, y0);
    a.arc_to(14.0, 14.0, PI, false, true, 118.0, y1);
    a.line_to(x0, y1);
    a.line_to(x0, y0 + 1.0);
    a.draw_path(DrawPath::Fill);
    let r = Rect::new(p.x0, p.y0, 124, p.y1 - 1);
    let colour = if m.enabled { ACCENT } else { 0x8595a0 };
    text.draw(dib, Font::Caption, "shape", r, 0x1a, colour, 0x100);
    text.draw(dib, Font::IlGlyphs, GLYPH_WAVE, r, 0x19, colour, 0x100);
}

/// The captions of Clamp's knobs and the meter's frame (the knobs and bars are live).
fn clamp_row(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let colour = if m.clamp_on { 0xb9c5c4 } else { 0x5d6b74 };
    for (r, label) in layout::CLAMP_KNOBS
        .iter()
        .zip(["IN", "OUT", "CEIL", "SOFT"])
    {
        let c = (r.x0 + r.x1) / 2;
        let at = Rect::new(c - 20, 117, c + 20, 128);
        text.draw(dib, Font::ComboLabel, label, at, 0x1a, colour, 0x110);
    }
    let f = layout::METER;
    dib.multiply_rect(f, 0.6);
    dib.frame_rect(f, 0x2b3a45, 0x100, 1);
    for (r, label) in [(layout::METER_IN, "I"), (layout::METER_OUT, "O")] {
        dib.fill_rect(r, 0x0d1215, 0x100);
        let c = (r.x0 + r.x1) / 2;
        let at = Rect::new(c - 6, 126, c + 6, 137);
        text.draw(dib, Font::ComboLabel, label, at, 0x1a, colour, 0x110);
    }
}

/// The bottom row: the shape preset with previous and next, then Clamp's toggle and combos.
fn bottom_row(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let value = |w: i32, h: i32, x: i32| Rect::new(x, 1, w - 18, h);
    control(dib, layout::PRESET, |d, w, h| {
        combo_at(d, w, h, "PRESET:", &m.preset, value(w, h, 48), text)
    });
    control(dib, layout::PREV, |d, w, h| {
        h_button(d, w, h, GLYPH_PREV, false, text)
    });
    control(dib, layout::NEXT, |d, w, h| {
        h_button(d, w, h, GLYPH_NEXT, false, text)
    });
    control(dib, layout::CLAMP, |d, w, h| {
        bottom_tab(d, w - 18, m.clamp_on);
        let colour = if m.clamp_on { ACCENT } else { TOGGLE[0] };
        text.draw(
            d,
            Font::ComboLabel,
            "CLAMP",
            Rect::new(0, 1, w, h),
            0x1a,
            colour,
            0x110,
        );
    });
    control(dib, layout::TYPE, |d, w, h| {
        combo_at(d, w, h, "TYPE:", &m.clamp_type, value(w, h, 34), text)
    });
    control(dib, layout::MODE, |d, w, h| {
        combo_at(d, w, h, "MODE:", &m.clamp_mode, value(w, h, 38), text)
    });
    control(dib, layout::OVERSAMPLING, |d, w, h| {
        combo_at(d, w, h, "OS:", &m.clamp_oversampling, value(w, h, 24), text)
    });
}

/// The rack: the slot grid's cells (TimeSlotSelPaint) in the section colour.
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

/// The band tabs (SnapSel cells).
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
    dib.fill_rect(Rect::new(0, 0, w, h), PANEL, 0x100);
    for k in 1..16 {
        let x = k * (w - 1) / 16;
        let c = if k % 4 == 0 { 0x26363f } else { 0x2d414f };
        dib.fill_rect(Rect::new(x, 0, x + 1, h), c, 0x100);
    }
    for k in 1..4 {
        let y = k * (h - 1) / 4;
        dib.fill_rect(Rect::new(0, y, w, y + 1), 0x2d414f, 0x100);
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
    // Where the form's envelope shows its watermark.
    text.draw(
        dib,
        Font::Caption,
        &m.readout,
        Rect::new(8, h - 24, w - 10, h - 4),
        0x19,
        0x5d7383,
        0x100,
    );
}

/// Paints the section; `dib`'s origin is the section's top-left corner.
pub fn paint_sb(dib: &mut Dib, m: &SbModel, text: &mut dyn Text) {
    let w = dib.width();
    dib.fill_rect(Rect::new(0, 0, w, SB_HEIGHT), PANEL, 0x100);
    dib.fill_rect(layout::BEVEL, 0xff171f24, 0x100);
    control(dib, layout::RACK, |d, w, h| rack(d, w, h, m, text));
    clamp_row(dib, m, text);
    shape_bar(dib, m, text);
    control(dib, layout::BANDS, |d, w, _| bands(d, w, m, text));
    let on = m.shapers.get(m.selected).is_some_and(|s| s.visible && s.on);
    control(dib, layout::SHAPER_ON, |d, w, h| {
        v_button(d, w, h, GLYPH_POWER, on, text)
    });
    control(dib, layout::WAVE, |d, w, h| wave(d, w, h, m, text));
    bottom_row(dib, m, text);
}
