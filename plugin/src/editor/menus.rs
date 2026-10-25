//! The menus (ui.md section 3).

use super::dialogs::{self, ToolKind};
use super::paint::*;
use super::{Gui, refusal};
use crate::shared::Command;
use backspin_engine::envelope::tools::{ToolError, parse_value, value_text};
use backspin_engine::envelope::{Envelope, SHAPE_MENU};
use backspin_engine::{PARAMS, fst_state};
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{self, Button, Id, Pos2, Rect, Response, RichText, Ui, pos2};
use std::path::PathBuf;
use std::sync::Mutex;

/// A deferred edit of the slot's envelope.
type Edit = Box<dyn FnOnce(&mut Envelope) -> Result<(), ToolError>>;

/// The FL install folder of Gross Beat (state presets, maps and artwork; SPEC.md decision 15).
pub const GB_DIR: &str =
    "/Applications/FL Studio 2026.app/Contents/Resources/FL/Plugins/Fruity/Effects/Gross Beat";

/// The factory state presets of the local FL install.
pub const PRESET_DIR: &str = "/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat";

/// The "Image Line Envelope" clipboard of Copy state / Paste state. It lives in the process, so
/// it is shared by every instance but not with FL.
static STATE_CLIPBOARD: Mutex<Option<Vec<u8>>> = Mutex::new(None);

fn title(ui: &mut Ui, s: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(s).small().color(rgb(0x8595a0)));
}

fn item(ui: &mut Ui, enabled: bool, text: &str, shortcut: &str) -> bool {
    let b = Button::new(text).shortcut_text(shortcut);
    ui.add_enabled(enabled, b).clicked()
}

/// The text clipboard (Copy value / Paste value).
pub fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

pub fn set_clipboard_text(s: &str) {
    if let Ok(mut c) = arboard::Clipboard::new() {
        let _ = c.set_text(s.to_string());
    }
}

/// SynthResModule.OptionsMenu (3.1), opened by the Options button.
pub fn options_menu(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter, r: &Response) {
    let id = Id::new("options_menu");
    if r.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup_below_widget(
        ui,
        id,
        r,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_min_width(230.0);
            // Backspin addition: the presets are in the ShaperBox section's header.
            if ui.button("Open preset file...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Gross Beat preset", &["fst"])
                    .pick_file()
                {
                    load_preset(gui, setter, &p);
                }
                ui.close_menu();
            }
            if ui.button("Open ShaperBox preset file...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("ShaperBox preset", &["dat"])
                    .pick_file()
                {
                    let _ = gui.shared.sb.load_file(&p);
                }
                ui.close_menu();
            }
            ui.separator();
            let (mut midi, mut click, mut comp, mut dc, mut hq, mut view) = {
                let m = gui.shared.model.lock();
                (
                    m.midi_trigger != 0,
                    m.click,
                    m.attack_comp != 0,
                    m.remove_dc != 0,
                    m.hq != 0,
                    m.view_scratch != 0,
                )
            };
            let mut options_changed = false;
            if ui
                .checkbox(&mut midi, "Trigger slots from MIDI notes")
                .changed()
            {
                gui.shared.model.lock().midi_trigger = midi as u8;
                gui.shared.send(Command::MidiTrigger(midi as u8));
            }
            ui.separator();
            ui.menu_button("Click reduction", |ui| {
                for (k, s) in ["Off", "Low (drums)", "High (melodic)"].iter().enumerate() {
                    if ui.radio(click == k as u8, *s).clicked() {
                        click = k as u8;
                        options_changed = true;
                    }
                }
            });
            options_changed |= ui
                .checkbox(&mut comp, "Smoothing attack compensation")
                .changed();
            options_changed |= ui.checkbox(&mut dc, "Remove DC offset").changed();
            options_changed |= ui.checkbox(&mut hq, "HQ resampling").changed();
            if options_changed {
                let mut m = gui.shared.model.lock();
                m.set_options(dc as u8, comp as u8, hq as u8, click);
                gui.shared.send(Command::Options {
                    remove_dc: dc as u8,
                    attack_comp: comp as u8,
                    hq: hq as u8,
                    click,
                });
            }
            ui.separator();
            if ui.checkbox(&mut view, "View scratching clock").changed() {
                gui.shared.model.lock().view_scratch = view as u8;
            }
            ui.separator();
            ui.menu_button("About", |ui| {
                if ui.button("About").clicked() {
                    gui.dialogs.about = true;
                    ui.close_menu();
                }
            });
        },
    );
}

