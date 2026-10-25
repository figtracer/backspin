//! The form's painters at scale 1 (gfx.md sections 7.2, 7.4 and 8), in the component order of
//! `SynthEditorForm` (the later control paints over the earlier one). Controls not ported yet
//! (the knobs, HOLD, Options, SlotZoom, the scratch clock, the envelope editor) are skipped.

use crate::{Font, HEIGHT, Model, PANEL, Text};
use backspin_gfx::dib::lighten;
use backspin_gfx::{Dib, DrawPath, PI, Rect, gradient_lut, s};

const SC: f64 = 1.0;

pub(crate) fn sd(n: f64) -> f64 {
    s(n, SC) as f64
}

/// `FUN_00148200`: paints a graphic control in its own coordinates, clipped to its rect.
pub(crate) fn control(dib: &mut Dib, r: Rect, f: impl FnOnce(&mut Dib, i32, i32)) {
    let old = dib.clip();
    dib.move_origin(r.x0, r.y0);
    let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
    let c = Rect::new(0, 0, w, h).intersect(dib.clip());
    dib.set_clip(c);
    f(dib, w, h);
    dib.move_origin(-r.x0, -r.y0);
    dib.set_clip(old);
}

pub(crate) const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect::new(x, y, x + w, y + h)
}

/// The slot grids at form height 535 (layout `FUN_00064c50`): `h = (H - S(236)) div 2`.
fn grid_rects() -> [Rect; 2] {
    let h = (HEIGHT - s(236.0, SC)) / 2;
    [rect(10, 102, 248, h), rect(10, 102 + h + 11, 248, h)]
}

/// TopBgPaintBoxPaint (0x6266c).
fn top_bg(dib: &mut Dib, w: i32, clock: bool, text: &mut dyn Text) {
    dib.fill_rect(Rect::new(0, 0, w, 91), 0xff171f24, 0x100);
    let a = dib.agg();
    a.reset_path();
    a.line_width(1.0);
    a.line_color(0xff323a40);
    a.fill_color(0xff746e66);
    a.move_to(sd(0.0), sd(0.0));
    a.line_to(sd(0.0), sd(61.0));
    a.line_to(sd(39.0), sd(61.0));
    a.line_to(sd(60.0), sd(53.0));
    if clock {
        let cx = (186 - s(10.0, SC)) as f64;
        a.line_to(cx, sd(53.0));
        a.arc_to(
            sd(5.0),
            sd(5.0),
            PI * 0.5,
            false,
            true,
            sd(5.0) + cx,
            sd(58.0),
        );
        a.line_to(sd(5.0) + cx, sd(85.0));
        a.arc_to(
            sd(5.0),
            sd(5.0),
            PI * 0.5,
            false,
            false,
            sd(10.0) + cx,
            sd(90.0),
        );
        a.line_to(sd(1.0) + w as f64, sd(90.0));
    } else {
        a.line_to(sd(1.0) + w as f64, sd(53.0));
    }
    a.line_to(sd(1.0) + w as f64, sd(0.0));
    a.line_to(sd(0.0), sd(0.0));
    a.linear_gradient((w / 2) as f64, 0.0, (w / 2) as f64, sd(90.0), 1);
    gradient_lut(a, 72.0, -48.0, 0xff746e66, 1);
    a.draw_path(DrawPath::FillAndStroke);
    a.reset_path();
    a.fill_color(0xff3a352c);
    a.move_to(sd(36.0), sd(14.0));
    a.line_to(sd(118.0), sd(14.0));
    a.arc_to(sd(14.0), sd(14.0), PI, false, true, sd(118.0), sd(45.0));
    a.line_to(sd(36.0), sd(45.0));
    a.line_to(sd(36.0), sd(15.0));
    a.draw_path(DrawPath::Fill);
    let r = Rect::new(36, 14, 124, 44);
    text.draw(dib, Font::Caption, "time", r, 0x1a, 0xa1f46d, 0x100);
    text.draw(dib, Font::IlGlyphs, "\u{f463}", r, 0x19, 0xa1f46d, 0x100);
}

