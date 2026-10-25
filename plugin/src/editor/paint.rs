//! Drawing helpers: colours, glyphs and the framework widgets (ui.md sections 2.5 and 6).
//!
//! The FL framework widgets (knob bevels, button styles) were not reverse engineered; they are
//! drawn after the thumbnail (SPEC.md decision 14). Positions, sizes and colours come from ui.md.

use nih_plug_egui::egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2,
};
use std::f32::consts::{PI, TAU};

/// A colour from `0xRRGGBB`.
pub const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A colour from `0xRRGGBB` with alpha.
pub fn rgba(c: u32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, a)
}

/// "Lighten" (FUN_001a1d70): adds `d` to each channel and clamps.
pub fn lighten(c: Color32, d: i32) -> Color32 {
    let f = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
    Color32::from_rgba_unmultiplied(f(c.r()), f(c.g()), f(c.b()), c.a())
}

/// The colour with alpha `a`.
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Mixes `c` towards `to` by `t` (0..1).
pub fn mix(c: Color32, to: Color32, t: f32) -> Color32 {
    let f = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(f(c.r(), to.r()), f(c.g(), to.g()), f(c.b(), to.b()))
}

pub const PANEL: Color32 = rgb(0x344b5b);
pub const TIME: Color32 = rgb(0xa8ff70);
pub const VOLUME: Color32 = rgb(0xffb870);
pub const TIME_MIX: Color32 = rgb(0x86bf69);
pub const KNOB_RAIL: Color32 = rgb(0x3f484d);
pub const KNOB_PRESSED: Color32 = rgb(0xced0d1);
pub const LABEL: Color32 = rgb(0xb9c5c4);
pub const ORANGE: Color32 = rgb(0xff9637);
pub const HOLD_ON: Color32 = rgb(0x83e32b);
pub const HOLD_TEXT: Color32 = rgb(0x9fa8ad);
pub const SELECT_RING: Color32 = rgb(0xa8e44a);

/// The colour of a group (bank): time green, volume orange.
pub fn group_color(bank: usize) -> Color32 {
    if bank == 0 { TIME } else { VOLUME }
}

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// The icons of the IL glyph font, drawn as vector shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// U+F08B: the envelope options menu.
    Menu,
    /// `C` (ILGlyphsEx): step editing.
    Step,
    /// U+F0C3: slide remaining points.
    Slide,
    /// U+F07E: freeze editing.
    Freeze,
    /// U+F094: time travel.
    TimeMap,
    /// U+F0A0: volume.
    VolumeMap,
    /// U+F054: link volume slot.
    Link,
    /// U+F014: key-held.
    KeyHeld,
    /// `1`: one-shot.
    OneShot,
    /// U+F05F: combo arrow.
    Down,
    /// U+F024: toggle envelope visibility.
    Zoom,
    /// ILIndex 16: options.
    Options,
    /// U+F463: the time pill.
    Clock,
    /// U+F0B8: the volume pill.
    Speaker,
}

