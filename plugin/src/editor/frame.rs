//! The code-exact frame ([`backspin_ui::paint`]) shown as the editor's background texture.
//!
//! Text goes through an interim overlay that draws egui text and the vector glyphs at the rects
//! the painters hand over, until the QuickFontCache port replaces it.

use super::paint::{Glyph, font, glyph};
use super::{Gui, HOLD_ITEMS, SYNC_ITEMS};
use backspin_gfx::{Dib, Rect as DRect};
use backspin_ui::{Font, HEIGHT, Model, PANEL, SB_HEIGHT, Text, WIDTH, paint, paint_sb};
use nih_plug_egui::egui::{
    self, Align2, Color32, ColorImage, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2,
};
use std::sync::atomic::Ordering;

/// One text draw of the painters, in form coordinates.
struct Item {
    font: Font,
    text: String,
    rect: DRect,
    flags: u32,
    colour: u32,
}

/// Records the painters' text draws.
#[derive(Default)]
struct Overlay(Vec<Item>);

impl Text for Overlay {
    fn draw(
        &mut self,
        dib: &mut Dib,
        font: Font,
        text: &str,
        r: DRect,
        flags: u32,
        colour: u32,
        _: i32,
    ) {
        let (ox, oy) = dib.origin();
        self.0.push(Item {
            font,
            text: text.to_string(),
            rect: r.offset(ox, oy),
            flags,
            colour,
        });
    }
}

/// The editor state as the painters see it.
fn model(gui: &Gui) -> Model {
    let group = gui.group();
    let status = &gui.shared.status;
    let playing = status.playing.load(Ordering::Relaxed);
    let m = gui.shared.model.lock();
    let mut out = Model {
        names: [0, 1].map(|b| m.banks[b].slots.iter().map(|s| s.name.clone()).collect()),
        empty: [0, 1].map(|b| m.banks[b].slots.iter().map(|s| s.empty).collect()),
        selected: [m.params[0], m.params[1]],
        edited: gui.edited,
        group,
        snap: gui.snap,
        step: gui.step,
        slide: gui.slide,
        freeze: gui.freeze,
        clock: m.view_scratch != 0,
        ..Default::default()
    };
    // The progress bar: four beats over the cell width (FUN_00066a30 keeps it in +0x1fc).
    let cw = 248 / 4;
    for b in 0..2 {
        out.playing[b] = if playing {
            status.active[b].load(Ordering::Relaxed)
        } else {
            -1
        };
        let ppb = (cw - 1) as f64 / 4.0;
        out.bar_px[b] = ((status.position(b) * ppb) as i32).clamp(0, cw - 1);
    }
    let slot = &m.banks[group].slots[gui.edited[group]];
    out.link = slot.link != 0;
    out.key_held = slot.key_held != 0;
    out.one_shot = slot.one_shot != 0;
    let item = |items: &[&str], v: i32| {
        items
            .get(v.max(0) as usize)
            .copied()
            .unwrap_or("?")
            .to_string()
    };
    out.combos = [
        item(&HOLD_ITEMS, slot.force_hold),
        item(&SYNC_ITEMS, slot.trigger_sync),
        item(&SYNC_ITEMS, slot.position_sync),
    ];
    out
}

/// The vector glyph for an IL glyph code point.
fn il_glyph(text: &str) -> Option<Glyph> {
    Some(match text {
        "\u{f08b}" => Glyph::Menu,
        "C" => Glyph::Step,
        "\u{f0c3}" => Glyph::Slide,
        "\u{f07e}" => Glyph::Freeze,
        "\u{f094}" => Glyph::TimeMap,
        "\u{f0a0}" => Glyph::VolumeMap,
        "\u{f054}" => Glyph::Link,
        "\u{f014}" => Glyph::KeyHeld,
        "1" => Glyph::OneShot,
        "\u{f05f}" => Glyph::Down,
        "\u{f463}" => Glyph::Clock,
        "\u{f0b8}" => Glyph::Speaker,
        backspin_ui::sb::GLYPH_WAVE => Glyph::Wave,
        backspin_ui::sb::GLYPH_PREV => Glyph::Prev,
        backspin_ui::sb::GLYPH_NEXT => Glyph::Next,
        backspin_ui::sb::GLYPH_POWER => Glyph::Power,
        _ => return None,
    })
}

fn colour(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Draws one text item with egui: horizontal alignment from the low two flag bits (0 left,
/// 1 right, 2 centre), vertically centred with flag 0x8, else at the top.
fn draw_item(p: &Painter, it: &Item) {
    let r = Rect::from_min_max(
        pos2(it.rect.x0 as f32, it.rect.y0 as f32),
        pos2(it.rect.x1 as f32, it.rect.y1 as f32),
    );
    let c = colour(it.colour);
    let vcentre = it.flags & 0x8 != 0;
    if matches!(it.font, Font::ButtonGlyph | Font::MapType | Font::IlGlyphs)
        && let Some(g) = il_glyph(&it.text)
    {
        let size = if matches!(it.font, Font::IlGlyphs) {
            22.0
        } else {
            13.0
        };
        let cx = match it.flags & 3 {
            1 => r.max.x - size * 0.5 + 4.0,
            _ => r.center().x,
        };
        let cy = if vcentre {
            r.center().y
        } else {
            r.min.y + 12.0
        };
        glyph(
            p,
            Rect::from_center_size(pos2(cx, cy), vec2(size, size)),
            g,
            c,
        );
        return;
    }
    let size = match it.font {
        Font::SlotCell | Font::ComboLabel => 9.5,
        Font::Snap => 9.0,
        Font::ComboValue => 10.0,
        Font::Caption => 15.0,
        _ => 11.0,
    };
    let (x, ha) = match it.flags & 3 {
        1 => (r.max.x, egui::Align::Max),
        2 => (r.center().x, egui::Align::Center),
        _ => (r.min.x, egui::Align::Min),
    };
    let (y, va) = if vcentre {
        (r.center().y, egui::Align::Center)
    } else {
        (r.min.y, egui::Align::Min)
    };
    p.with_clip_rect(r.expand(2.0))
        .text(pos2(x, y), Align2([ha, va]), &it.text, font(size), c);
}

/// Paints the frame and the text overlay (every egui frame; the bitmap is rebuilt each time).
pub fn show(gui: &mut Gui, ui: &egui::Ui, tex: &mut Option<TextureHandle>) {
    let mut dib = Dib::new(WIDTH, HEIGHT + SB_HEIGHT, PANEL);
    let mut overlay = Overlay::default();
    paint(&mut dib, &model(gui), &mut overlay);
    dib.move_origin(0, HEIGHT);
    paint_sb(&mut dib, &super::sb_section::model(gui), &mut overlay);
    dib.move_origin(0, -HEIGHT);
    let rgba: Vec<u8> = dib
        .pixels()
        .iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 0xff])
        .collect();
    let image =
        ColorImage::from_rgba_unmultiplied([WIDTH as usize, (HEIGHT + SB_HEIGHT) as usize], &rgba);
    let t = tex.get_or_insert_with(|| {
        ui.ctx()
            .load_texture("backspin-frame", image.clone(), TextureOptions::NEAREST)
    });
    t.set(image, TextureOptions::NEAREST);
    let p = ui.painter();
    let full = Rect::from_min_size(
        pos2(0.0, 0.0),
        vec2(WIDTH as f32, (HEIGHT + SB_HEIGHT) as f32),
    );
    p.image(
        t.id(),
        full,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    for it in &overlay.0 {
        draw_item(p, it);
    }
}
