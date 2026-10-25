//! The editor (ui.md): the slot column, the side buttons, the envelope editor and the slot bar.
//!
//! The editor works on the model engine in [`Shared`] and sends every change to the audio engine
//! (`Shared::send_slot` and friends). Host parameters go through the [`ParamSetter`].

mod dialogs;
mod envelope;
mod frame;
mod menus;
mod paint;
mod sb_section;

use crate::params::{BackspinParams, EDITOR_SIZE};
use crate::shared::{Command, Shared};
use backspin_engine::envelope::tools::ToolError;
use backspin_engine::{Engine, PARAMS, param_text};
use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::egui::{
    self, Align2, Color32, Context, Id, PointerButton, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2,
};
use nih_plug_egui::{EguiState, create_egui_editor};
use paint::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The SnapSel divisions (0x34c948): index 0 is off with a 1/4-beat grid.
pub const SNAP_DIV: [i32; 7] = [4, 16, 12, 8, 6, 4, 3];
pub const SNAP_HINTS: [&str; 7] = [
    "off",
    "1/16 beat",
    "1/12 beat",
    "1/8 beat",
    "1/6 beat",
    "1/4 beat",
    "1/3 beat",
];
pub const SYNC_ITEMS: [&str; 6] = [
    "Off", "1/4 beat", "1/2 beat", "1 beat", "2 beats", "4 beats",
];
pub const HOLD_ITEMS: [&str; 3] = ["Off", "Last", "Next"];
const GROUP_NAMES: [&str; 2] = ["Time", "Volume"];
/// The width of the left column (S(266)).
const COLUMN: f32 = 266.0;

pub fn create(params: Arc<BackspinParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let state = Gui::new(params.clone(), shared);
    create_egui_editor(
        params.editor_state.clone(),
        state,
        |ctx, _| {
            let mut style = (*ctx.style()).clone();
            style.visuals.window_fill = rgb(0x3b4a54);
            style.visuals.panel_fill = PANEL;
            style.visuals.override_text_color = Some(rgb(0xd0dfea));
            ctx.set_style(style);
        },
        |ctx, setter, gui| gui.frame(ctx, setter),
    )
}

/// The editor state (TSynthEditorForm plus the envelope editor settings).
pub struct Gui {
    pub params: Arc<BackspinParams>,
    pub shared: Arc<Shared>,
    /// The edited slot of each grid (group+8).
    pub edited: [usize; 2],
    /// Compact mode (SlotZoom down: the envelope hidden).
    pub compact: bool,
    pub freeze: bool,
    pub slide: bool,
    pub step: bool,
    /// SnapSel index 0..6.
    pub snap: usize,
    /// The EnvOptionsMenu View and Options items (registry settings in the original).
    pub invert_grid: bool,
    pub gradient: bool,
    pub focus_on_click: bool,
    /// The last tension (+0x17dc) and the default shape for new points (+0x17e0).
    pub last_tension: f32,
    pub default_shape: u8,
    pub env: envelope::EnvState,
    pub dialogs: dialogs::Dialogs,
    /// A knob being dragged: parameter index, start value and the accumulated pointer travel.
    knob_drag: Option<(usize, f32, f32)>,
    /// The scratch clock drag: the last pointer angle and the current value.
    scratch_drag: Option<(f32, f32)>,
    /// The engine parameters last seen in the status, to follow MIDI slot selection.
    last_status: [i32; 2],
    /// The text shown in the hint area.
    pub hint: String,
    /// The local ShaperBox preset library, loaded on a background thread on first use.
    pub sb_library: Arc<parking_lot::Mutex<Option<Vec<crate::sb::Pack>>>>,
    pub sb_loading: bool,
    /// The ShaperBox section: the pack shown, the loaded preset file, the selected shaper and
    /// band.
    pub sb_pack: usize,
    pub sb_preset: Option<std::path::PathBuf>,
    pub sb_selected: usize,
    pub sb_band: usize,
    /// The last Gross Beat preset loaded from the header.
    pub gb_preset: String,
    /// The shape mix knob drag: the start value and the pointer travel.
    pub sb_mix_drag: Option<(f32, f32)>,
    /// The texture of the code-exact frame.
    frame_tex: Option<egui::TextureHandle>,
}

impl Gui {
    fn new(params: Arc<BackspinParams>, shared: Arc<Shared>) -> Self {
        let (edited, last_status) = {
            let m = shared.model.lock();
            (
                [
                    m.banks[0].selected.max(0) as usize,
                    m.banks[1].selected.max(0) as usize,
                ],
                [m.params[0], m.params[1]],
            )
        };
        Self {
            params,
            shared,
            edited,
            compact: false,
            freeze: false,
            slide: false,
            step: false,
            snap: 3,
            invert_grid: true,
            gradient: false,
            focus_on_click: false,
            last_tension: 0.0,
            default_shape: 2,
            env: envelope::EnvState::default(),
            dialogs: dialogs::Dialogs::default(),
            knob_drag: None,
            scratch_drag: None,
            last_status,
            hint: String::new(),
            sb_library: Arc::new(parking_lot::Mutex::new(None)),
            sb_loading: false,
            sb_pack: 0,
            sb_preset: None,
            sb_selected: 5,
            sb_band: 1,
            gb_preset: "(current)".into(),
            sb_mix_drag: None,
            frame_tex: None,
        }
    }