/// SynthResModule.WheelMenu (3.2): the right-click menu of the knobs, the slot grids, HOLD and
/// the scratch clock. The host's parameter items of FL are not available through the plugin API.
pub fn wheel_menu(gui: &mut Gui, _ui: &mut Ui, setter: &ParamSetter, r: &Response, tag: usize) {
    r.context_menu(|ui| {
        if ui.button("Reset").clicked() {
            gui.set_param(setter, tag, PARAMS[tag].default);
            ui.close_menu();
        }
        if tag < 2 {
            let bank = tag;
            if ui.button("Edit slot name...").clicked() {
                {
                    let name = gui.shared.model.lock().banks[bank].slots[gui.edited[bank]]
                        .name
                        .clone();
                    gui.dialogs.rename(bank, gui.edited[bank], name);
                };
                ui.close_menu();
            }
            for (label, d) in [("Move slot left", -1i32), ("Move slot right", 1)] {
                if ui.button(label).clicked() {
                    let from = gui.edited[bank];
                    let to = ((d + from as i32 + 36) % 36) as usize;
                    gui.shared.model.lock().move_slot(bank, from, to);
                    gui.shared.send(Command::MoveSlot { bank, from, to });
                    gui.select_edited(setter, bank, to, false);
                    gui.set_param(setter, bank, to as i32);
                    ui.close_menu();
                }
            }
        }
    });
}