/// BottomBgPaintBoxPaint (0x634e0).
fn bottom_bg(dib: &mut Dib, w: i32, text: &mut dyn Text) {
    dib.fill_rect(Rect::new(0, 0, w, 63), 0xff171f24, 0x100);
    let a = dib.agg();
    a.reset_path();
    a.line_width(1.0);
    a.line_color(0xff323a40);
    a.fill_color(0xff746e66);
    a.move_to(0.0, 0.0);
    a.line_to(38.0, 0.0);
    a.line_to(58.0, 8.0);
    a.line_to(sd(60.0) + w as f64, 8.0);
    a.line_to(sd(40.0) + w as f64, 0.0);
    a.line_to((w + 1) as f64, 0.0);
    a.line_to((w + 1) as f64, 63.0);
    a.line_to(0.0, 63.0);
    a.linear_gradient((w / 2) as f64, 0.0, (w / 2) as f64, 63.0, 1);
    gradient_lut(a, 72.0, -48.0, 0xff746e66, 1);
    a.draw_path(DrawPath::FillAndStroke);
    a.reset_path();
    a.fill_color(0xff3a352c);
    a.move_to(36.0, 18.0);
    a.line_to(118.0, 18.0);
    a.arc_to(15.0, 15.0, PI, false, true, 118.0, 48.0);
    a.line_to(36.0, 48.0);
    a.close_polygon();
    a.draw_path(DrawPath::Fill);
    let r = Rect::new(36, 21, 124, 47);
    text.draw(dib, Font::Caption, "volume", r, 0x1a, 0xffb870, 0x100);
    text.draw(dib, Font::IlGlyphs, "\u{f0b8}", r, 0x19, 0xffb870, 0x100);
}

/// `FUN_00065c00(form, ctrl, y, on)`: the chamfered side tab.
pub(crate) fn side_tab(dib: &mut Dib, y: f64, on: bool) {
    let a = dib.agg();
    a.reset_path();
    a.line_width(sd(2.0));
    a.line_color_sep(0x25, 0x31, 0x3b, 0xff);
    if on {
        a.fill_color_sep(0x25, 0x31, 0x3b, 0xff);
    } else {
        a.fill_color_sep(0x34, 0x4b, 0x5b, 0xff);
    }
    a.move_to(sd(1.0), y + sd(27.0));
    a.line_to(sd(1.0), y + sd(7.0));
    a.line_to(sd(4.0), y + sd(4.0));
    a.line_to(sd(19.0), y + sd(4.0));
    a.line_to(sd(22.0), y + sd(0.0));
    a.line_to(sd(22.0), y + sd(19.0));
    a.line_to(sd(19.0), y + sd(22.0));
    a.line_to(sd(3.0), y + sd(22.0));
    a.close_polygon();
    a.draw_path(DrawPath::FillAndStroke);
}

/// `FUN_000661a0(form, ctrl, w, on)`: the chamfered bottom tab.
pub(crate) fn bottom_tab(dib: &mut Dib, w: i32, on: bool) {
    let a = dib.agg();
    a.reset_path();
    a.line_width(sd(2.0));
    a.line_color_sep(0x25, 0x31, 0x3b, 0xff);
    if on {
        a.fill_color_sep(0x25, 0x31, 0x3b, 0xff);
    } else {
        a.fill_color_sep(0x34, 0x4b, 0x5b, 0xff);
    }
    let w = w as f64;
    a.move_to(sd(1.0), sd(17.0));
    a.line_to(sd(1.0), sd(4.0));
    a.line_to(sd(4.0), sd(1.0));
    a.line_to(w + sd(17.0), sd(1.0));
    a.line_to(w + sd(17.0), sd(14.0));
    a.line_to(w + sd(14.0), sd(17.0));
    a.close_polygon();
    a.draw_path(DrawPath::FillAndStroke);
}

pub(crate) const TOGGLE: [u32; 2] = [0x1a232a, 0xb0bdc6];

/// EnvVBtnPaint (0x66600): the side toggle and its glyph.
pub(crate) fn v_button(
    dib: &mut Dib,
    w: i32,
    h: i32,
    glyph: &str,
    down: bool,
    text: &mut dyn Text,
) {
    side_tab(dib, 0.0, down);
    text.draw(
        dib,
        Font::ButtonGlyph,
        glyph,
        Rect::new(0, 0, w, h),
        0x1a,
        TOGGLE[down as usize],
        0x110,
    );
}

