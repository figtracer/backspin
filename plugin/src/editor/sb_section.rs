//! The shape section under the Gross Beat form: the shaper rack, Clamp's knobs and meter, the
//! shape bar with its mix knob, the band tabs, the wave view and the bottom row (the shape
//! preset, Clamp's toggle and combos). The painters are `backspin_ui::sb`; this module builds
//! their model, handles input on the same rects and draws the live parts (knobs, meter).

use super::Gui;
use crate::params::{CLAMP_MODES, CLAMP_OVERSAMPLING, CLAMP_TYPES};
use backspin_gfx::Rect as DRect;
use backspin_presets::{Group, Preset};
use backspin_shaperbox::state::State;
use backspin_shaperbox::wave::{LENGTHS, Point, TABLE, rate_hz, render_table};
use backspin_ui::sb::{ACCENT, WAVE_PAD, layout};
use backspin_ui::{HEIGHT, SbModel, ShaperCell};
use nih_plug::prelude::{FloatParam, IntParam, Param, ParamSetter};
use nih_plug_egui::egui::{
    self, AboveOrBelow, Color32, Id, PointerButton, PopupCloseBehavior, Pos2, Rect, Response,
    ScrollArea, Sense, Ui, pos2,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The rack names in state order.
const NAMES: [&str; 16] = [
    "Time", "Crush", "Filter", "Pan", "Reverb", "Volume", "Delay", "Width", "Noise", "Drive",
    "Dynamics", "Liquid", "Scope", "Shift", "Limiter", "Pitch",
];

/// Shapers with DSP. Scope only displays and Shift has no algorithm in ShaperBox 3 either;
/// both pass audio through, as there.
const PORTED: [usize; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 14, 15];

const BANDS: [&str; 3] = ["low", "mid", "high"];

/// The section-local rect in editor coordinates.
fn at(r: DRect) -> Rect {
    Rect::from_min_max(
        pos2(r.x0 as f32, (r.y0 + HEIGHT) as f32),
        pos2(r.x1 as f32, (r.y1 + HEIGHT) as f32),
    )
}

fn colour(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
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

/// The name of the loaded shape preset: the library's when it came from there, else the
/// state's program name.
fn preset_name(gui: &Gui, state: Option<&State>) -> String {
    let lib = gui.sb_library.lock();
    gui.sb_preset
        .as_ref()
        .and_then(|src| {
            let all = lib.as_ref()?.first()?;
            all.presets
                .iter()
                .find(|p| &p.source == src)
                .map(|p| p.name.clone())
        })
        .or_else(|| state.map(|s| s.name.clone()))
        .unwrap_or_else(|| "(none)".into())
}

/// What the section derives from the shape state, kept until the model's revision changes
/// (parsing the tree and rendering a wave table every frame cost most of the idle editor time).
#[derive(Default)]
pub struct SbCache {
    revision: u64,
    state: Option<Arc<State>>,
    /// The wave tables of the first LFOs shown so far, by shaper and band.
    tables: HashMap<(usize, usize), Arc<Vec<f32>>>,
}

/// The parsed shape state (`None` without a preset), parsed once per revision.
fn state(gui: &mut Gui) -> Option<Arc<State>> {
    let m = gui.shared.sb.model.lock();
    let c = &mut gui.sb_cache;
    if c.revision != m.revision || c.state.is_some() != m.tree.is_some() {
        *c = SbCache {
            revision: m.revision,
            state: m.tree.as_ref().map(|t| Arc::new(State::from_tree(t))),
            tables: HashMap::new(),
        };
    }
    c.state.clone()
}

/// The wave table of band `b`'s first LFO of shaper `k`, rendered once per revision.
fn table(gui: &mut Gui, k: usize, b: usize) -> Option<Arc<Vec<f32>>> {
    let s = state(gui)?;
    let points = &s.shapers[k].bands[b].sections.first()?.lfo.points;
    if points.is_empty() {
        return None;
    }
    let t = gui
        .sb_cache
        .tables
        .entry((k, b))
        .or_insert_with(|| Arc::new(render_table(points)));
    Some(t.clone())
}

/// The painters' model.
pub fn model(gui: &mut Gui) -> SbModel {
    let enabled = gui.shared.sb.model.lock().enabled;
    let state = state(gui);
    let wave = table(gui, gui.sb_selected, gui.sb_band);
    let c = &gui.params.clamp;
    let item = |list: &[&str], v: i32| list[v.clamp(0, list.len() as i32 - 1) as usize].to_string();
    let mut out = SbModel {
        enabled,
        preset: preset_name(gui, state.as_deref()),
        selected: gui.sb_selected,
        band: gui.sb_band,
        bands_active: [false, true, false],
        clamp_on: c.on.value(),
        clamp_knobs: [&c.input, &c.output, &c.ceiling, &c.shape]
            .map(|p| p.unmodulated_normalized_value()),
        clamp_type: ["A", "B"][c.kind.value().clamp(0, 1) as usize].into(),
        clamp_mode: item(&CLAMP_MODES, c.mode.value()),
        clamp_oversampling: item(&CLAMP_OVERSAMPLING, c.oversampling.value()),
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
    out.mix = state.as_ref().map_or(1.0, |s| s.master_mix);
    let Some(s) = state else {
        return out;
    };
    let k = gui.sb_selected;
    out.bands_active = bands_active(&s, k);
    let band = &s.shapers[k].bands[gui.sb_band];
    let mut readout = format!(
        "{} {} \u{b7} mix {:.0}%",
        NAMES[k],
        BANDS[gui.sb_band],
        band.mix * 100.0
    );
    if let Some(sec) = band.sections.first() {
        let lfo = &sec.lfo;
        if let Some(t) = &wave {
            out.table = t.to_vec();
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
        readout = format!("{readout} \u{b7} {timing}");
    }
    if !PORTED.contains(&k) {
        readout.push_str(" \u{b7} passes audio through, as in ShaperBox");
    }
    out.readout = readout;
    out
}

/// Opens or closes popup `id` on a click of `r` and shows it above `r` (the section is at the
/// bottom of the window); returns what `add` returns while it is open.
fn popup_above<R>(ui: &Ui, r: &Response, id: Id, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    if r.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let close = PopupCloseBehavior::CloseOnClickOutside;
    egui::popup_above_or_below_widget(ui, id, r, AboveOrBelow::Above, close, add)
}

/// A single-level list popup: returns the clicked index.
fn list_popup(ui: &Ui, r: &Response, id: Id, items: &[&str], current: usize) -> Option<usize> {
    let picked = popup_above(ui, r, id, |ui| {
        ui.set_min_width(r.rect.width().max(90.0));
        let mut picked = None;
        for (i, it) in items.iter().enumerate() {
            if ui.selectable_label(i == current, *it).clicked() {
                picked = Some(i);
            }
        }
        picked
    })
    .flatten();
    if picked.is_some() {
        ui.memory_mut(|m| m.close_popup());
    }
    picked
}

fn load_sb(gui: &mut Gui, preset: &Preset) {
    if let Some(data) = preset.bytes()
        && gui.shared.sb.load(&data).is_ok()
    {
        gui.sb_preset = Some(preset.source.clone());
    }
}

/// Loads the library's "Init" preset (every shaper off) so there is a state to edit.
fn load_init(gui: &mut Gui, lib: &[Group]) {
    let init = lib
        .first()
        .and_then(|p| p.presets.iter().find(|p| p.name == "Init"))
        .cloned();
    if let Some(p) = init {
        load_sb(gui, &p);
    }
}

/// The shape preset: a two-column picker (groups, then the group's presets) above the combo,
/// with previous and next stepping through the shown group.
fn presets(gui: &mut Gui, ui: &mut Ui) {
    let lib = gui.sb_library.lock().clone().unwrap_or_default();
    let r = ui.interact(at(layout::PRESET), Id::new("sb_preset"), Sense::click());
    if r.hovered() {
        gui.hint = "Shape preset".into();
    }
    let group = gui.sb_pack.min(lib.len().saturating_sub(1));
    let shown = lib.get(group).and_then(|g| {
        g.presets
            .iter()
            .position(|p| Some(&p.source) == gui.sb_preset.as_ref())
    });
    let mut pick: Option<(usize, usize)> = None;
    let mut show_group = None;
    popup_above(ui, &r, Id::new("sb_preset_popup"), |ui| {
        // Two columns side by side; each lists its items top to bottom (a list directly inside
        // the horizontal row would inherit its left-to-right layout).
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(200.0);
                ScrollArea::vertical()
                    .id_salt("sb_groups")
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            for (g, grp) in lib.iter().enumerate() {
                                let label = format!(
                                    "{}  ({})",
                                    grp.name.replace('@', " \u{203a} "),
                                    grp.presets.len()
                                );
                                if ui.selectable_label(g == group, label).clicked() {
                                    show_group = Some(g);
                                }
                            }
                        });
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(230.0);
                ScrollArea::vertical()
                    .id_salt("sb_group_presets")
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            if let Some(grp) = lib.get(group) {
                                for (i, p) in grp.presets.iter().enumerate() {
                                    if ui.selectable_label(Some(i) == shown, &p.name).clicked() {
                                        pick = Some((group, i));
                                    }
                                }
                            }
                        });
                    });
            });
        });
    });
    if let Some(g) = show_group {
        gui.sb_pack = g;
    }
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
        if r.clicked()
            && let Some(grp) = lib.get(group)
            && !grp.presets.is_empty()
        {
            let n = grp.presets.len() as i32;
            pick = Some((
                group,
                shown.map_or(0, |c| (c as i32 + step).rem_euclid(n)) as usize,
            ));
        }
    }
    if let Some((g, i)) = pick {
        ui.memory_mut(|m| m.close_popup());
        let preset = lib[g].presets[i].clone();
        load_sb(gui, &preset);
    }
}