/// The factory state presets of the local FL install, then the user's own, sorted by name.
pub fn state_presets() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(PRESET_DIR)];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(
            PathBuf::from(home)
                .join("Documents/Image-Line/FL Studio/Presets/Plugin presets/Effects/Gross Beat"),
        );
    }
    let mut out = Vec::new();
    for d in dirs {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .map(|r| {
                r.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fst")))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        out.extend(v);
    }
    out
}

/// Loads a state preset: the model takes the new state, every slot and the options go to the
/// audio engine, and the parameters go through the host.
pub fn load_preset(gui: &mut Gui, setter: &ParamSetter, path: &std::path::Path) {
    let Ok(data) = std::fs::read(path) else {
        return;
    };
    let blob = match fst_state(&data) {
        Ok(b) => b.to_vec(),
        Err(_) => data,
    };
    let mut new = backspin_engine::Engine::new();
    new.load_state(&blob);
    let params = new.params;
    let (dc, comp, hq, click, midi) = (
        new.remove_dc,
        new.attack_comp,
        new.hq,
        new.click,
        new.midi_trigger,
    );
    {
        let mut m = gui.shared.model.lock();
        *m = new;
        for bank in 0..2 {
            for i in 0..36 {
                gui.shared.send_slot(&m, bank, i, false);
            }
        }
    }
    gui.shared.send(Command::Options {
        remove_dc: dc,
        attack_comp: comp,
        hq,
        click,
    });
    gui.shared.send(Command::MidiTrigger(midi));
    for (i, &v) in params.iter().enumerate() {
        gui.set_param(setter, i, v);
    }
    gui.edited = [
        params[0].clamp(0, 35) as usize,
        params[1].clamp(0, 35) as usize,
    ];
}

/// The `.fnv` files of the Maps folder (the Presets submenu).
fn presets() -> Vec<PathBuf> {
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

/// Loads a `.fnv` file into the edited envelope with the kind check bypassed (0x239070).
fn load_fnv(gui: &mut Gui, path: &std::path::Path) {
    if let Ok(data) = std::fs::read(path) {
        let (g, i) = (gui.group(), gui.edited[gui.group()]);
        gui.edit_slot(g, i, false, |s| s.env.load_any(&data));
    }
}

/// MEnvEditor.EnvOptionsMenu (3.3), opened by the menu button: two columns.
pub fn env_options_menu(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter, r: &Response) {
    let id = Id::new("env_options_menu");
    if r.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    let group = gui.group();
    let index = gui.edited[group];
    egui::popup_below_widget(
        ui,
        id,
        r,
        egui::PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            let env = gui.shared.model.lock().banks[group].slots[index]
                .env
                .clone();
            let sel = env.selected_count();
            let cont = env.selection_continuous();
            let mut edit: Option<Edit> = None;
            let mut close = false;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(190.0);
                    title(ui, "State");
                    let list = presets();
                    ui.add_enabled_ui(!list.is_empty(), |ui| {
                        ui.menu_button("Presets", |ui| {
                            for p in &list {
                                let name = p
                                    .file_stem()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                if ui.button(name).clicked() {
                                    load_fnv(gui, p);
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                    if item(ui, true, "Open state file...", "") {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("Envelope state file", &["fnv"])
                            .pick_file()
                        {
                            load_fnv(gui, &p);
                        }
                        close = true;
                    }
                    if item(ui, true, "Save state file...", "") {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("Envelope state file", &["fnv"])
                            .save_file()
                        {
                            let _ = backspin_engine::envelope::write_fnv(&p, &env);
                        }
                        close = true;
                    }
                    ui.separator();
                    let mut is_default = env.is_default();
                    if ui.checkbox(&mut is_default, "Reset").clicked() {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.reset();
                            e.prepare();
                            Ok(())
                        }));
                    }
                    if item(ui, true, "Prepare for smooth editing", "") {
                        // The editor's grids (+0x1590 = one beat, +0x1598 = the snap step).
                        let coarse = 1.0 / super::SNAP_DIV[gui.snap] as f64;
                        edit = Some(Box::new(move |e: &mut Envelope| {
                            e.prepare_smooth_editing(1.0, coarse);
                            Ok(())
                        }));
                        close = true;
                    }
                    ui.separator();
                    if item(ui, true, "Copy state", "") {
                        *STATE_CLIPBOARD.lock().unwrap() = Some(env.state_clipboard());
                    }
                    let clip = STATE_CLIPBOARD.lock().unwrap().clone();
                    let can_paste = clip
                        .as_deref()
                        .is_some_and(Envelope::accepts_state_clipboard);
                    if item(ui, can_paste, "Paste state", "") {
                        let data = clip.unwrap_or_default();
                        edit = Some(Box::new(move |e: &mut Envelope| {
                            e.paste_state_clipboard(&data);
                            Ok(())
                        }));
                    }
                    title(ui, "Points selection");
                    if item(ui, true, "Select all", "Ctrl+A") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.select_all();
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, true, "Deselect all", "Ctrl+D") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.deselect_all();
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, true, "Invert", "Ctrl+I") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.invert_selection();
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, cont, "Cut", "Ctrl+X") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.copy_points(true);
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, sel >= 1, "Copy", "Ctrl+C") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.copy_points(false);
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, !env.clip.is_empty(), "Paste (insert)", "Ctrl+V") {
                        edit = Some(Box::new(|e: &mut Envelope| e.paste_points(false)));
                        close = true;
                    }
                    if item(
                        ui,
                        !env.clip.is_empty() && cont,
                        "Paste (replace)",
                        "Ctrl+R",
                    ) {
                        edit = Some(Box::new(|e: &mut Envelope| e.paste_points(true)));
                        close = true;
                    }
                    if item(ui, sel > 1, "Duplicate", "Ctrl+B") {
                        edit = Some(Box::new(|e: &mut Envelope| e.duplicate()));
                        close = true;
                    }
                    if item(ui, sel >= 1, "Delete", "Del") {
                        edit = Some(Box::new(|e: &mut Envelope| e.delete_selected()));
                        close = true;
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width(190.0);
                    title(ui, "Tools");
                    item(ui, false, "Insert point at locator", "");
                    if item(ui, true, "Flip vertically", "") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.flip();
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, true, "Scale levels...", "") {
                        gui.dialogs.open_tool(ToolKind::Scale, group, index, &env);
                        close = true;
                    }
                    if item(ui, true, "Normalize levels", "") {
                        edit = Some(Box::new(|e: &mut Envelope| {
                            e.normalize();
                            Ok(())
                        }));
                        close = true;
                    }
                    if item(ui, true, "Decimate points...", "") {
                        if sel > 1 && !cont {
                            refusal(gui, dialogs::not_continuous("decimate"));
                        } else {
                            gui.dialogs
                                .open_tool(ToolKind::Decimate, group, index, &env);
                        }
                        close = true;
                    }
                    if item(ui, true, "Filter...", "") {
                        if sel > 1 && !cont {
                            refusal(gui, dialogs::not_continuous("filter"));
                        } else {
                            gui.dialogs.open_tool(ToolKind::Filter, group, index, &env);
                        }
                        close = true;
                    }
                    if item(ui, true, "Smooth up...", "") {
                        if sel > 1 && !cont {
                            refusal(gui, dialogs::not_continuous("smoothing"));
                        } else {
                            gui.dialogs.open_tool(ToolKind::Smooth, group, index, &env);
                        }
                        close = true;
                    }
                    if item(ui, true, "Turn all points smooth", "") {
                        edit = Some(Box::new(|e: &mut Envelope| e.turn_smooth()));
                        gui.last_tension = 0.0;
                        gui.default_shape = backspin_engine::envelope::Shape::SMOOTH;
                        close = true;
                    }
                    if item(ui, true, "Create sequence...", "") {
                        gui.dialogs
                            .open_tool(ToolKind::Sequence, group, index, &env);
                        close = true;
                    }
                    if item(ui, true, "Analyze audio file...", "") {
                        dialogs::analyze_file(gui, group, index);
                        close = true;
                    }
                    title(ui, "Options");
                    ui.checkbox(&mut gui.focus_on_click, "Focus on click");
                    title(ui, "View");
                    ui.checkbox(&mut gui.gradient, "Background gradient");
                    ui.checkbox(&mut gui.invert_grid, "Invert grid");
                });
            });
            if let Some(f) = edit {
                let mut e = env.clone();
                match f(&mut e) {
                    Ok(()) => gui.edit_slot(group, index, false, |s| s.env = e),
                    Err(err) => refusal(gui, err),
                }
            }
            if close {
                ui.memory_mut(|m| m.close_popup());
            }
            let _ = setter;
        },
    );
}