/// Draws a glyph centred in `r`.
pub fn glyph(p: &Painter, r: Rect, g: Glyph, c: Color32) {
    let s = Stroke::new(1.5f32, c);
    let ctr = r.center();
    let w = r.width().min(r.height()) * 0.5;
    match g {
        Glyph::Menu => {
            for k in -1..=1 {
                let y = ctr.y + k as f32 * w * 0.45;
                p.line_segment([pos2(ctr.x - w * 0.6, y), pos2(ctr.x + w * 0.6, y)], s);
            }
        }
        Glyph::Step => {
            let pts = [
                (-0.6, 0.5),
                (-0.6, 0.1),
                (-0.2, 0.1),
                (-0.2, -0.3),
                (0.2, -0.3),
                (0.2, 0.2),
                (0.6, 0.2),
                (0.6, -0.5),
            ];
            let v: Vec<Pos2> = pts
                .iter()
                .map(|(x, y)| pos2(ctr.x + x * w, ctr.y + y * w))
                .collect();
            p.add(Shape::line(v, s));
        }
        Glyph::Slide => {
            for dx in [-0.35, 0.15] {
                let x = ctr.x + dx * w;
                p.add(Shape::line(
                    vec![
                        pos2(x - 0.2 * w, ctr.y - 0.4 * w),
                        pos2(x + 0.15 * w, ctr.y),
                        pos2(x - 0.2 * w, ctr.y + 0.4 * w),
                    ],
                    s,
                ));
            }
        }
        Glyph::Freeze => {
            for k in 0..3 {
                let a = k as f32 * PI / 3.0;
                let d = vec2(a.cos(), a.sin()) * w * 0.6;
                p.line_segment([ctr - d, ctr + d], s);
            }
        }
        Glyph::TimeMap => {
            let v = vec![
                pos2(ctr.x - 0.6 * w, ctr.y + 0.4 * w),
                pos2(ctr.x - 0.1 * w, ctr.y - 0.4 * w),
                pos2(ctr.x - 0.1 * w, ctr.y + 0.4 * w),
                pos2(ctr.x + 0.5 * w, ctr.y - 0.4 * w),
            ];
            p.add(Shape::line(v, s));
        }
        Glyph::VolumeMap | Glyph::Speaker => {
            let v = vec![
                pos2(ctr.x - 0.6 * w, ctr.y - 0.2 * w),
                pos2(ctr.x - 0.3 * w, ctr.y - 0.2 * w),
                pos2(ctr.x + 0.1 * w, ctr.y - 0.55 * w),
                pos2(ctr.x + 0.1 * w, ctr.y + 0.55 * w),
                pos2(ctr.x - 0.3 * w, ctr.y + 0.2 * w),
                pos2(ctr.x - 0.6 * w, ctr.y + 0.2 * w),
            ];
            p.add(Shape::convex_polygon(v, c, Stroke::NONE));
            p.add(Shape::line(
                vec![
                    pos2(ctr.x + 0.35 * w, ctr.y - 0.3 * w),
                    pos2(ctr.x + 0.5 * w, ctr.y),
                    pos2(ctr.x + 0.35 * w, ctr.y + 0.3 * w),
                ],
                s,
            ));
        }
        Glyph::Link => {
            p.circle_stroke(ctr - vec2(0.25 * w, 0.0), 0.32 * w, s);
            p.circle_stroke(ctr + vec2(0.25 * w, 0.0), 0.32 * w, s);
        }
        Glyph::KeyHeld => {
            let k = Rect::from_center_size(ctr, vec2(0.7 * w, 1.1 * w));
            p.rect_stroke(k, 1.0, s, nih_plug_egui::egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_size(k.min, vec2(0.35 * w, 0.6 * w)), 0.0, c);
        }
        Glyph::OneShot => {
            p.text(ctr, Align2::CENTER_CENTER, "1", font(12.0), c);
        }
        Glyph::Down => {
            p.add(Shape::convex_polygon(
                vec![
                    pos2(ctr.x - 0.4 * w, ctr.y - 0.2 * w),
                    pos2(ctr.x + 0.4 * w, ctr.y - 0.2 * w),
                    pos2(ctr.x, ctr.y + 0.3 * w),
                ],
                c,
                Stroke::NONE,
            ));
        }
        Glyph::Zoom => {
            let v = vec![
                pos2(ctr.x - 0.7 * w, ctr.y + 0.3 * w),
                pos2(ctr.x - 0.3 * w, ctr.y - 0.4 * w),
                pos2(ctr.x + 0.1 * w, ctr.y + 0.2 * w),
                pos2(ctr.x + 0.4 * w, ctr.y - 0.2 * w),
                pos2(ctr.x + 0.7 * w, ctr.y + 0.3 * w),
            ];
            p.add(Shape::line(v, s));
        }
        Glyph::Options => {
            p.circle_stroke(ctr, 0.35 * w, s);
            for k in 0..6 {
                let a = k as f32 * PI / 3.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([ctr + d * 0.45 * w, ctr + d * 0.7 * w], s);
            }
        }
        Glyph::Clock => {
            p.circle_stroke(ctr, 0.55 * w, s);
            p.line_segment([ctr, ctr + vec2(0.0, -0.35 * w)], s);
            p.line_segment([ctr, ctr + vec2(0.3 * w, 0.0)], s);
        }
    }
}