/// EnvHBtnPaint (0x666f0): the bottom toggle and its glyph.
pub(crate) fn h_button(
    dib: &mut Dib,
    w: i32,
    h: i32,
    glyph: &str,
    down: bool,
    text: &mut dyn Text,
) {
    bottom_tab(dib, 0, down);
    text.draw(
        dib,
        Font::ButtonGlyph,
        glyph,
        Rect::new(-5, -3, w, h),
        0x2,
        TOGGLE[down as usize],
        0x110,
    );
}

/// TPaintBoxSelector.Paint with SnapSelPaintCell / MapTypeSelPaintCell: every cell, the selected
/// one last.
fn side_selector(
    dib: &mut Dib,
    h: i32,
    rows: i32,
    selected: usize,
    mut cell: impl FnMut(&mut Dib, i32, bool),
) {
    let ch = h / rows;
    let order = (0..rows as usize)
        .filter(|&i| i != selected)
        .chain([selected]);
    for i in order {
        cell(dib, i as i32 * ch, i == selected);
    }
}

/// TimeSlotSelPaint (0x66bb0) for one group.
fn slot_grid(dib: &mut Dib, w: i32, h: i32, m: &Model, g: usize, text: &mut dyn Text) {
    const FILL: [u32; 2] = [0x99e669, 0xffb870];
    const GROUP: [u32; 2] = [0xa8ff70, 0xffb870];
    let (cw, ch) = (w / 4, h / 9);
    let selected = m.selected[g];
    for i in 0..36usize {
        let (col, row) = ((i % 4) as i32, (i / 4) as i32);
        let r = Rect::new(col * cw, row * ch, col * cw + cw - 1, row * ch + ch - 1);
        if r.intersect(dib.clip()).is_empty() {
            continue;
        }
        let empty_unnamed = m.empty[g][i] && m.names[g][i].is_empty();
        dib.multiply_rect(r, if empty_unnamed { 0.6 } else { 1.8 });
        let playing = m.playing[g] == i as i32;
        let is_sel = selected == i as i32;
        if playing {
            if is_sel {
                dib.frame_rect(r, FILL[g], 0x50, 1);
            }
            dib.fill_rect(
                Rect::new(r.x0, r.y0, r.x0 + m.bar_px[g], r.y1),
                FILL[g],
                0x3c,
            );
        }
        if (!playing || !is_sel) && (is_sel || m.edited[g] == i) {
            dib.frame_rect(r, FILL[g], 0x50, 1);
        }
        let tr = Rect::new(r.x0 + s(3.0, SC), r.y0 + 1, r.x1 - s(2.0, SC), r.y1);
        let colour = if playing || is_sel {
            lighten(GROUP[g], 0x18) & 0xffffff
        } else if m.edited[g] == i {
            0xf0ff0a
        } else if empty_unnamed {
            0x485762
        } else {
            0xd0dfea
        };
        let name = if m.names[g][i].is_empty() {
            format!("Slot {}", i + 1)
        } else {
            m.names[g][i].clone()
        };
        text.draw(dib, Font::SlotCell, &name, tr, 0x205a, colour, 0x110);
    }
}

/// The combo boxes (ForceHoldSelPaint 0x617a0).
pub(crate) fn combo(dib: &mut Dib, w: i32, h: i32, label: &str, value: &str, text: &mut dyn Text) {
    let v = Rect::new((w as f64 * 0.5) as i32, s(1.0, SC), w, h);
    combo_at(dib, w, h, label, value, v, text);
}

/// The combo with its value in `v` (the ShaperBox section's narrow-label combos).
pub(crate) fn combo_at(
    dib: &mut Dib,
    w: i32,
    h: i32,
    label: &str,
    value: &str,
    v: Rect,
    text: &mut dyn Text,
) {
    bottom_tab(dib, w - s(18.0, SC), true);
    text.draw(
        dib,
        Font::ComboLabel,
        label,
        Rect::new(s(4.0, SC), s(1.0, SC), w, h),
        0x18,
        0xb0bdc6,
        0x110,
    );
    text.draw(dib, Font::ComboValue, value, v, 0x18, 0xc4d0d7, 0x110);
    text.draw(
        dib,
        Font::ButtonGlyph,
        "\u{f05f}",
        Rect::new(w - s(16.0, SC), s(1.0, SC), w, h),
        0x1a,
        0xb0bdc6,
        0x110,
    );
}

