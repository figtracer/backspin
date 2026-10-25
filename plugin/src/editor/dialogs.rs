//! The dialogs: tool dialogs with live preview, the rename and value prompts, message boxes and
//! About.
//!
//! A tool dialog previews on the slot itself, like the original: every change restores the
//! envelope saved when the dialog opened and runs the tool again. Accept keeps the result,
//! closing the dialog any other way restores the saved envelope.

use super::Gui;
use super::menus::GB_DIR;
use super::paint::*;
use backspin_engine::envelope::tools::{
    FilterSettings, SEQ_FIELDS, ScaleSettings, SeqSettings, SeqStep, SmoothSettings, ToolError,
    default_steps, parse_value, value_text,
};
use backspin_engine::envelope::{Envelope, Point};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{
    self, Color32, ColorImage, Context, Id, PointerButton, Rect, Sense, Slider, TextureHandle, Ui,
    pos2, vec2,
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Scale,
    Decimate,
    Filter,
    Smooth,
    Sequence,
}

/// An open tool dialog: the slot it edits and the envelope saved when it opened.
pub struct ToolDialog {
    kind: ToolKind,
    bank: usize,
    index: usize,
    saved: Envelope,
}

/// Every dialog's state, and the tool settings the original keeps in the registry.
pub struct Dialogs {
    pub message: Option<ToolError>,
    rename: Option<(usize, usize, String)>,
    value: Option<(usize, usize, usize, String)>,
    tool: Option<ToolDialog>,
    pub about: bool,
    about_image: Option<TextureHandle>,
    scale: ScaleSettings,
    decimate: i32,
    smooth: SmoothSettings,
    filter: FilterSettings,
    kernel: Envelope,
    steps: [SeqStep; 32],
    seq: SeqSettings,
    seq_field: usize,
    rng: u64,
}

impl Default for Dialogs {
    fn default() -> Self {
        Self {
            message: None,
            rename: None,
            value: None,
            tool: None,
            about: false,
            about_image: None,
            scale: ScaleSettings::default(),
            decimate: 500,
            smooth: SmoothSettings {
                link: true,
                ..SmoothSettings::default()
            },
            filter: FilterSettings::default(),
            kernel: default_kernel(),
            steps: default_steps(),
            seq: SeqSettings::default(),
            seq_field: 0,
            rng: 0x2545_f491_4f6c_dd1d,
        }
    }
}

/// The Filter kernel: `Env filter - default.fnv` from the Maps folder, or its points.
fn default_kernel() -> Envelope {
    let mut k = Envelope::with_points(
        1.0,
        vec![
            Point::new(0.0, 0.0, 0.0, 0),
            Point::new(0.5, 1.0, 0.0, 0),
            Point::new(0.5, 0.0, 0.0, 0),
        ],
    );
    if let Ok(data) =
        std::fs::read(PathBuf::from(GB_DIR).join("Data/Maps/Env filter - default.fnv"))
    {
        k.load_any(&data);
    }
    k
}

pub fn not_continuous(tool: &str) -> ToolError {
    ToolError {
        title: "Can't decimate",
        message: format!(
            "The selection must have at least two points and be continuous for the {tool} tool to work."
        ),
    }
}

impl Dialogs {
    pub fn rename(&mut self, bank: usize, index: usize, name: String) {
        self.rename = Some((bank, index, name));
    }

    pub fn type_value(&mut self, bank: usize, index: usize, point: usize, v: f32) {
        self.value = Some((bank, index, point, value_text(v)));
    }

    pub fn open_tool(&mut self, kind: ToolKind, bank: usize, index: usize, env: &Envelope) {
        self.tool = Some(ToolDialog {
            kind,
            bank,
            index,
            saved: env.clone(),
        });
    }

    /// A random number in `0..n` for the Create sequence randomizers (the original uses FPC
    /// `Random`, which cannot be reproduced; SPEC.md decision 10).
    fn random(&mut self, n: u64) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng % n.max(1)
    }
}

/// Shows the open dialogs.
pub fn show(gui: &mut Gui, ctx: &Context, setter: &ParamSetter) {
    if let Some(e) = gui.dialogs.message.clone() {
        let mut open = true;
        egui::Window::new(e.title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(&e.message);
                if ui.button("OK").clicked() {
                    open = false;
                }
            });
        if !open {
            gui.dialogs.message = None;
        }
    }
    rename_prompt(gui, ctx);
    value_prompt(gui, ctx);
    tool_dialog(gui, ctx);
    about(gui, ctx);
    let _ = setter;
}