/// A TVectorWheel knob. `t` is the normalized value (0..1), `centered` draws from the middle.
/// `gap_right` puts the rail gap at the right (the mix knobs), else at the bottom.
pub fn knob(
    p: &Painter,
    r: Rect,
    t: f32,
    fill: Color32,
    centered: bool,
    gap_right: bool,
    pressed: bool,
) {
    let c = r.center();
    let rad = r.width().min(r.height()) * 0.5 - 2.0;
    // Angles in screen space, clockwise from the right. The rail spans 270 degrees.
    let (a0, a1) = if gap_right {
        (PI * 0.25, PI * 1.75)
    } else {
        (PI * 0.75, PI * 2.25)
    };
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = 48;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * rad
            })
            .collect()
    };
    p.add(Shape::line(arc(a0, a1), Stroke::new(4.0f32, KNOB_RAIL)));
    let col = if pressed { KNOB_PRESSED } else { fill };
    let at = a0 + (a1 - a0) * t.clamp(0.0, 1.0);
    let from = if centered { (a0 + a1) * 0.5 } else { a0 };
    if (at - from).abs() > 1e-3 {
        p.add(Shape::line(
            arc(from.min(at), from.max(at)),
            Stroke::new(4.0f32, col),
        ));
    }
    p.circle_filled(c, rad * 0.6, KNOB_RAIL);
    let d = vec2(at.cos(), at.sin());
    p.line_segment(
        [c + d * rad * 0.15, c + d * rad * 0.55],
        Stroke::new(2.0f32, lighten(KNOB_RAIL, 0x60)),
    );
}

/// The scratch clock (TScratchWheel paint 0x60690). `value` is parameter 8 (65536 = no offset),
/// `live` the live scratch position while playing.
pub fn scratch_clock(
    p: &Painter,
    r: Rect,
    value: f32,
    hand: Color32,
    pressed: bool,
    live: Option<f32>,
) {
    p.rect_filled(r, 0.0, Color32::BLACK);
    let inner = r.shrink(1.0);
    p.rect_filled(inner, 0.0, rgb(0x4e5962));
    let g = r.shrink(2.0);
    let n = 12;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let band = Rect::from_min_max(
            pos2(g.min.x, g.min.y + g.height() * t),
            pos2(g.max.x, g.min.y + g.height() * (t + 1.0 / n as f32)),
        );
        p.rect_filled(band, 0.0, mix(rgb(0x7c8790), rgb(0x4e5962), t));
    }
    let c = r.center();
    let s = r.width() / 72.0;
    p.circle_stroke(c, 30.0 * s, Stroke::new(1.0f32, rgb(0x848f96)));
    for k in 0..16 {
        let a = k as f32 * TAU / 16.0;
        let pt = c + vec2(a.cos(), a.sin()) * 32.0 * s;
        p.circle_filled(
            pt,
            1.0 * s,
            if k % 2 == 0 {
                Color32::WHITE
            } else {
                rgb(0x99a5ad)
            },
        );
    }
    let hand_col = if pressed { Color32::WHITE } else { hand };
    let angle = PI - value / 65536.0 * TAU;
    let d = vec2(angle.sin(), -angle.cos());
    p.line_segment([c, c + d * 25.0 * s], Stroke::new(1.5 * s, hand_col));
    p.circle_filled(c + d * 25.6 * s, 2.4 * s, hand_col);
    if let Some(l) = live {
        let a = PI - l / 65536.0 * TAU;
        let d = vec2(a.sin(), -a.cos());
        p.circle_stroke(c + d * 25.6 * s, 4.8 * s, Stroke::new(1.3 * s, hand));
    }
}

/// A vertical gradient fill of a convex path, approximated with horizontal strips clipped to
/// the path's bounding box.
pub fn gradient_rect(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let n = (r.height() / 2.0).ceil().max(1.0) as usize;
    for i in 0..n {
        let t0 = i as f32 / n as f32;
        let t1 = (i + 1) as f32 / n as f32;
        let band = Rect::from_min_max(
            pos2(r.min.x, r.min.y + r.height() * t0),
            pos2(r.max.x, r.min.y + r.height() * t1),
        );
        p.rect_filled(band, 0.0, mix(top, bottom, (t0 + t1) * 0.5));
    }
}
