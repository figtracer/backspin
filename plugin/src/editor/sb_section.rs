//! The shape section under the Gross Beat form (the per-band shapers): the preset header (Gross
//! Beat and shape presets), the mix knob, the shaper rack, the band tabs and the wave view. The painters are
//! `backspin_ui::sb`; this module builds their model and handles input on the same rects.

use super::Gui;
use super::menus::{load_preset, state_presets};
use super::paint::knob;
use backspin_gfx::Rect as DRect;
use backspin_shaperbox::state::State;
use backspin_shaperbox::wave::{LENGTHS, Point, rate_hz, render_table};
use backspin_ui::sb::ACCENT;
use backspin_ui::sb::layout;
use backspin_ui::{HEIGHT, SbModel, ShaperCell};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{
    self, AboveOrBelow, Id, PointerButton, PopupCloseBehavior, Rect, Response, ScrollArea, Sense,
    Ui, pos2,
};
use std::path::{Path, PathBuf};

/// The rack names in state order.
const NAMES: [&str; 16] = [
    "Time", "Crush", "Filter", "Pan", "Reverb", "Volume", "Delay", "Width", "Noise", "Drive",
    "Dynamics", "Liquid", "Scope", "Shift", "Limiter", "Pitch",
];

/// Shapers whose DSP is ported (the others pass audio through).
const PORTED: [usize; 6] = [0, 3, 5, 7, 8, 11];

const BANDS: [&str; 3] = ["low", "mid", "high"];

/// The section-local rect in editor coordinates.
fn at(r: DRect) -> Rect {
    Rect::from_min_max(
        pos2(r.x0 as f32, (r.y0 + HEIGHT) as f32),
        pos2(r.x1 as f32, (r.y1 + HEIGHT) as f32),
    )
}

/// Starts loading the preset library on a background thread on first use.
fn ensure_library(gui: &mut Gui) {
    if !gui.sb_loading && gui.sb_library.lock().is_none() {
        gui.sb_loading = true;
        let lib = gui.sb_library.clone();
        std::thread::spawn(move || *lib.lock() = Some(crate::sb::library()));
    }
}

/// The active bands of a shaper: the split puts the crossovers at the ends when a band is off.
fn bands_active(s: &State, k: usize) -> [bool; 3] {
    let sp = s.shapers[k].split;
    [sp.low > 0.0, true, sp.high < 1.0]
}

fn length_text(length: i32) -> String {
    let (n, d) = LENGTHS[length.clamp(0, LENGTHS.len() as i32 - 1) as usize];
    match d {
        1 if n == 1 => "1 bar".into(),
        1 => format!("{n} bars"),
        _ => format!("{n}/{d}"),
    }
}

/// The painters' model.
pub fn model(gui: &Gui) -> SbModel {
    let (enabled, state) = {
        let m = gui.shared.sb.model.lock();
        (m.enabled, m.tree.as_ref().map(State::from_tree))
    };
    let lib = gui.sb_library.lock();
    let pack = match lib.as_ref() {
        None => "loading...".to_string(),
        Some(l) if l.len() <= 1 && l.first().is_none_or(|p| p.presets.is_empty()) => {
            "not installed".to_string()
        }
        Some(l) => l
            .get(gui.sb_pack)
            .map(|p| p.short().to_string())
            .unwrap_or_default(),
    };
    let mut out = SbModel {
        enabled,
        gb_preset: gui.gb_preset.clone(),
        pack,
        // The library's name when the preset came from it, else the state's program name.
        preset: gui
            .sb_preset
            .as_ref()
            .and_then(|path| {
                let all = lib.as_ref()?.first()?;
                all.presets
                    .iter()
                    .find(|p| &p.path == path)
                    .map(|p| p.name.clone())
            })
            .or_else(|| state.as_ref().map(|s| s.name.clone()))
            .unwrap_or_else(|| "(none)".into()),
        selected: gui.sb_selected,
        band: gui.sb_band,
        bands_active: [false, true, false],
        ..Default::default()
    };
    out.shapers = (0..16)
        .map(|k| ShaperCell {
            name: NAMES[k].into(),
            visible: state.as_ref().is_some_and(|s| s.shapers[k].visible),
            on: state.as_ref().is_some_and(|s| s.shapers[k].on),
            ported: PORTED.contains(&k),
        })
        .collect();
    let Some(s) = state else {
        out.info = format!("{}: no preset loaded", NAMES[gui.sb_selected]);
        return out;
    };
    let k = gui.sb_selected;
    out.bands_active = bands_active(&s, k);
    let band = &s.shapers[k].bands[gui.sb_band];
    let mut info = format!(
        "{} \u{b7} {} band \u{b7} mix {:.0}%",
        NAMES[k],
        BANDS[gui.sb_band],
        band.mix * 100.0
    );
    if let Some(sec) = band.sections.first() {
        let lfo = &sec.lfo;
        if !lfo.points.is_empty() {
            out.table = render_table(&lfo.points);
            out.points = lfo
                .points
                .iter()
                .map(|p: &Point| (p.x as f32 / 8192.0, p.y))
                .collect();
        }
        let timing = if lfo.timing_mode == 1 {
            format!("{:.2} Hz", rate_hz(lfo.speed))
        } else {
            length_text(lfo.length)
        };
        info = format!("{info} \u{b7} {} \u{b7} {timing}", sec.id);
    }
    if !PORTED.contains(&k) {
        info.push_str(" \u{b7} not available yet");
    }
    out.info = info;
    out
}