/// Paints the frame (the parent form; the envelope editor child window is separate).
pub fn paint(dib: &mut Dib, m: &Model, text: &mut dyn Text) {
    dib.fill_rect(Rect::new(0, 0, dib.width(), dib.height()), PANEL, 0x100);
    // SlotsBevel (VB_Plain #171f24).
    dib.fill_rect(rect(0, 62, 266, 411), 0xff171f24, 0x100);
    control(dib, rect(0, 472, 266, 63), |d, w, _| bottom_bg(d, w, text));
    control(dib, rect(0, 0, 266, 91), |d, w, _| {
        top_bg(d, w, m.clock, text)
    });
    control(dib, rect(270, 327, 23, 27), |d, w, h| {
        v_button(d, w, h, "\u{f07e}", m.freeze, text)
    });
    control(dib, rect(270, 4, 23, 27), |d, w, h| {
        v_button(d, w, h, "\u{f08b}", false, text)
    });
    control(dib, rect(270, 275, 23, 27), |d, w, h| {
        v_button(d, w, h, "C", m.step, text)
    });
    const SNAP: [&str; 7] = ["X", "1/16", "1/12", "1/8", "1/6", "1/4", "1/3"];
    control(dib, rect(270, 40, 23, 189), |d, w, h| {
        side_selector(d, h, 7, m.snap, |d, y, on| {
            side_tab(d, y as f64, on);
            let ch = h / 7;
            let i = (y / ch) as usize;
            text.draw(
                d,
                Font::Snap,
                SNAP[i],
                Rect::new(0, y, w, y + ch),
                0x1a,
                TOGGLE[on as usize],
                0x110,
            );
        })
    });
    control(dib, rect(270, 301, 23, 27), |d, w, h| {
        v_button(d, w, h, "\u{f0c3}", m.slide, text)
    });
    control(dib, rect(270, 470, 23, 52), |d, w, h| {
        side_selector(d, h, 2, m.group, |d, y, on| {
            side_tab(d, y as f64, on);
            let ch = h / 2;
            let i = (y / ch) as usize;
            let colours = [0x1a232a, 0x94db66];
            text.draw(
                d,
                Font::MapType,
                ["\u{f094}", "\u{f0a0}"][i],
                Rect::new(0, y, w, y + ch),
                0x1a,
                colours[on as usize],
                0x110,
            );
        })
    });
    // The knobs (VolAttWheel, VolTnsWheel, VolRelWheel) come next; they leave the shared canvas
    // with the round join, which the later tabs inherit (the constructor's reset runs before
    // attach, so the earlier tabs stroke with Blend2D's default miter clip).
    dib.agg().line_join(2);
    let [tg, vg] = grid_rects();
    control(dib, tg, |d, w, h| slot_grid(d, w, h, m, 0, text));
    control(dib, vg, |d, w, h| slot_grid(d, w, h, m, 1, text));
    control(dib, rect(610, 512, 104, 19), |d, w, h| {
        combo(d, w, h, "TRIG SYNC:", &m.combos[1], text)
    });
    control(dib, rect(718, 512, 106, 19), |d, w, h| {
        combo(d, w, h, "POS SYNC:", &m.combos[2], text)
    });
    if m.group == 0 {
        control(dib, rect(457, 512, 22, 24), |d, w, h| {
            h_button(d, w, h, "\u{f054}", m.link, text)
        });
    }
    control(dib, rect(480, 512, 22, 24), |d, w, h| {
        h_button(d, w, h, "\u{f014}", m.key_held, text)
    });
    control(dib, rect(503, 512, 22, 24), |d, w, h| {
        h_button(d, w, h, "1", m.one_shot, text)
    });
    control(dib, rect(530, 512, 76, 19), |d, w, h| {
        combo(d, w, h, "HOLD:", &m.combos[0], text)
    });
}
