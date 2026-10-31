//! The shape section under the Gross Beat form: the shaper rack, Clamp's knobs and meter, the
//! shape bar with its mix knob, the band tabs, the wave view and the bottom row (the shape
//! preset, Clamp's toggle and combos). The painters are `backspin_ui::sb`; this module builds
//! their model, handles input on the same rects and draws the live parts (knobs, meter).

use super::Gui;
use super::paint::knob;
use crate::params::{CLAMP_MODES, CLAMP_OVERSAMPLING, CLAMP_TYPES};
use backspin_gfx::Rect as DRect;
use backspin_presets::{Group, Preset};
use backspin_shaperbox::state::State;
use backspin_shaperbox::wave::{LENGTHS, Point, rate_hz, render_table};
use backspin_ui::sb::{ACCENT, layout};
use backspin_ui::{HEIGHT, SbModel, ShaperCell};
use nih_plug::prelude::{FloatParam, IntParam, Param, ParamSetter};
use nih_plug_egui::egui::{
    self, AboveOrBelow, Color32, Id, PointerButton, PopupCloseBehavior, Rect, Response, ScrollArea,
    Sense, Ui, pos2,
};
use std::sync::atomic::Ordering;

/// The rack names in state order.
const NAMES: [&str; 16] = [
    "Time", "Crush", "Filter", "Pan", "Reverb", "Volume", "Delay", "Width", "Noise", "Drive",
    "Dynamics", "Liquid", "Scope", "Shift", "Limiter", "Pitch",
];

/// Shapers whose DSP is ported (the others pass audio through).
const PORTED: [usize; 10] = [0, 1, 2, 3, 4, 5, 7, 8, 9, 11];

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

/// The painters' model.
pub fn model(gui: &Gui) -> SbModel {
    let (enabled, state) = {
        let m = gui.shared.sb.model.lock();
        (m.enabled, m.tree.as_ref().map(State::from_tree))
    };
    let c = &gui.params.clamp;
    let item = |list: &[&str], v: i32| list[v.clamp(0, list.len() as i32 - 1) as usize].to_string();
    let mut out = SbModel {
        enabled,
        preset: preset_name(gui, state.as_ref()),
        selected: gui.sb_selected,
        band: gui.sb_band,
        bands_active: [false, true, false],
        clamp_on: c.on.value(),
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
        readout = format!("{readout} \u{b7} {timing}");
    }
    if !PORTED.contains(&k) {
        readout.push_str(" \u{b7} not available yet");
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
        ui.horizontal_top(|ui| {
            ScrollArea::vertical()
                .id_salt("sb_groups")
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.set_width(190.0);
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
            ui.separator();
            ScrollArea::vertical()
                .id_salt("sb_group_presets")
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.set_width(220.0);
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

/// The wave view: vertical drag sets the band's mix (Ctrl finer), the wheel steps the LFO length
/// through the sync divisions, double-click resets the mix to 100 %. The processors are rebuilt
/// when a drag ends.
fn wave_view(gui: &mut Gui, ui: &mut Ui) {
    let resp = ui.interact(
        at(layout::WAVE),
        Id::new("sb_wave"),
        Sense::click_and_drag(),
    );
    let (k, b) = (gui.sb_selected, gui.sb_band);
    let mix = {
        let m = gui.shared.sb.model.lock();
        m.tree
            .as_ref()
            .map(|t| State::from_tree(t).shapers[k].bands[b].mix)
    };
    let Some(mix) = mix else { return };
    if resp.drag_started_by(PointerButton::Primary) {
        gui.sb_band_drag = Some((mix, 0.0));
    }
    if let Some((start, travel)) = gui.sb_band_drag
        && resp.dragged_by(PointerButton::Primary)
    {
        let fine = ui.input(|i| i.modifiers.command);
        let travel = travel - resp.drag_delta().y / if fine { 16.0 } else { 1.0 };
        gui.sb_band_drag = Some((start, travel));
        let v = (start + travel / resp.rect.height()).clamp(0.0, 1.0);
        gui.shared.sb.set_band_mix(k, b, v, false);
    }
    if resp.drag_stopped_by(PointerButton::Primary) && gui.sb_band_drag.take().is_some() {
        gui.shared.sb.set_band_mix(k, b, mix, true);
    }
    if resp.double_clicked() {
        gui.shared.sb.set_band_mix(k, b, 1.0, true);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            gui.shared
                .sb
                .step_lfo_length(k, b, if scroll > 0.0 { 1 } else { -1 });
        }
        gui.hint = format!(
            "{} {} band mix {:.0}%: drag to change, wheel for the rate, double-click for 100%",
            NAMES[k],
            BANDS[b],
            mix * 100.0
        );
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
    on: bool,
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
    let fill = if on { colour(ACCENT) } else { colour(0x5d6b74) };
    // The gains are centred at 0 dB like the TNS knob; ceiling and softness fill from the left.
    let centred = p.preview_normalized(0.0) > 0.0 && p.preview_normalized(0.0) < 1.0;
    knob(ui.painter(), r, norm, fill, centred, false, resp.dragged());
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
        param_knob(gui, ui, setter, p, r, id, hint, step, on);
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

/// Input and live drawing for the whole section.
pub fn show(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter) {
    ensure_library(gui);
    presets(gui, ui);
    rack(gui, ui);
    wave_view(gui, ui);
    mix_knob(gui, ui);
    clamp(gui, ui, setter);
}