    /// Selects shaper `k` in the ShaperBox section and shows the band its split selects.
    pub fn select_shaper(&mut self, k: usize) {
        self.sb_selected = k;
        let m = self.shared.sb.model.lock();
        self.sb_band = m
            .tree
            .as_ref()
            .map(|t| {
                let sp = backspin_shaperbox::state::State::from_tree(t).shapers[k].split;
                let active = [sp.low > 0.0, true, sp.high < 1.0];
                let b = sp.selected.clamp(0, 2) as usize;
                if active[b] { b } else { 1 }
            })
            .unwrap_or(1);
    }

    /// The current group (time 0 / volume 1): the model's editor view bank.
    pub fn group(&self) -> usize {
        (self.shared.model.lock().view_bank as usize).min(1)
    }

    /// The current host value of parameter `i`.
    pub fn param(&self, i: usize) -> i32 {
        self.params.by_index()[i].value()
    }

    /// Sets a host parameter (begin, set, end), as CutoffWheelChange sends `(Tag, Value)`.
    pub fn set_param(&self, setter: &ParamSetter, i: usize, v: i32) {
        let p = self.params.by_index()[i];
        let v = v.clamp(PARAMS[i].min, PARAMS[i].max);
        setter.begin_set_parameter(p);
        setter.set_parameter(p, v);
        setter.end_set_parameter(p);
    }

    /// Edits the model's slot of `bank`/`index` and sends it to the audio engine.
    pub fn edit_slot(
        &self,
        bank: usize,
        index: usize,
        rearm: bool,
        f: impl FnOnce(&mut backspin_engine::Slot),
    ) {
        let mut m = self.shared.model.lock();
        let slot = &mut m.banks[bank].slots[index];
        f(slot);
        slot.update_empty();
        self.shared.send_slot(&m, bank, index, rearm);
    }

    /// Changes the edited slot of a grid (FUN_00067330): a slot with link selects the same index
    /// in the other grid while stopped, and the grid becomes the current group.
    pub fn select_edited(&mut self, setter: &ParamSetter, bank: usize, slot: usize, inhibit: bool) {
        if bank != self.group() || slot != self.edited[bank] {
            self.edited[bank] = slot;
            if !inhibit {
                let (link, playing) = {
                    let m = self.shared.model.lock();
                    (
                        m.banks[bank].slots[slot].link != 0,
                        self.shared.status.playing.load(Ordering::Relaxed),
                    )
                };
                if link && !playing {
                    self.edited[bank ^ 1] = slot;
                    self.set_param(setter, bank ^ 1, slot as i32);
                }
                self.set_group(bank);
            }
        }
    }

    /// Sets the current group (FUN_000670e0); compact mode forces time.
    pub fn set_group(&mut self, bank: usize) {
        let bank = if self.compact { 0 } else { bank };
        self.shared.model.lock().view_bank = bank as u32;
        self.env.clear();
    }

    /// Follows the audio engine: slot selections made by MIDI move the edited slot and the host
    /// parameter, as the selector's change event does in the original.
    fn follow_status(&mut self, setter: &ParamSetter) {
        self.shared.collect_garbage();
        for b in 0..2 {
            let v = self.shared.status.params[b].load(Ordering::Relaxed);
            if v != self.last_status[b] {
                self.last_status[b] = v;
                if self.param(b) != v {
                    self.set_param(setter, b, v);
                }
                if (0..36).contains(&v) {
                    self.edited[b] = v as usize;
                }
            }
        }
    }