/// The mix knob (the state's master mix): drag, Ctrl for finer steps, double-click for 100 %.
/// The processors are rebuilt when the drag ends.
fn mix_knob(gui: &mut Gui, ui: &mut Ui) {
    let r = at(layout::KNOB);
    let resp = ui.interact(r, Id::new("sb_mix"), Sense::click_and_drag());
    let mix = state(gui).map_or(1.0, |s| s.master_mix);
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
    // The frame paints the knob, like the time and volume Mix knobs.
}

/// The wave view. On a point: drag moves it (Ctrl finer in `y`), double-click deletes it and
/// right-click toggles a soft point and a corner. Elsewhere: vertical drag sets the band's mix
/// and double-click adds a corner point. The wheel steps the LFO length through the sync
/// divisions. The processors are rebuilt when a drag ends.
fn wave_view(gui: &mut Gui, ui: &mut Ui) {
    let resp = ui.interact(
        at(layout::WAVE),
        Id::new("sb_wave"),
        Sense::click_and_drag(),
    );
    let (k, b) = (gui.sb_selected, gui.sb_band);
    let band = state(gui).map(|s| {
        let band = &s.shapers[k].bands[b];
        let points = band.sections.first().map(|s| s.lfo.points.clone());
        (band.mix, points)
    });
    let Some((mix, points)) = band else { return };
    let wave = table(gui, k, b);
    let r = resp.rect;
    // The painter's mapping: `y` runs from +1 at the top inset to -1 at the bottom inset.
    let (fw, fh) = (r.width() - 1.0, r.height() - 1.0 - 2.0 * WAVE_PAD);
    // The point whose marker (drawn on the curve, as the painter does) is under `pos`.
    let hit = |pos: Pos2| {
        let points = points.as_deref()?;
        let table = wave.as_deref()?;
        let n = table.len().max(1);
        points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let px = p.x as f32 / TABLE as f32;
                let v = table.get(((px * n as f32) as usize).min(n - 1)).copied();
                let y = WAVE_PAD + fh * (1.0 - (v.unwrap_or(p.y) + 1.0) * 0.5);
                (i, pos.distance(pos2(r.min.x + px * fw, r.min.y + y)))
            })
            .filter(|&(_, d)| d <= 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    if resp.drag_started_by(PointerButton::Primary) {
        let origin = ui.input(|i| i.pointer.press_origin());
        match origin.and_then(hit).zip(points.as_ref()) {
            Some((i, pts)) => gui.sb_point_drag = Some((i, pts[i].y, 0.0)),
            None => gui.sb_band_drag = Some((mix, 0.0)),
        }
    }
    let fine = ui.input(|i| i.modifiers.command);
    if let Some((i, y0, travel)) = gui.sb_point_drag
        && resp.dragged_by(PointerButton::Primary)
        && let Some(mut pts) = points.clone()
        && let Some(pos) = resp.interact_pointer_pos()
        && i < pts.len()
    {
        let travel = travel - resp.drag_delta().y / if fine { 16.0 } else { 1.0 };
        gui.sb_point_drag = Some((i, y0, travel));
        // A point stays between its neighbours, so the indices keep their order.
        let lo = if i == 0 { 0 } else { pts[i - 1].x + 1 };
        let hi = pts.get(i + 1).map_or(TABLE as i32 - 1, |p| p.x - 1).max(lo);
        let x = ((pos.x - r.min.x) / fw * TABLE as f32).round() as i32;
        pts[i].x = x.clamp(lo, hi);
        pts[i].y = (y0 + travel * 2.0 / fh).clamp(-1.0, 1.0);
        gui.shared.sb.set_lfo_points(k, b, &pts, false);
    }
    if resp.drag_stopped_by(PointerButton::Primary)
        && gui.sb_point_drag.take().is_some()
        && let Some(pts) = &points
    {
        gui.shared.sb.set_lfo_points(k, b, pts, true);
    }
    if let Some((start, travel)) = gui.sb_band_drag
        && resp.dragged_by(PointerButton::Primary)
    {
        let travel = travel - resp.drag_delta().y / if fine { 16.0 } else { 1.0 };
        gui.sb_band_drag = Some((start, travel));
        let v = (start + travel / resp.rect.height()).clamp(0.0, 1.0);
        gui.shared.sb.set_band_mix(k, b, v, false);
    }
    if resp.drag_stopped_by(PointerButton::Primary) && gui.sb_band_drag.take().is_some() {
        gui.shared.sb.set_band_mix(k, b, mix, true);
    }
    let pos = resp.interact_pointer_pos().or(resp.hover_pos());
    let under = pos.and_then(hit);
    if let Some(mut pts) = points.clone()
        && let Some(pos) = pos
    {
        if resp.double_clicked() {
            match under {
                // The wave keeps at least one point (a constant).
                Some(i) if pts.len() > 1 => {
                    pts.remove(i);
                    gui.shared.sb.set_lfo_points(k, b, &pts, true);
                }
                Some(_) => {}
                None => {
                    let x = (((pos.x - r.min.x) / fw * TABLE as f32).round() as i32)
                        .clamp(0, TABLE as i32 - 1);
                    let y = (1.0 - 2.0 * (pos.y - r.min.y - WAVE_PAD) / fh).clamp(-1.0, 1.0);
                    let at = pts.partition_point(|p| p.x < x);
                    if pts.get(at).is_none_or(|p| p.x != x) {
                        // A corner point: the curve passes through where the click was.
                        pts.insert(at, Point { x, y, factor: 3 });
                        gui.shared.sb.set_lfo_points(k, b, &pts, true);
                    }
                }
            }
        } else if resp.secondary_clicked()
            && let Some(i) = under
        {
            pts[i].factor = if pts[i].factor >= 3 { 1 } else { 3 };
            gui.shared.sb.set_lfo_points(k, b, &pts, true);
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            gui.shared
                .sb
                .step_lfo_length(k, b, if scroll > 0.0 { 1 } else { -1 });
        }
        gui.hint = if under.is_some() || gui.sb_point_drag.is_some() {
            "Point: drag to move, double-click to delete, right-click for a soft or sharp \
             corner"
                .into()
        } else {
            format!(
                "{} {} band mix {:.0}%: drag to change, double-click to add a point, wheel for \
                 the rate",
                NAMES[k],
                BANDS[b],
                mix * 100.0
            )
        };
    }
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
        gui.hint = format!(
            "{}: click to select, double-click to turn on or off",
            NAMES[cell(p)]
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
    let band_at = |p: egui::Pos2| (((p.y - b.min.y) / 27.0) as usize).min(2);
    if let Some(p) = resp.hover_pos() {
        gui.hint = format!("Show the {} band", BANDS[band_at(p)]);
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let i = band_at(p);
        let k = gui.sb_selected;
        let active = state(gui).map(|s| bands_active(&s, k));
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
    let on = state(gui).is_some_and(|s| s.shapers[k].visible && s.shapers[k].on);
    gui.shared.sb.set_shaper(k, !on);
    if !on && !gui.shared.sb.model.lock().enabled {
        gui.shared.sb.set_enabled(true);
    }
    gui.select_shaper(k);
}

/// A knob bound to a float parameter: vertical drag (Ctrl finer), wheel, double-click resets.
#[allow(clippy::too_many_arguments)]
fn param_knob(
    gui: &mut Gui,
    ui: &mut Ui,
    setter: &ParamSetter,
    p: &FloatParam,
    r: DRect,
    id: &str,
    hint: &str,
    step: f32,
) {
    let r = at(r);
    let resp = ui.interact(r, Id::new(id), Sense::click_and_drag());
    let norm = p.unmodulated_normalized_value();
    if resp.drag_started_by(PointerButton::Primary) {
        setter.begin_set_parameter(p);
    }
    if resp.dragged_by(PointerButton::Primary) {
        let fine = ui.input(|i| i.modifiers.command);
        let d = -resp.drag_delta().y / if fine { 16.0 } else { 1.0 } / 200.0;
        setter.set_parameter_normalized(p, (norm + d).clamp(0.0, 1.0));
    }
    if resp.drag_stopped_by(PointerButton::Primary) {
        setter.end_set_parameter(p);
    }
    if resp.double_clicked() {
        setter.begin_set_parameter(p);
        setter.set_parameter(p, p.default_plain_value());
        setter.end_set_parameter(p);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let v = p.value() + if scroll > 0.0 { step } else { -step };
            setter.begin_set_parameter(p);
            setter.set_parameter(p, v);
            setter.end_set_parameter(p);
        }
        gui.hint = format!("{hint}: {}", p.normalized_value_to_string(norm, true));
    }
    // The frame paints the knob, like the form's small knobs.
}

/// A combo bound to an int parameter, with its items in a popup above it.
#[allow(clippy::too_many_arguments)]
fn param_combo(
    gui: &mut Gui,
    ui: &mut Ui,
    setter: &ParamSetter,
    p: &IntParam,
    r: DRect,
    id: &str,
    hint: &str,
    items: &[&str],
) {
    let resp = ui.interact(at(r), Id::new(id), Sense::click());
    let last = items.len() as i32 - 1;
    if resp.hovered() {
        gui.hint = hint.into();
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let v = (p.value() + if scroll > 0.0 { -1 } else { 1 }).clamp(0, last);
            setter.begin_set_parameter(p);
            setter.set_parameter(p, v);
            setter.end_set_parameter(p);
        }
    }
    let current = p.value().clamp(0, last) as usize;
    if let Some(i) = list_popup(ui, &resp, Id::new((id, "popup")), items, current) {
        setter.begin_set_parameter(p);
        setter.set_parameter(p, i as i32);
        setter.end_set_parameter(p);
    }
}