/// The rename prompt (FUN_000642d0): caption "Slot N name".
fn rename_prompt(gui: &mut Gui, ctx: &Context) {
    let Some((bank, index, mut text)) = gui.dialogs.rename.take() else {
        return;
    };
    let mut keep = true;
    let mut ok = false;
    egui::Window::new(format!("Slot {} name", index + 1))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
        });
    if ok {
        gui.edit_slot(bank, index, false, |s| s.name = text);
    } else if keep {
        gui.dialogs.rename = Some((bank, index, text));
    }
}

/// Type in value (0xb7240): "Value for automation point".
fn value_prompt(gui: &mut Gui, ctx: &Context) {
    let Some((bank, index, point, mut text)) = gui.dialogs.value.take() else {
        return;
    };
    let mut keep = true;
    let mut ok = false;
    egui::Window::new("Value for automation point")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    keep = false;
                }
            });
        });
    if ok {
        let env = gui.shared.model.lock().banks[bank].slots[index].env.clone();
        let old = env.points.get(point).map_or(0.0, |p| p.value as f64);
        let v = parse_value(&text, true).map_or(old, |v| v.clamp(0.0, 1.0));
        if env.selected_count() > 1 || v != old {
            gui.edit_slot(bank, index, false, |s| s.env.set_selected_values(v));
        }
    } else if keep {
        gui.dialogs.value = Some((bank, index, point, text));
    }
}

/// A labelled slider row.
fn slider(ui: &mut Ui, label: &str, v: &mut i32, lo: i32, hi: i32) -> bool {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(80.0, 18.0), egui::Label::new(label));
        ui.add(Slider::new(v, lo..=hi).show_value(true)).changed()
    })
    .inner
}