    fn frame(&mut self, ctx: &Context, setter: &ParamSetter) {
        self.follow_status(setter);
        self.hint.clear();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                // The Gross Beat form keeps its own height; the ShaperBox section sits below.
                let (w, h) = (full.width(), backspin_ui::HEIGHT as f32);
                let mut tex = self.frame_tex.take();
                frame::show(self, ui, &mut tex);
                self.frame_tex = tex;
                self.left_column(ui, setter, h);
                sb_section::show(self, ui, setter);
                if !self.compact {
                    self.side_buttons(ui, setter, h);
                    let r = Rect::from_min_size(pos2(298.0, 5.0), vec2(w - 304.0, h - 34.0));
                    envelope::show(self, ui, setter, r);
                    self.slot_bar(ui, setter, h);
                    // The mouse wheel over the strip under the envelope toggles the group.
                    let strip = Rect::from_min_max(pos2(298.0, h - 29.0), pos2(w, h));
                    if ui.rect_contains_pointer(strip) {
                        let scroll = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll != 0.0 {
                            let g = self.group();
                            self.set_group(g ^ 1);
                        }
                    }
                }
                if !self.hint.is_empty() {
                    let p = ui.painter();
                    if let Some(pos) = ui.ctx().pointer_hover_pos() {
                        let text = self.hint.clone();
                        let galley = p.layout_no_wrap(text, font(11.0), rgb(0xe8eef2));
                        let mut at = pos + vec2(14.0, 18.0);
                        at.x = at.x.min(full.max.x - galley.size().x - 6.0);
                        at.y = at.y.min(full.max.y - galley.size().y - 4.0);
                        let bg = Rect::from_min_size(
                            at - vec2(3.0, 2.0),
                            galley.size() + vec2(6.0, 4.0),
                        );
                        let fg = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Tooltip,
                            Id::new("hint"),
                        ));
                        fg.rect_filled(bg, 2.0, rgba(0x1b2328, 0xe8));
                        fg.galley(at, galley, Color32::WHITE);
                    }
                }
            });
        dialogs::show(self, ctx, setter);
    }

    /// The left column: header, knobs, HOLD, the scratch clock and the two slot grids.
    fn left_column(&mut self, ui: &mut Ui, setter: &ParamSetter, h: f32) {
        let p = ui.painter().clone();
        let clock = self.shared.model.lock().view_scratch != 0;

        // Time and volume mix knobs.
        self.knob(
            ui,
            setter,
            2,
            Rect::from_min_size(pos2(7.0, 5.0), vec2(48.0, 48.0)),
            TIME_MIX,
            true,
        );
        self.knob(
            ui,
            setter,
            3,
            Rect::from_min_size(pos2(7.0, h - 55.0), vec2(48.0, 48.0)),
            VOLUME,
            true,
        );

        // Options button.
        let opt = Rect::from_min_size(pos2(7.0, 66.0), vec2(20.0, 20.0));
        let r = ui.interact(opt, Id::new("options_btn"), Sense::click());
        p.rect_filled(opt, 4.0, rgb(0x2a353d));
        glyph(&p, opt.shrink(2.0), Glyph::Options, ORANGE);
        if r.hovered() {
            self.hint = "Options".into();
        }
        menus::options_menu(self, ui, setter, &r);

        // HOLD button (parameter 7, a direct-click toggle).
        let hold = Rect::from_min_size(pos2(109.0, 60.0), vec2(63.0, 30.0));
        let r = ui.interact(hold, Id::new("hold_btn"), Sense::click());
        let on = self.param(7) > 0;
        p.rect_filled(hold, 6.0, if on { rgb(0x2c3a24) } else { rgb(0x29333a) });
        if on {
            p.rect_stroke(
                hold.shrink(1.0),
                6.0,
                Stroke::new(1.5f32, HOLD_ON),
                egui::StrokeKind::Middle,
            );
        }
        p.text(
            hold.center(),
            Align2::CENTER_CENTER,
            "HOLD",
            font(16.0),
            if on { HOLD_ON } else { HOLD_TEXT },
        );
        if r.clicked() {
            self.set_param(setter, 7, if on { 0 } else { 1 });
        }
        if r.hovered() {
            self.hint = "Hold delay (bar repeat)".into();
        }
        menus::wheel_menu(self, ui, setter, &r, 7);

        if clock {
            self.scratch_clock(
                ui,
                setter,
                Rect::from_min_size(pos2(186.0, 11.0), vec2(72.0, 72.0)),
            );
        } else if self.compact {
            let r = Rect::from_min_size(pos2(186.0, 11.0), vec2(72.0, 72.0));
            envelope::preview(self, ui, r);
        }

        // Slot grids: 4 columns x 9 rows, h = (H - 236) div 2.
        let gh = ((h - 236.0) / 2.0).floor();
        for bank in 0..2 {
            let top = if bank == 0 { 102.0 } else { 102.0 + gh + 11.0 };
            self.slot_grid(
                ui,
                setter,
                bank,
                Rect::from_min_size(pos2(10.0, top), vec2(248.0, gh)),
            );
        }

        // Volume knobs and their labels.
        let y = h - 113.0;
        self.knob(
            ui,
            setter,
            4,
            Rect::from_min_size(pos2(56.0, y), vec2(42.0, 42.0)),
            VOLUME,
            false,
        );
        self.knob(
            ui,
            setter,
            5,
            Rect::from_min_size(pos2(106.0, y), vec2(42.0, 42.0)),
            VOLUME,
            false,
        );
        self.knob(
            ui,
            setter,
            6,
            Rect::from_min_size(pos2(159.0, y - 4.0), vec2(48.0, 48.0)),
            VOLUME,
            false,
        );
        for (x, s) in [(77.0, "ATT"), (127.0, "REL"), (183.0, "TNS")] {
            p.text(
                pos2(x, h - 70.0),
                Align2::CENTER_CENTER,
                s,
                font(11.0),
                LABEL,
            );
        }

        // SlotZoom: toggle the envelope.
        let zr = Rect::from_min_size(pos2(8.0, h - 88.0), vec2(24.0, 19.0));
        let r = ui.interact(zr, Id::new("slot_zoom"), Sense::click());
        p.rect_filled(
            zr,
            3.0,
            if self.compact {
                rgb(0x25313b)
            } else {
                rgb(0x2a353d)
            },
        );
        glyph(
            &p,
            zr.shrink(3.0),
            Glyph::Zoom,
            if self.compact { ORANGE } else { rgb(0xb9bfc3) },
        );
        if r.hovered() {
            self.hint = "Toggle Envelope Visibility".into();
        }
        if r.clicked() {
            self.toggle_compact(setter);
        }
    }

    /// SlotZoomOnClick (0x64b40): compact mode asks the host for a 266 px wide window.
    fn toggle_compact(&mut self, setter: &ParamSetter) {
        self.compact = !self.compact;
        if self.compact {
            self.freeze = false;
            self.set_group(0);
        }
        let w = if self.compact {
            COLUMN as u32
        } else {
            EDITOR_SIZE.0
        };
        let state = Arc::try_unwrap(EguiState::from_size(w, EDITOR_SIZE.1)).ok();
        if let Some(state) = state {
            nih_plug::params::persist::PersistentField::set(&self.params.editor_state, state);
            setter.raw_context.request_resize();
        }
    }

    /// A TVectorWheel bound to parameter `i`: vertical drag (Ctrl for 16x finer), mouse wheel,
    /// right-click WheelMenu.
    fn knob(
        &mut self,
        ui: &mut Ui,
        setter: &ParamSetter,
        i: usize,
        r: Rect,
        fill: Color32,
        gap_right: bool,
    ) {
        let resp = ui.interact(r, Id::new(("knob", i)), Sense::click_and_drag());
        let info = PARAMS[i];
        let range = (info.max - info.min) as f32;
        let value = self.param(i);
        if resp.drag_started_by(PointerButton::Primary) {
            self.knob_drag = Some((i, value as f32, 0.0));
            setter.begin_set_parameter(self.params.by_index()[i]);
        }
        if let Some((k, start, travel)) = self.knob_drag
            && k == i
            && resp.dragged_by(PointerButton::Primary)
        {
            let fine = ui.input(|inp| inp.modifiers.command);
            let d = -resp.drag_delta().y / if fine { 16.0 } else { 1.0 };
            let travel = travel + d;
            self.knob_drag = Some((k, start, travel));
            let v = (start + travel / 200.0 * range)
                .round()
                .clamp(info.min as f32, info.max as f32) as i32;
            setter.set_parameter(self.params.by_index()[i], v);
        }
        if resp.drag_stopped_by(PointerButton::Primary) && self.knob_drag.map(|d| d.0) == Some(i) {
            self.knob_drag = None;
            setter.end_set_parameter(self.params.by_index()[i]);
        }
        if resp.hovered() {
            let scroll = ui.input(|inp| inp.raw_scroll_delta.y);
            if scroll != 0.0 {
                let step = (range / 64.0).max(1.0);
                self.set_param(
                    setter,
                    i,
                    value
                        + if scroll > 0.0 {
                            step as i32
                        } else {
                            -(step as i32)
                        },
                );
            }
            let text = param_text(i, value, |_, _| String::new());
            self.hint = format!("{}: {}", info.name, text);
        }
        let t = (value - info.min) as f32 / range;
        knob(
            ui.painter(),
            r,
            t,
            fill,
            i == 6,
            gap_right,
            self.knob_drag.map(|d| d.0) == Some(i),
        );
        menus::wheel_menu(self, ui, setter, &resp, i);
    }

    /// The scratch clock (TScratchWheel, parameter 8): drag to rotate, 65536 per turn; the mouse
    /// wheel turns it by 2048; releasing springs it back to no offset.
    fn scratch_clock(&mut self, ui: &mut Ui, setter: &ParamSetter, r: Rect) {
        let resp = ui.interact(r, Id::new("scratch"), Sense::click_and_drag());
        let c = r.center();
        let angle = |p: Pos2| (p.x - c.x).atan2(-(p.y - c.y));
        let param = self.params.by_index()[8];
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.scratch_drag = Some((angle(p), self.param(8) as f32));
            setter.begin_set_parameter(param);
        }
        if let (Some((last, v)), Some(p)) = (self.scratch_drag, resp.interact_pointer_pos())
            && resp.dragged_by(PointerButton::Primary)
        {
            let a = angle(p);
            let mut d = a - last;
            if d > std::f32::consts::PI {
                d -= std::f32::consts::TAU;
            }
            if d < -std::f32::consts::PI {
                d += std::f32::consts::TAU;
            }
            // The hand angle is pi - value/65536 * 2 pi: turning clockwise lowers the value.
            let v = (v - d / std::f32::consts::TAU * 65536.0).clamp(0.0, 65536.0);
            self.scratch_drag = Some((a, v));
            setter.set_parameter(param, v.round() as i32);
        }
        if resp.drag_stopped() && self.scratch_drag.is_some() {
            self.scratch_drag = None;
            setter.set_parameter(param, PARAMS[8].default);
            setter.end_set_parameter(param);
        }
        if resp.hovered() {
            let scroll = ui.input(|inp| inp.raw_scroll_delta.y);
            if scroll != 0.0 {
                let v = self.param(8);
                self.set_param(setter, 8, v + if scroll > 0.0 { 2048 } else { -2048 });
            }
            self.hint = format!(
                "Time offset (scratching): {}",
                param_text(8, self.param(8), |_, _| String::new())
            );
        }
        let playing = self.shared.status.playing.load(Ordering::Relaxed);
        let value = self.scratch_drag.map_or(self.param(8) as f32, |d| d.1);
        let live = playing.then_some(value);
        scratch_clock(
            ui.painter(),
            r,
            value,
            group_color(self.group()),
            self.scratch_drag.is_some(),
            live,
        );
        menus::wheel_menu(self, ui, setter, &resp, 8);
    }

    /// A slot grid (TPaintBoxSelector, 4 x 9): click selects and edits, Ctrl+click moves the
    /// edited slot, middle-click renames, right-click opens the WheelMenu, the wheel steps.
    fn slot_grid(&mut self, ui: &mut Ui, setter: &ParamSetter, bank: usize, r: Rect) {
        let (cols, rows) = (4usize, 9usize);
        let cw = r.width() / cols as f32;
        let ch = r.height() / rows as f32;
        let resp = ui.interact(r, Id::new(("grid", bank)), Sense::click_and_drag());
        let cell_at = |p: Pos2| -> Option<usize> {
            if !r.contains(p) {
                return None;
            }
            let c = (((p.x - r.min.x) / cw) as usize).min(cols - 1);
            let rr = (((p.y - r.min.y) / ch) as usize).min(rows - 1);
            Some(rr * cols + c)
        };
        let selected = self.param(bank);
        if let Some(p) = resp.interact_pointer_pos()
            && let Some(cell) = cell_at(p)
            && (resp.clicked_by(PointerButton::Primary) || resp.dragged_by(PointerButton::Primary))
            && cell as i32 != selected
        {
            if ui.input(|i| i.modifiers.command) && resp.clicked() {
                let from = selected.clamp(0, 35) as usize;
                self.shared.model.lock().move_slot(bank, from, cell);
                self.shared.send(Command::MoveSlot {
                    bank,
                    from,
                    to: cell,
                });
            }
            self.select_edited(setter, bank, cell, false);
            self.set_param(setter, bank, cell as i32);
        }
        if resp.clicked_by(PointerButton::Primary) || resp.drag_stopped() {
            if let Some(cell) = resp.interact_pointer_pos().and_then(cell_at) {
                self.select_edited(setter, bank, cell, false);
            }
            self.shared.send(Command::ClearKeyStack(bank));
        }
        if resp.clicked_by(PointerButton::Middle)
            && let Some(cell) = resp.interact_pointer_pos().and_then(cell_at)
        {
            self.select_edited(setter, bank, cell, false);
            {
                let name = self.shared.model.lock().banks[bank].slots[cell]
                    .name
                    .clone();
                self.dialogs.rename(bank, cell, name);
            };
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let v = (selected + if scroll > 0.0 { -1 } else { 1 }).clamp(0, 35);
                self.select_edited(setter, bank, v as usize, false);
                self.set_param(setter, bank, v);
            }
            if let Some(cell) = ui.ctx().pointer_hover_pos().and_then(cell_at) {
                self.hint = slot_hint(&self.shared.model.lock(), bank, cell);
            }
        }
        menus::wheel_menu(self, ui, setter, &resp, bank);
    }

    /// The column between the slots and the envelope: menu, snap, step, slide, freeze, map type.
    fn side_buttons(&mut self, ui: &mut Ui, setter: &ParamSetter, h: f32) {
        // MenuBtn: the envelope options menu.
        let mr = Rect::from_min_size(pos2(270.0, 4.0), vec2(23.0, 27.0));
        let r = ui.interact(mr, Id::new("menu_btn"), Sense::click());
        if r.hovered() {
            self.hint = "Options".into();
        }
        menus::env_options_menu(self, ui, setter, &r);

        // SnapSel: 7 cells of 27 px.
        let sr = Rect::from_min_size(pos2(270.0, 40.0), vec2(23.0, 189.0));
        let r = ui.interact(sr, Id::new("snap_sel"), Sense::click_and_drag());
        if let Some(pt) = r.interact_pointer_pos()
            && (r.clicked() || r.dragged())
        {
            self.snap = (((pt.y - sr.min.y) / 27.0) as usize).min(6);
        }
        for (i, hint) in SNAP_HINTS.iter().enumerate() {
            let cr = Rect::from_min_size(pos2(270.0, 40.0 + i as f32 * 27.0), vec2(23.0, 27.0));
            if ui.rect_contains_pointer(cr) {
                self.hint = format!("Snap: {hint}");
            }
        }

        // Step, Slide, Freeze toggles.
        let toggles = [
            (275.0, "Step editing (hold shift for pulse mode)"),
            (301.0, "Slide remaining points"),
            (327.0, "Freeze editing"),
        ];
        for (k, (y, hint)) in toggles.into_iter().enumerate() {
            let tr = Rect::from_min_size(pos2(270.0, y), vec2(23.0, 27.0));
            let r = ui.interact(tr, Id::new(("vbtn", k)), Sense::click());
            if r.clicked() {
                match k {
                    0 => self.step = !self.step,
                    1 => self.slide = !self.slide,
                    _ => self.freeze = !self.freeze,
                }
            }
            if r.hovered() {
                self.hint = hint.into();
            }
        }

        // MapTypeSel: time travel / volume.
        let _ = h;
        for k in 0..2 {
            let cr = Rect::from_min_size(pos2(270.0, 470.0 + k as f32 * 26.0), vec2(23.0, 26.0));
            let r = ui.interact(cr, Id::new(("maptype", k)), Sense::click());
            if r.clicked() {
                self.set_group(k);
                let _ = setter;
            }
            if r.hovered() {
                self.hint = format!(
                    "Envelope: {}",
                    if k == 0 { "time travel" } else { "volume" }
                );
            }
        }
    }

    /// The bar under the envelope: slot name and the slot settings of the edited slot.
    fn slot_bar(&mut self, ui: &mut Ui, setter: &ParamSetter, h: f32) {
        let p = ui.painter().clone();
        let group = self.group();
        let index = self.edited[group];
        let slot = self.shared.model.lock().banks[group].slots[index].clone();
        let y = h - 25.0;

        // NameLabel: click to rename.
        let nr = Rect::from_min_size(pos2(298.0, y), vec2(157.0, 15.0));
        let r = ui.interact(nr, Id::new("name_label"), Sense::click());
        let text = format!("{}: {}", GROUP_NAMES[group], slot.display_name(index));
        p.with_clip_rect(nr).text(
            pos2(nr.min.x, nr.center().y),
            Align2::LEFT_CENTER,
            text,
            font(12.0),
            group_color(group),
        );
        if r.clicked() {
            {
                let name = self.shared.model.lock().banks[group].slots[index]
                    .name
                    .clone();
                self.dialogs.rename(group, index, name);
            };
        }

        // Link (time group only), key-held and one-shot toggles.
        let buttons = [
            (457.0, "Link volume slot", slot.link != 0, group == 0),
            (
                480.0,
                "Key-held  (back to previous slot on key release)",
                slot.key_held != 0,
                true,
            ),
            (
                503.0,
                "One-shot (back to first slot once done)",
                slot.one_shot != 0,
                true,
            ),
        ];
        for (k, (x, hint, on, visible)) in buttons.into_iter().enumerate() {
            if !visible {
                continue;
            }
            let br = Rect::from_min_size(pos2(x, y + 2.0), vec2(22.0, 22.0));
            let r = ui.interact(br, Id::new(("hbtn", k)), Sense::click());
            if r.hovered() {
                self.hint = hint.into();
            }
            if r.clicked() {
                let rearm = k == 2;
                self.edit_slot(group, index, rearm, |s| match k {
                    0 => s.link ^= 1,
                    1 => s.key_held ^= 1,
                    _ => s.one_shot ^= 1,
                });
                let playing = self.shared.status.playing.load(Ordering::Relaxed);
                if k == 0 && !on && !playing {
                    self.edited[group ^ 1] = index;
                    self.set_param(setter, group ^ 1, index as i32);
                }
            }
        }

        // The three combos.
        let combos = [
            (
                530.0,
                76.0,
                &HOLD_ITEMS[..],
                slot.force_hold,
                "Force hold delay (bar repeat)",
            ),
            (
                610.0,
                104.0,
                &SYNC_ITEMS[..],
                slot.trigger_sync,
                "Slot trigger synchronization",
            ),
            (
                718.0,
                106.0,
                &SYNC_ITEMS[..],
                slot.position_sync,
                "Slot position synchronization",
            ),
        ];
        for (k, (x, w, items, value, hint)) in combos.into_iter().enumerate() {
            let cr = Rect::from_min_size(pos2(x, y + 2.0), vec2(w, 19.0));
            let r = ui.interact(cr, Id::new(("combo", k)), Sense::click());
            if r.hovered() {
                self.hint = hint.into();
            }
            let popup = Id::new(("combo_popup", k));
            if r.clicked() {
                ui.memory_mut(|mem| mem.toggle_popup(popup));
            }
            egui::popup::popup_below_widget(
                ui,
                popup,
                &r,
                egui::PopupCloseBehavior::CloseOnClick,
                |ui| {
                    ui.set_min_width(w);
                    for (v, it) in items.iter().enumerate() {
                        if ui.selectable_label(v as i32 == value, *it).clicked() {
                            self.edit_slot(group, index, k == 0, |s| match k {
                                0 => s.force_hold = v as i32,
                                1 => s.trigger_sync = v as i32,
                                _ => s.position_sync = v as i32,
                            });
                        }
                    }
                },
            );
        }
    }
}