/// A single-level popup list under `r`: returns the clicked index.
fn list_popup(
    ui: &Ui,
    r: &Response,
    id: Id,
    items: &[String],
    current: Option<usize>,
) -> Option<usize> {
    if r.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let mut picked = None;
    // Upward: the section is at the bottom of the window.
    let close = PopupCloseBehavior::CloseOnClickOutside;
    egui::popup_above_or_below_widget(ui, id, r, AboveOrBelow::Above, close, |ui| {
        ui.set_min_width(r.rect.width().max(200.0));
        ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (i, it) in items.iter().enumerate() {
                if ui.selectable_label(Some(i) == current, it).clicked() {
                    picked = Some(i);
                }
            }
        });
    });
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load_sb(gui: &mut Gui, path: &Path) {
    if gui.shared.sb.load_file(path).is_ok() {
        gui.sb_preset = Some(path.to_path_buf());
    }
}

/// The Gross Beat preset controls.
fn gb_presets(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter) {
    let list = state_presets();
    let names: Vec<String> = list.iter().map(|p| stem(p)).collect();
    let current = names.iter().position(|n| *n == gui.gb_preset);
    let r = ui.interact(
        at(layout::GB_PRESET),
        Id::new("sb_gb_preset"),
        Sense::click(),
    );
    if r.hovered() {
        gui.hint = "Gross Beat preset".into();
    }
    let mut pick = list_popup(ui, &r, Id::new("sb_gb_preset_popup"), &names, current);
    for (rect, step, id) in [
        (layout::GB_PREV, -1, "sb_gb_prev"),
        (layout::GB_NEXT, 1, "sb_gb_next"),
    ] {
        let r = ui.interact(at(rect), Id::new(id), Sense::click());
        if r.hovered() {
            gui.hint = if step < 0 {
                "Previous preset"
            } else {
                "Next preset"
            }
            .into();
        }
        if r.clicked() && !list.is_empty() {
            let n = list.len() as i32;
            pick = Some(current.map_or(0, |c| (c as i32 + step).rem_euclid(n)) as usize);
        }
    }
    if let Some(i) = pick {
        load_preset(gui, setter, &list[i]);
        gui.gb_preset = names[i].clone();
    }
}

/// The ShaperBox pack and preset controls and the section's power toggle.
fn sb_presets(gui: &mut Gui, ui: &mut Ui) {
    let lib = gui.sb_library.lock().clone().unwrap_or_default();
    let packs: Vec<String> = lib
        .iter()
        .map(|p| {
            format!(
                "{}  ({})",
                p.name.replace('@', " \u{203a} "),
                p.presets.len()
            )
        })
        .collect();
    let r = ui.interact(at(layout::PACK), Id::new("sb_pack"), Sense::click());
    if r.hovered() {
        gui.hint = "Shape preset pack".into();
    }
    if let Some(i) = list_popup(ui, &r, Id::new("sb_pack_popup"), &packs, Some(gui.sb_pack)) {
        gui.sb_pack = i;
    }
    let presets: &[crate::sb::Preset] = lib.get(gui.sb_pack).map_or(&[], |p| &p.presets);
    let names: Vec<String> = presets.iter().map(|p| p.name.clone()).collect();
    let current = presets
        .iter()
        .position(|p| Some(&p.path) == gui.sb_preset.as_ref());
    let r = ui.interact(at(layout::PRESET), Id::new("sb_preset"), Sense::click());
    if r.hovered() {
        gui.hint = "Shape preset".into();
    }
    let mut pick = list_popup(ui, &r, Id::new("sb_preset_popup"), &names, current);
    for (rect, step, id) in [(layout::PREV, -1, "sb_prev"), (layout::NEXT, 1, "sb_next")] {
        let r = ui.interact(at(rect), Id::new(id), Sense::click());
        if r.hovered() {
            gui.hint = if step < 0 {
                "Previous preset"
            } else {
                "Next preset"
            }
            .into();
        }
        if r.clicked() && !presets.is_empty() {
            let n = presets.len() as i32;
            pick = Some(current.map_or(0, |c| (c as i32 + step).rem_euclid(n)) as usize);
        }
    }
    if let Some(i) = pick {
        let path: PathBuf = presets[i].path.clone();
        load_sb(gui, &path);
    }
    let r = ui.interact(at(layout::POWER), Id::new("sb_power"), Sense::click());
    if r.hovered() {
        gui.hint = "Shapes on/off".into();
    }
    if r.clicked() {
        let (enabled, has_tree) = {
            let m = gui.shared.sb.model.lock();
            (m.enabled, m.tree.is_some())
        };
        if !has_tree {
            load_init(gui, &lib);
        } else {
            gui.shared.sb.set_enabled(!enabled);
        }
    }
}