/// The tool dialogs: Level scaling, Decimate, Filter, Smooth up, Create sequence.
fn tool_dialog(gui: &mut Gui, ctx: &Context) {
    let Some(t) = gui.dialogs.tool.take() else {
        return;
    };
    let title = match t.kind {
        ToolKind::Scale => "Level scaling",
        ToolKind::Decimate => "Decimate",
        ToolKind::Filter => "Filter",
        ToolKind::Smooth => "Smooth up",
        ToolKind::Sequence => "Create sequence",
    };
    let mut changed = false;
    let mut accept = false;
    let mut cancel = false;
    let mut reset = false;
    let mut first = false;
    let d = &mut gui.dialogs;
    egui::Window::new(title)
        .id(Id::new(("tool", title)))
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            match t.kind {
                ToolKind::Scale => {
                    let s = &mut d.scale;
                    changed |= slider(ui, "Center", &mut s.center, 0, 8192);
                    changed |= slider(ui, "Multiply", &mut s.multiply, -16384, 16384);
                    changed |= slider(ui, "Offset", &mut s.offset, -8192, 8192);
                    changed |= slider(ui, "Tension", &mut s.tension, -8192, 8192);
                }
                ToolKind::Decimate => {
                    changed |= slider(ui, "Threshold", &mut d.decimate, 0, 1000);
                }
                ToolKind::Filter => {
                    let s = &mut d.filter;
                    changed |= slider(ui, "Width", &mut s.width, 0, 1000);
                    changed |= slider(ui, "Offset", &mut s.offset, -1000, 1000);
                    changed |= slider(ui, "Decimation", &mut s.decimate, 0, 1000);
                    ui.label("Impulse");
                    changed |= kernel_editor(ui, &mut d.kernel);
                    ui.menu_button("Presets", |ui| {
                        for p in kernel_presets() {
                            let name = p
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if ui.button(name).clicked() {
                                if let Ok(data) = std::fs::read(&p) {
                                    d.kernel.load_any(&data);
                                    changed = true;
                                }
                                ui.close_menu();
                            }
                        }
                    });
                }
                ToolKind::Smooth => {
                    let s = &mut d.smooth;
                    let (a, r, asl, rsl) = (s.attack, s.release, s.attack_slope, s.release_slope);
                    changed |= slider(ui, "Attack", &mut s.attack, 0, 1000);
                    changed |= slider(ui, "Release", &mut s.release, 0, 1000);
                    changed |= slider(ui, "Attack slope", &mut s.attack_slope, 0, 1000);
                    changed |= slider(ui, "Release slope", &mut s.release_slope, 0, 1000);
                    if s.link {
                        // AttWheelChange 0xce4b0: a linked change copies to the other control.
                        if s.attack != a {
                            s.release = s.attack;
                        } else if s.release != r {
                            s.attack = s.release;
                        }
                        if s.attack_slope != asl {
                            s.release_slope = s.attack_slope;
                        } else if s.release_slope != rsl {
                            s.attack_slope = s.release_slope;
                        }
                    }
                    ui.checkbox(&mut s.link, "Link attack & release controls");
                    changed |= slider(ui, "Offset", &mut s.offset, -1000, 1000);
                    changed |= slider(ui, "Decimation", &mut s.decimate, 0, 1000);
                }
                ToolKind::Sequence => {
                    changed |= sequence_editor(ui, d);
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() {
                    reset = true;
                }
                if ui.button("Accept").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    accept = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });
            first = ui.ctx().memory(|m| {
                m.data
                    .get_temp::<bool>(Id::new(("tool_open", title)))
                    .is_none()
            });
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp(Id::new(("tool_open", title)), true));
        });
    if reset {
        match t.kind {
            ToolKind::Scale => d.scale = ScaleSettings::default(),
            ToolKind::Decimate => d.decimate = 500,
            ToolKind::Filter => {
                d.filter = FilterSettings::default();
                d.kernel = default_kernel();
            }
            ToolKind::Smooth => {
                d.smooth = SmoothSettings {
                    link: true,
                    ..SmoothSettings::default()
                }
            }
            ToolKind::Sequence => {
                d.steps = default_steps();
                d.seq = SeqSettings::default();
            }
        }
        changed = true;
    }
    if changed || first || accept {
        let mut e = t.saved.clone();
        let result = match t.kind {
            ToolKind::Scale => {
                e.scale_levels(&d.scale);
                Ok(())
            }
            ToolKind::Decimate => e.decimate_tool(d.decimate),
            ToolKind::Filter => e.filter(&d.filter, &d.kernel),
            ToolKind::Smooth => e.smooth_up(&d.smooth),
            ToolKind::Sequence => {
                e.create_sequence(&d.steps, &d.seq);
                Ok(())
            }
        };
        match result {
            Ok(()) => gui.edit_slot(t.bank, t.index, false, |s| s.env = e),
            Err(err) => {
                gui.edit_slot(t.bank, t.index, false, |s| s.env = t.saved.clone());
                gui.dialogs.message = Some(err);
                ctx.memory_mut(|m| m.data.remove::<bool>(Id::new(("tool_open", title))));
                return;
            }
        }
    }
    if accept || cancel {
        if cancel {
            gui.edit_slot(t.bank, t.index, false, |s| s.env = t.saved.clone());
        }
        ctx.memory_mut(|m| m.data.remove::<bool>(Id::new(("tool_open", title))));
    } else {
        gui.dialogs.tool = Some(t);
    }
}

fn kernel_presets() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(PathBuf::from(GB_DIR).join("Data/Maps"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fnv")))
        .collect();
    v.sort();
    v
}

/// The Filter impulse editor: a small envelope editor of length 1 (drag points vertically,
/// right-click adds, double-click deletes).
fn kernel_editor(ui: &mut Ui, k: &mut Envelope) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(220.0, 90.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, rgb(0x294050));
    let x = |pos: f64| r.min.x + pos as f32 * r.width();
    let y = |v: f64| r.min.y + (1.0 - v as f32) * r.height();
    let mut pts = Vec::new();
    let n = 110;
    for i in 0..=n {
        let pos = i as f64 / n as f64;
        pts.push(pos2(x(pos), y(k.value_at(pos))));
    }
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.5f32, TIME)));
    let abs: Vec<f64> = (0..k.points.len()).map(|i| k.abs_pos(i)).collect();
    for (i, q) in k.points.iter().enumerate() {
        p.circle_filled(pos2(x(abs[i]), y(q.value as f64)), 3.0, lighten(TIME, 0x20));
    }
    let mut changed = false;
    if let Some(m) = resp.interact_pointer_pos() {
        let pos = ((m.x - r.min.x) / r.width()).clamp(0.0, 1.0) as f64;
        let val = (1.0 - (m.y - r.min.y) / r.height()).clamp(0.0, 1.0) as f64;
        let near = (0..k.points.len())
            .min_by(|&a, &b| (abs[a] - pos).abs().total_cmp(&(abs[b] - pos).abs()));
        if resp.double_clicked() {
            if let Some(i) = near {
                k.delete_point(i);
                changed = true;
            }
        } else if resp.clicked_by(PointerButton::Secondary) {
            k.insert_point(pos, val as f32, 0.0, true, false);
            changed = true;
        } else if resp.dragged_by(PointerButton::Primary)
            && let Some(i) = near
        {
            k.points[i].value = val as f32;
            k.prepare();
            changed = true;
        }
    }
    changed
}