/// The slot cell hint (FUN_0006cae0): settings, then the trigger note and the slot name.
pub fn slot_hint(m: &Engine, bank: usize, i: usize) -> String {
    let s = &m.banks[bank].slots[i];
    let mut parts = Vec::new();
    if s.link != 0 {
        parts.push("link".to_string());
    }
    if s.key_held != 0 {
        parts.push("key-held".into());
    }
    if s.one_shot != 0 {
        parts.push("one-shot".into());
    }
    if s.force_hold > 0 {
        parts.push(format!(
            "hold {}",
            HOLD_ITEMS.get(s.force_hold as usize).unwrap_or(&"?")
        ));
    }
    if (0..5).contains(&s.trigger_sync) {
        parts.push(format!("TS {}", SYNC_ITEMS[s.trigger_sync as usize]));
    }
    if (0..5).contains(&s.position_sync) {
        parts.push(format!("PS {}", SYNC_ITEMS[s.position_sync as usize]));
    }
    let mut head = parts.join(", ");
    if let Some(c) = head.get(..1) {
        head = c.to_uppercase() + &head[1..];
    }
    let note = m.banks[bank].base_note + i as i32;
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let name = format!("{}{}", NAMES[(note % 12) as usize], note / 12);
    let body = format!("{name} {}", s.display_name(i));
    if head.is_empty() {
        body
    } else {
        format!("{head} - {body}")
    }
}