/// MEnvEditor.PointMenu (3.4), opened by a right-click on a point.
pub fn point_menu(gui: &mut Gui, ui: &mut Ui, _setter: &ParamSetter, area: &Rect) {
    let Some((i, at)) = gui.env.point_menu else {
        return;
    };
    let group = gui.group();
    let index = gui.edited[group];
    let env = gui.shared.model.lock().banks[group].slots[index]
        .env
        .clone();
    if i >= env.points.len() {
        gui.env.point_menu = None;
        return;
    }
    let sel = env.selected_count();
    let mut close = false;
    let mut edit: Option<Envelope> = None;
    let pos = Pos2::new(
        at.x.min(area.max.x - 160.0),
        at.y.min(area.max.y - 380.0).max(0.0),
    );
    let resp = egui::Area::new(Id::new("point_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ui.ctx(), |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.set_min_width(150.0);
                let head = if sel > 1 {
                    "Multiple selection".to_string()
                } else {
                    format!("Point {}/{}", i + 1, env.points.len())
                };
                title(ui, &head);
                let can_delete = !(i == 0 && sel <= 1);
                if item(ui, can_delete, "Delete", "") {
                    let mut e = env.clone();
                    match e.delete_selected() {
                        Ok(()) => edit = Some(e),
                        Err(err) => refusal(gui, err),
                    }
                    close = true;
                }
                title(ui, "Mode");
                let mode_ok = i >= 1 || sel > 1;
                for (id, name) in SHAPE_MENU {
                    if ui
                        .add_enabled(
                            mode_ok,
                            egui::RadioButton::new(env.points[i].shape == id, name),
                        )
                        .clicked()
                    {
                        let mut e = env.clone();
                        for q in &mut e.points {
                            if q.selected() {
                                q.shape = id;
                            }
                        }
                        e.prepare();
                        gui.default_shape = id;
                        edit = Some(e);
                        close = true;
                    }
                }
                ui.separator();
                if item(ui, sel <= 1, "Copy value", "") {
                    set_clipboard_text(&value_text(env.points[i].value));
                    close = true;
                }
                let pasted = clipboard_text().and_then(|s| parse_value(&s, true));
                let label = match pasted {
                    Some(v) => format!("Paste value ({}%)", (v * 100.0).round() as i64),
                    None => "Paste value".into(),
                };
                if item(ui, pasted.is_some(), &label, "") {
                    let mut e = env.clone();
                    e.set_selected_values(pasted.unwrap_or(0.0));
                    edit = Some(e);
                    close = true;
                }
                if item(ui, true, "Type in value...", "") {
                    gui.dialogs.type_value(group, index, i, env.points[i].value);
                    close = true;
                }
                title(ui, "Pitch");
                for (label, up) in [("1 semitone up", true), ("1 semitone down", false)] {
                    if item(ui, true, label, "") {
                        let mut e = env.clone();
                        e.semitone(i, up);
                        edit = Some(e);
                        close = true;
                    }
                }
            });
        });
    if let Some(e) = edit {
        gui.edit_slot(group, index, false, |s| s.env = e);
    }
    let clicked_outside = ui.input(|inp| inp.pointer.any_pressed())
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| !resp.response.rect.contains(p))
        && !ui.input(|inp| inp.pointer.secondary_pressed());
    if close || clicked_outside {
        gui.env.point_menu = None;
    }
    let _ = pos2;
}