/// The Create sequence editor (TMEArpToolForm): the step table and the wheels.
fn sequence_editor(ui: &mut Ui, d: &mut Dialogs) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Mode");
        let mut pp = d.seq.ping_pong;
        egui::ComboBox::from_id_salt("seq_mode")
            .selected_text(if pp { "Ping-pong" } else { "Normal" })
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut pp, false, "Normal").changed();
                changed |= ui.selectable_value(&mut pp, true, "Ping-pong").changed();
            });
        d.seq.ping_pong = pp;
    });
    // StepVisual: left click steps the type up, right click down (Off / on / hold / stick).
    let cell = 12.0;
    let (r, resp) = ui.allocate_exact_size(vec2(cell * 32.0, 16.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    let colors = [rgb(0x2b3a45), TIME, VOLUME, rgb(0x6fb4ff)];
    for (i, s) in d.steps.iter().enumerate() {
        let c = Rect::from_min_size(
            pos2(r.min.x + i as f32 * cell, r.min.y),
            vec2(cell - 1.0, 16.0),
        );
        p.rect_filled(c, 1.0, colors[s.kind.clamp(0, 3) as usize]);
    }
    if let Some(m) = resp.interact_pointer_pos() {
        let i = (((m.x - r.min.x) / cell) as usize).min(31);
        if resp.clicked() {
            d.steps[i].kind = (d.steps[i].kind + 1).rem_euclid(4);
            changed = true;
        } else if resp.clicked_by(PointerButton::Secondary) {
            d.steps[i].kind = (d.steps[i].kind - 1).rem_euclid(4);
            changed = true;
        }
    }
    // TargetSelect and LevelVisual.
    ui.horizontal(|ui| {
        for (k, name) in ["Level", "Decay slope", "Sustain", "Release slope"]
            .iter()
            .enumerate()
        {
            if ui.selectable_label(d.seq_field == k, *name).clicked() {
                d.seq_field = k;
            }
        }
    });
    let field = 2 + d.seq_field;
    let (r, resp) = ui.allocate_exact_size(vec2(cell * 32.0, 80.0), Sense::click_and_drag());
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, rgb(0x1f2a31));
    let max = SEQ_FIELDS[field].1;
    for (i, s) in d.steps.iter().enumerate() {
        let v = [
            s.kind,
            s.brk,
            s.level,
            s.decay_slope,
            s.sustain,
            s.release_slope,
        ][field] as f32
            / max as f32;
        let c = Rect::from_min_max(
            pos2(r.min.x + i as f32 * cell, r.max.y - v * r.height()),
            pos2(r.min.x + (i + 1) as f32 * cell - 1.0, r.max.y),
        );
        p.rect_filled(c, 0.0, if s.kind == 0 { rgb(0x485762) } else { TIME_MIX });
    }
    if let Some(m) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let i = (((m.x - r.min.x) / cell) as usize).min(31);
        let alt = ui.input(|inp| inp.modifiers.alt);
        let v = if alt {
            SEQ_FIELDS[field].0
        } else {
            ((1.0 - (m.y - r.min.y) / (r.height() - 1.0)) as f64 * max as f64)
                .round()
                .clamp(0.0, max as f64) as i32
        };
        set_field(&mut d.steps[i], field, v);
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui.button("Reset").clicked() {
            for s in &mut d.steps {
                set_field(s, field, SEQ_FIELDS[field].0);
            }
            changed = true;
        }
        if ui.button("Randomize").clicked() {
            for i in 0..32 {
                let v = random_field(d, field);
                if field != 0 || d.steps[i].kind > 0 {
                    set_field(&mut d.steps[i], field, v);
                }
            }
            changed = true;
        }
        if ui.button("Humanize").clicked() {
            for i in 0..32 {
                let cur = [
                    d.steps[i].kind,
                    d.steps[i].brk,
                    d.steps[i].level,
                    d.steps[i].decay_slope,
                    d.steps[i].sustain,
                    d.steps[i].release_slope,
                ][field];
                let v = (cur + d.random(0x199a) as i32 - 0xccc).clamp(0, 0x10000);
                set_field(&mut d.steps[i], field, v);
            }
            changed = true;
        }
        if ui.button("Full random").clicked() {
            for f in [0usize, 2, 3, 4, 5] {
                for i in 0..32 {
                    let v = random_field(d, f);
                    if f != 0 || d.steps[i].kind > 0 {
                        set_field(&mut d.steps[i], f, v);
                    }
                }
            }
            changed = true;
        }
    });
    let s = &mut d.seq;
    changed |= slider(ui, "Swing", &mut s.swing, 0, 1000);
    changed |= slider(ui, "Attack", &mut s.attack, 0, 900);
    changed |= slider(ui, "Decay", &mut s.decay, 0, 900);
    changed |= slider(ui, "Sustain", &mut s.sustain, 0, 1000);
    changed |= slider(ui, "Gate", &mut s.gate, 100, 1000);
    changed
}