/// A level as the meter's fill fraction: -60 dB to +6 dB.
fn meter_fraction(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    ((20.0 * level.log10() + 60.0) / 66.0).clamp(0.0, 1.0)
}

/// Clamp: the knobs, the meter, the toggle and the combos.
fn clamp(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter) {
    let params = gui.params.clone();
    let c = &params.clamp;
    let on = c.on.value();
    let knobs = [
        (&c.input, "clamp_in", "Clamp input gain", 0.5),
        (&c.output, "clamp_out", "Clamp output gain", 0.5),
        (&c.ceiling, "clamp_ceiling", "Clamp ceiling", 0.5),
        (&c.shape, "clamp_shape", "Clamp softness (shape)", 0.05),
    ];
    for ((p, id, hint, step), r) in knobs.into_iter().zip(layout::CLAMP_KNOBS) {
        param_knob(gui, ui, setter, p, r, id, hint, step);
    }
    let meter = &gui.shared.status.clamp_meter;
    let levels = [
        f32::from_bits(meter[1].load(Ordering::Relaxed)),
        f32::from_bits(meter[0].load(Ordering::Relaxed)),
    ];
    for (r, level) in [layout::METER_IN, layout::METER_OUT]
        .into_iter()
        .zip(levels)
    {
        let r = at(r);
        let f = if on { meter_fraction(level) } else { 0.0 };
        if f > 0.0 {
            let bar = Rect::from_min_max(pos2(r.min.x, r.max.y - r.height() * f), r.max);
            ui.painter().rect_filled(bar, 0.0, colour(ACCENT));
        }
    }
    if on {
        let ceiling = meter_fraction(10f32.powf(c.ceiling.value() / 20.0));
        let r = at(layout::METER_OUT);
        let y = r.max.y - r.height() * ceiling;
        ui.painter().line_segment(
            [pos2(r.min.x - 1.0, y), pos2(r.max.x + 1.0, y)],
            (1.0, colour(0xffb870)),
        );
    }
    let resp = ui.interact(at(layout::METER), Id::new("clamp_meter"), Sense::hover());
    if resp.hovered() {
        let db = |l: f32| {
            if l > 0.0 {
                format!("{:.1} dB", 20.0 * l.log10())
            } else {
                "-inf".into()
            }
        };
        gui.hint = format!("Clamp level: in {}, out {}", db(levels[0]), db(levels[1]));
    }
    let resp = ui.interact(at(layout::CLAMP), Id::new("clamp_on"), Sense::click());
    if resp.hovered() {
        gui.hint = "Clamp clipper on/off (the last stage)".into();
    }
    if resp.clicked() {
        setter.begin_set_parameter(&c.on);
        setter.set_parameter(&c.on, !on);
        setter.end_set_parameter(&c.on);
    }
    param_combo(
        gui,
        ui,
        setter,
        &c.kind,
        layout::TYPE,
        "clamp_type",
        "Clamp type: A saturates into a hard clip, B is a soft-knee clipper",
        &CLAMP_TYPES,
    );
    param_combo(
        gui,
        ui,
        setter,
        &c.mode,
        layout::MODE,
        "clamp_mode",
        "Clamp channels: left/right or mid/side",
        &CLAMP_MODES,
    );
    param_combo(
        gui,
        ui,
        setter,
        &c.oversampling,
        layout::OVERSAMPLING,
        "clamp_os",
        "Clamp oversampling",
        &CLAMP_OVERSAMPLING,
    );
}

/// The "shape" pill turns the whole section on or off (its caption dims while it is off).
fn pill(gui: &mut Gui, ui: &mut Ui) {
    // The pill starts under the knob; the knob keeps its drags.
    let mut r = at(layout::PILL);
    r.min.x = at(layout::KNOB).max.x;
    let resp = ui.interact(r, Id::new("sb_pill"), Sense::click());
    let (enabled, loaded) = {
        let m = gui.shared.sb.model.lock();
        (m.enabled, m.tree.is_some())
    };
    if resp.hovered() {
        gui.hint = format!(
            "Shape: {}, click to turn {}",
            ["off", "on"][enabled as usize],
            ["on", "off"][enabled as usize]
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        if !loaded {
            let lib = gui.sb_library.lock().clone().unwrap_or_default();
            load_init(gui, &lib);
        }
        gui.shared.sb.set_enabled(!enabled || !loaded);
    }
}

/// Input and live drawing for the whole section.
pub fn show(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter) {
    ensure_library(gui);
    presets(gui, ui);
    rack(gui, ui);
    wave_view(gui, ui);
    mix_knob(gui, ui);
    pill(gui, ui);
    clamp(gui, ui, setter);
}