/// Loads the library's "Init" preset (every shaper off) so there is a state to edit.
fn load_init(gui: &mut Gui, lib: &[crate::sb::Pack]) {
    let init = lib
        .first()
        .and_then(|p| p.presets.iter().find(|p| p.name == "Init"))
        .map(|p| p.path.clone());
    if let Some(p) = init {
        load_sb(gui, &p);
    }
}

/// The mix knob (the state's master mix): drag, Ctrl for finer steps, double-click for 100 %.
/// The processors are rebuilt when the drag ends.
fn mix_knob(gui: &mut Gui, ui: &mut Ui) {
    let r = at(layout::KNOB);
    let resp = ui.interact(r, Id::new("sb_mix"), Sense::click_and_drag());
    let mix = {
        let m = gui.shared.sb.model.lock();
        m.tree
            .as_ref()
            .map_or(1.0, |t| State::from_tree(t).master_mix)
    };
    if resp.drag_started_by(PointerButton::Primary) {
        gui.sb_mix_drag = Some((mix, 0.0));
    }
    if let Some((start, travel)) = gui.sb_mix_drag
        && resp.dragged_by(PointerButton::Primary)
    {
        let fine = ui.input(|i| i.modifiers.command);
        let travel = travel - resp.drag_delta().y / if fine { 16.0 } else { 1.0 };
        gui.sb_mix_drag = Some((start, travel));
        gui.shared
            .sb
            .set_mix((start + travel / 200.0).clamp(0.0, 1.0), false);
    }
    if resp.drag_stopped_by(PointerButton::Primary) && gui.sb_mix_drag.take().is_some() {
        gui.shared.sb.set_mix(mix, true);
    }
    if resp.double_clicked() {
        gui.shared.sb.set_mix(1.0, true);
    }
    if resp.hovered() {
        gui.hint = format!("Shape mix: {:.0}%", mix * 100.0);
    }
    knob(
        ui.painter(),
        r,
        mix,
        colour(ACCENT),
        false,
        true,
        gui.sb_mix_drag.is_some(),
    );
}

/// The rack, the band tabs and the shaper's on toggle.
fn rack(gui: &mut Gui, ui: &mut Ui) {
    let r = at(layout::RACK);
    let resp = ui.interact(r, Id::new("sb_rack"), Sense::click());
    let (cw, ch) = (
        r.width() / layout::COLS as f32,
        r.height() / layout::ROWS as f32,
    );
    let cell = |p: egui::Pos2| {
        let c = (((p.x - r.min.x) / cw) as usize).min(layout::COLS as usize - 1);
        let row = (((p.y - r.min.y) / ch) as usize).min(layout::ROWS as usize - 1);
        row * layout::COLS as usize + c
    };
    if let Some(p) = resp.hover_pos() {
        let k = cell(p);
        gui.hint = format!(
            "{}: click to select, double-click to turn on or off",
            NAMES[k]
        );
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        gui.select_shaper(cell(p));
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        toggle_shaper(gui, cell(p));
    }
    let b = at(layout::BANDS);
    let resp = ui.interact(b, Id::new("sb_bands"), Sense::click());
    if let Some(p) = resp.hover_pos() {
        let i = (((p.y - b.min.y) / 27.0) as usize).min(2);
        gui.hint = format!("Show the {} band", BANDS[i]);
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let i = (((p.y - b.min.y) / 27.0) as usize).min(2);
        let active = {
            let m = gui.shared.sb.model.lock();
            m.tree
                .as_ref()
                .map(|t| bands_active(&State::from_tree(t), gui.sb_selected))
        };
        if active.is_some_and(|a| a[i]) {
            gui.sb_band = i;
        }
    }
    let resp = ui.interact(
        at(layout::SHAPER_ON),
        Id::new("sb_shaper_on"),
        Sense::click(),
    );
    if resp.hovered() {
        gui.hint = format!("Turn {} on or off", NAMES[gui.sb_selected]);
    }
    if resp.clicked() {
        toggle_shaper(gui, gui.sb_selected);
    }
}

fn toggle_shaper(gui: &mut Gui, k: usize) {
    if gui.shared.sb.model.lock().tree.is_none() {
        let lib = gui.sb_library.lock().clone().unwrap_or_default();
        load_init(gui, &lib);
    }
    let on = {
        let m = gui.shared.sb.model.lock();
        m.tree.as_ref().is_some_and(|t| {
            let s = &State::from_tree(t).shapers[k];
            s.visible && s.on
        })
    };
    gui.shared.sb.set_shaper(k, !on);
    if !on && !gui.shared.sb.model.lock().enabled {
        gui.shared.sb.set_enabled(true);
    }
    gui.select_shaper(k);
}

/// Input for the whole section.
pub fn show(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter) {
    ensure_library(gui);
    gb_presets(gui, ui, setter);
    sb_presets(gui, ui);
    rack(gui, ui);
    mix_knob(gui, ui);
}

fn colour(c: u32) -> egui::Color32 {
    egui::Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}