/// The header art (TopBgPaintBoxPaint 0x6266c): a gradient shape with a notch for the scratch
/// Shows a tool refusal (the original's message box).
pub fn refusal(gui: &mut Gui, e: ToolError) {
    gui.dialogs.message = Some(e);
}

/// Headless renders of the editor for checking the layout against the thumbnail:
/// `cargo test -p backspin editor_snapshots -- --ignored` writes `target/editor-snapshots/*.png`.
#[cfg(test)]
mod snapshots {
    use super::*;
    use nih_plug::prelude::{GuiContext, ParamPtr, PluginApi, PluginState};

    struct NoHost;

    impl GuiContext for NoHost {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn request_resize(&self) -> bool {
            false
        }
        unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
        unsafe fn raw_set_parameter_normalized(&self, _param: ParamPtr, _normalized: f32) {}
        unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
        fn get_state(&self) -> PluginState {
            PluginState {
                version: String::new(),
                params: Default::default(),
                fields: Default::default(),
            }
        }
        fn set_state(&self, _state: PluginState) {}
    }

    /// Runs a few frames headless and rasterizes egui's triangles in software.
    fn render(name: &str, gui: Gui, setup: impl FnOnce(&mut Gui)) {
        let mut gui = gui;
        setup(&mut gui);
        let host = NoHost;
        let setter = ParamSetter::new(&host);
        let ctx = Context::default();
        let (w, h) = (EDITOR_SIZE.0 as usize, EDITOR_SIZE.1 as usize);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w as f32, h as f32))),
            ..Default::default()
        };
        let mut out = ctx.run(input(), |ctx| gui.frame(ctx, &setter));
        for _ in 0..2 {
            out = ctx.run(input(), |ctx| gui.frame(ctx, &setter));
        }
        let prims = ctx.tessellate(out.shapes, 1.0);
        let mut textures: std::collections::HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> =
            Default::default();
        for (id, delta) in &out.textures_delta.set {
            let (tw, th, px): (usize, usize, Vec<Color32>) = match &delta.image {
                egui::ImageData::Color(c) => (c.size[0], c.size[1], c.pixels.clone()),
                egui::ImageData::Font(f) => (f.size[0], f.size[1], f.srgba_pixels(None).collect()),
            };
            textures.insert(*id, (tw, th, px));
        }
        let mut img = vec![[0f32; 3]; w * h];
        for p in prims {
            let egui::epaint::Primitive::Mesh(mesh) = p.primitive else {
                continue;
            };
            let clip = p.clip_rect;
            let tex = textures.get(&mesh.texture_id);
            for t in mesh.indices.chunks(3) {
                let v = [
                    mesh.vertices[t[0] as usize],
                    mesh.vertices[t[1] as usize],
                    mesh.vertices[t[2] as usize],
                ];
                let min_x = v
                    .iter()
                    .map(|q| q.pos.x)
                    .fold(f32::MAX, f32::min)
                    .max(clip.min.x)
                    .max(0.0) as usize;
                let max_x = v
                    .iter()
                    .map(|q| q.pos.x)
                    .fold(f32::MIN, f32::max)
                    .min(clip.max.x)
                    .min(w as f32 - 1.0);
                let min_y = v
                    .iter()
                    .map(|q| q.pos.y)
                    .fold(f32::MAX, f32::min)
                    .max(clip.min.y)
                    .max(0.0) as usize;
                let max_y = v
                    .iter()
                    .map(|q| q.pos.y)
                    .fold(f32::MIN, f32::max)
                    .min(clip.max.y)
                    .min(h as f32 - 1.0);
                if max_x < 0.0 || max_y < 0.0 {
                    continue;
                }
                let area = (v[1].pos - v[0].pos).x * (v[2].pos - v[0].pos).y
                    - (v[1].pos - v[0].pos).y * (v[2].pos - v[0].pos).x;
                if area.abs() < 1e-9 {
                    continue;
                }
                for y in min_y..=max_y as usize {
                    for x in min_x..=max_x as usize {
                        let pt = pos2(x as f32 + 0.5, y as f32 + 0.5);
                        let e = |a: Pos2, b: Pos2| {
                            (b.x - a.x) * (pt.y - a.y) - (b.y - a.y) * (pt.x - a.x)
                        };
                        let w0 = e(v[1].pos, v[2].pos) / area;
                        let w1 = e(v[2].pos, v[0].pos) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let mut c = [0f32; 4];
                        for (k, q) in v.iter().enumerate() {
                            let wk = [w0, w1, w2][k];
                            let col = q.color.to_array();
                            for ch in 0..4 {
                                c[ch] += col[ch] as f32 / 255.0 * wk;
                            }
                        }
                        if let Some((tw, th, px)) = tex {
                            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                            let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                            let tx = ((u * *tw as f32) as usize).min(tw - 1);
                            let ty = ((vv * *th as f32) as usize).min(th - 1);
                            let s = px[ty * tw + tx].to_array();
                            for ch in 0..4 {
                                c[ch] *= s[ch] as f32 / 255.0;
                            }
                        }
                        // Premultiplied alpha over.
                        let d = &mut img[y * w + x];
                        for ch in 0..3 {
                            d[ch] = c[ch] + d[ch] * (1.0 - c[3]);
                        }
                    }
                }
            }
        }
        let bytes: Vec<u8> = img
            .iter()
            .flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/editor-snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(
            dir.join(format!("{name}.png")),
            &bytes,
            w as u32,
            h as u32,
            image::ColorType::Rgb8,
        )
        .unwrap();
    }

    fn gui_with(preset: Option<&str>) -> Gui {
        let (shared, _queues) = Shared::new();
        let shared = Arc::new(shared);
        if let Some(p) = preset {
            let path = format!(
                "/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat/{p}.fst"
            );
            if let Ok(data) = std::fs::read(path) {
                let blob = backspin_engine::fst_state(&data).unwrap().to_vec();
                shared.model.lock().load_state(&blob);
            }
        }
        let params = Arc::new(BackspinParams::new(shared.clone()));
        Gui::new(params, shared)
    }

    #[test]
    #[ignore = "writes images; run on demand"]
    fn editor_snapshots() {
        render("fresh", gui_with(None), |_| {});
        render("repeater_time", gui_with(Some("Repeater")), |g| {
            g.shared.model.lock().view_bank = 0;
            g.edited = [9, 2];
        });
        render("repeater_volume", gui_with(Some("Repeater")), |g| {
            g.shared.model.lock().view_bank = 1;
            g.edited = [9, 2];
        });
        render("stutter_compact", gui_with(Some("Stutter")), |g| {
            g.compact = true
        });
    }

    /// Runs frames with `events` (one list per frame) and returns the Gross Beat preset after.
    fn click_frames(gui: &mut Gui, frames: Vec<Vec<egui::Event>>) {
        let host = NoHost;
        let setter = ParamSetter::new(&host);
        let ctx = Context::default();
        let (w, h) = (EDITOR_SIZE.0 as f32, EDITOR_SIZE.1 as f32);
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| gui.frame(ctx, &setter));
        }
    }

    fn click(at: Pos2) -> Vec<Vec<egui::Event>> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        vec![
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
            vec![],
        ]
    }

    /// One click on the next arrow steps one Gross Beat preset.
    #[test]
    fn preset_arrow_steps_once() {
        let list = super::menus::state_presets();
        if list.len() < 3 {
            return;
        }
        let names: Vec<String> = list
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        let mut gui = gui_with(None);
        gui.gb_preset = names[0].clone();
        let r = backspin_ui::sb::layout::GB_NEXT;
        let at = pos2(
            (r.x0 + r.x1) as f32 * 0.5,
            ((r.y0 + r.y1) / 2 + backspin_ui::HEIGHT) as f32,
        );
        click_frames(&mut gui, click(at));
        assert_eq!(gui.gb_preset, names[1]);
    }
}