fn set_field(s: &mut SeqStep, f: usize, v: i32) {
    match f {
        0 => s.kind = v,
        1 => s.brk = v,
        2 => s.level = v,
        3 => s.decay_slope = v,
        4 => s.sustain = v,
        _ => s.release_slope = v,
    }
}

/// A random field value with the formulas of 0xcd6c0 (envelope.md 5.11.1).
fn random_field(d: &mut Dialogs, f: usize) -> i32 {
    match f {
        0 => [1, 1, 1, 1, 1, 2, 2, 2, 3][d.random(9) as usize],
        1 => [2, 2, 2, 2, 4, 4, 4, 4, 3][d.random(9) as usize],
        2 => {
            let r = (d.random(1 << 24) as f64 / (1u64 << 24) as f64) as f32;
            let e = ((r as f64 * 6.90875477931522).exp() - 1.0) as f32;
            (65536.0 - e as f64 * 52.4288).round() as i32
        }
        _ => d.random(0x10000) as i32,
    }
}

/// Analyze audio file (0xb9610): picks a file, decodes it and runs the analyzer.
pub fn analyze_file(gui: &mut Gui, bank: usize, index: usize) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(
            "Audio files",
            &["wav", "aif", "aiff", "wv", "mp3", "ogg", "flac", "m4a"],
        )
        .pick_file()
    else {
        return;
    };
    match crate::decode::decode(&path) {
        Ok((frames, channels, sr)) if !frames.is_empty() => {
            gui.edit_slot(bank, index, false, |s| {
                s.env.analyze(&frames, channels, sr, 0.0)
            });
        }
        Ok(_) => {}
        Err(e) => {
            gui.dialogs.message = Some(ToolError {
                title: "Analyze audio file",
                message: e,
            })
        }
    }
}

/// The About picture (`Artwork/About.png` in the plugin folder).
fn about(gui: &mut Gui, ctx: &Context) {
    if !gui.dialogs.about {
        return;
    }
    if gui.dialogs.about_image.is_none()
        && let Some(img) = load_png(&PathBuf::from(GB_DIR).join("Artwork/About.png"))
    {
        gui.dialogs.about_image = Some(ctx.load_texture("about", img, Default::default()));
    }
    let mut open = true;
    egui::Window::new("About")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| match &gui.dialogs.about_image {
            Some(t) => {
                ui.image((t.id(), t.size_vec2()));
            }
            None => {
                ui.label("Backspin");
            }
        });
    if !open {
        gui.dialogs.about = false;
    }
    let _: Color32 = PANEL;
}

fn load_png(path: &std::path::Path) -> Option<ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(file);
    dec.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2]
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        _ => return None,
    };
    Some(ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}
