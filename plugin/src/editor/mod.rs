//! The editor (ui.md): the slot column, the side buttons, the envelope editor and the slot bar.
//!
//! The editor works on the model engine in [`Shared`] and sends every change to the audio engine
//! (`Shared::send_slot` and friends). Host parameters go through the [`ParamSetter`].

mod browser;
mod dialogs;
mod envelope;
mod file_dialog;
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
    self, Color32, Context, Id, PointerButton, Pos2, Rect, Sense, Ui, pos2, vec2,
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
/// The width of the left column (S(266)).
const COLUMN: f32 = 266.0;

pub fn create(params: Arc<BackspinParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let state = Gui::new(params.clone(), shared);
    create_egui_editor(
        params.editor_state.clone(),
        state,
        |ctx, gui| {
            gui.on_open();
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
    /// The preset browser (a Backspin addition).
    pub browser: browser::Browser,
    /// A knob being dragged: parameter index, start value and the accumulated pointer travel.
    knob_drag: Option<(usize, f32, f32)>,
    /// The scratch clock drag: the last pointer angle and the current value.
    scratch_drag: Option<(f32, f32)>,
    /// The engine parameters last seen in the status, to follow MIDI slot selection.
    last_status: [i32; 2],
    /// The text shown in the hint area.
    pub hint: String,
    /// The local ShaperBox preset library, loaded on a background thread on first use.
    pub sb_library: Arc<parking_lot::Mutex<Option<Vec<backspin_presets::Group>>>>,
    pub sb_loading: bool,
    /// The ShaperBox section: the pack shown, the loaded preset file, the selected shaper and
    /// band.
    pub sb_pack: usize,
    pub sb_preset: Option<backspin_presets::Source>,
    pub sb_selected: usize,
    pub sb_band: usize,
    /// The last Gross Beat preset loaded from the browser or a file.
    pub gb_preset: Option<backspin_presets::Preset>,
    /// The shape mix knob drag and the wave view's band mix drag: the start value and the
    /// pointer travel.
    pub sb_mix_drag: Option<(f32, f32)>,
    pub sb_band_drag: Option<(f32, f32)>,
    /// The wave view's point drag: the point's index, its start `y` and the pointer travel.
    pub sb_point_drag: Option<(usize, f32, f32)>,
    /// The parsed shape state and wave tables, kept until the shape model changes.
    sb_cache: sb_section::SbCache,
    /// The code-exact frame of the open editor window.
    frame: frame::FrameCache,
    /// The file dialog that is open (behind a mutex: the editor state must be `Sync`).
    file_dialog: parking_lot::Mutex<Option<file_dialog::Pending>>,
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
            browser: browser::Browser::default(),
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
            gb_preset: None,
            sb_mix_drag: None,
            sb_band_drag: None,
            sb_point_drag: None,
            sb_cache: Default::default(),
            frame: frame::FrameCache::default(),
            file_dialog: parking_lot::Mutex::new(None),
        }
    }

    /// Called when an editor window opens: the state kept between windows must not refer to the
    /// previous window (its egui context owned the frame texture) or to its pointer drags.
    fn on_open(&mut self) {
        self.frame = frame::FrameCache::default();
        self.knob_drag = None;
        self.scratch_drag = None;
        self.sb_mix_drag = None;
        self.sb_band_drag = None;
        self.sb_point_drag = None;
        self.browser.open = None;
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
        file_dialog::poll(self, setter);
        self.follow_status(setter);
        self.hint.clear();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                // The Gross Beat form keeps its own height; the ShaperBox section sits below.
                let (w, h) = (full.width(), backspin_ui::HEIGHT as f32);
                let mut cache = std::mem::take(&mut self.frame);
                frame::show(self, ui, &mut cache);
                self.frame = cache;
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
        browser::show(self, ctx, setter);
        dialogs::show(self, ctx, setter);
    }

    /// The left column: header, knobs, HOLD, the scratch clock and the two slot grids.
    fn left_column(&mut self, ui: &mut Ui, setter: &ParamSetter, h: f32) {
        let clock = self.shared.model.lock().view_scratch != 0;

        // Time and volume mix knobs.
        self.knob(
            ui,
            setter,
            2,
            Rect::from_min_size(pos2(7.0, 5.0), vec2(48.0, 48.0)),
        );
        self.knob(
            ui,
            setter,
            3,
            Rect::from_min_size(pos2(7.0, h - 55.0), vec2(48.0, 48.0)),
        );

        // Options button.
        let opt = Rect::from_min_size(pos2(7.0, 66.0), vec2(20.0, 20.0));
        let r = ui.interact(opt, Id::new("options_btn"), Sense::click());
        if r.hovered() {
            self.hint = "Options".into();
        }
        menus::options_menu(self, ui, &r);

        // HOLD button (parameter 7, a direct-click toggle).
        let hold = Rect::from_min_size(pos2(109.0, 60.0), vec2(63.0, 30.0));
        let r = ui.interact(hold, Id::new("hold_btn"), Sense::click());
        let on = self.param(7) > 0;
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
        );
        self.knob(
            ui,
            setter,
            5,
            Rect::from_min_size(pos2(106.0, y), vec2(42.0, 42.0)),
        );
        self.knob(
            ui,
            setter,
            6,
            Rect::from_min_size(pos2(159.0, y - 4.0), vec2(48.0, 48.0)),
        );

        // SlotZoom: toggle the envelope.
        let zr = Rect::from_min_size(pos2(8.0, h - 88.0), vec2(24.0, 19.0));
        let r = ui.interact(zr, Id::new("slot_zoom"), Sense::click());
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
    fn knob(&mut self, ui: &mut Ui, setter: &ParamSetter, i: usize, r: Rect) {
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
        let group = self.group();
        let index = self.edited[group];
        let slot = self.shared.model.lock().banks[group].slots[index].clone();
        let y = h - 25.0;

        // NameLabel: click to rename.
        let nr = Rect::from_min_size(pos2(298.0, y), vec2(157.0, 15.0));
        let r = ui.interact(nr, Id::new("name_label"), Sense::click());
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
    use nih_plug::prelude::{GuiContext, Param, ParamPtr, PluginApi, PluginState};

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
        // Textures are uploaded in the frame that creates them, so keep every frame's uploads.
        let mut textures: std::collections::HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> =
            Default::default();
        let mut out = ctx.run(input(), |ctx| gui.frame(ctx, &setter));
        for k in 0..3 {
            for (id, delta) in &out.textures_delta.set {
                let (tw, th, px): (usize, usize, Vec<Color32>) = match &delta.image {
                    egui::ImageData::Color(c) => (c.size[0], c.size[1], c.pixels.clone()),
                    egui::ImageData::Font(f) => {
                        (f.size[0], f.size[1], f.srgba_pixels(None).collect())
                    }
                };
                match (delta.pos, textures.get_mut(id)) {
                    // A partial update of an existing texture.
                    (Some([x, y]), Some((stw, _, spx))) => {
                        for row in 0..th {
                            let (d, s) = ((y + row) * *stw + x, row * tw);
                            spx[d..d + tw].copy_from_slice(&px[s..s + tw]);
                        }
                    }
                    _ => {
                        textures.insert(*id, (tw, th, px));
                    }
                }
            }
            if k < 2 {
                out = ctx.run(input(), |ctx| gui.frame(ctx, &setter));
            }
        }
        let prims = ctx.tessellate(out.shapes, 1.0);
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
        // The shape section with a preset: some shapers on, one turned off, one hidden.
        if let Some(g) = gui_with_shape_named(Some("Patterns"), "16th Rhythm") {
            render("shape_section", g, |g| {
                g.shared.model.lock().view_bank = 0;
                g.edited = [14, 30];
            });
        }
        if let Some(g) = gui_with_shape_named(Some("Patterns"), "16th Rhythm") {
            render("shape_off", g, |g| {
                g.shared.sb.set_enabled(false);
                g.edited = [14, 30];
            });
        }
        // Volume steps between 0 and 100 %: vertical jumps and lines on the top and bottom edges.
        render("patterns_copter", gui_with(Some("Patterns")), |g| {
            g.shared.model.lock().view_bank = 1;
            g.edited = [0, 30];
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

    /// One click on the next arrow steps one shape preset of the shown group.
    #[test]
    fn preset_arrow_steps_once() {
        let lib = crate::sb::library();
        let Some(group) = lib.iter().position(|g| g.presets.len() >= 3) else {
            return;
        };
        let presets = lib[group].presets.clone();
        let mut gui = gui_with(None);
        *gui.sb_library.lock() = Some(lib);
        gui.sb_loading = true;
        gui.sb_pack = group;
        gui.sb_preset = Some(presets[0].source.clone());
        let r = backspin_ui::sb::layout::NEXT;
        let at = pos2(
            (r.x0 + r.x1) as f32 * 0.5,
            ((r.y0 + r.y1) / 2 + backspin_ui::HEIGHT) as f32,
        );
        click_frames(&mut gui, click(at));
        assert_eq!(gui.sb_preset, Some(presets[1].source.clone()));
    }

    /// Runs one frame in `ctx` and returns how many textures it created or updated.
    fn texture_uploads(ctx: &Context, gui: &mut Gui) -> usize {
        let host = NoHost;
        let setter = ParamSetter::new(&host);
        let (w, h) = (EDITOR_SIZE.0 as f32, EDITOR_SIZE.1 as f32);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| gui.frame(ctx, &setter));
        out.textures_delta
            .set
            .iter()
            .filter(|(_, d)| d.image.size() == [EDITOR_SIZE.0 as usize, EDITOR_SIZE.1 as usize])
            .count()
    }

    /// Closing and reopening the editor gives the new window its own frame texture, and an
    /// unchanged frame is not uploaded again.
    #[test]
    fn reopened_editor_uploads_its_frame() {
        let mut gui = gui_with(None);
        // A library finishing its background load between frames would change the frame.
        *gui.sb_library.lock() = Some(Vec::new());
        gui.sb_loading = true;
        let first = Context::default();
        gui.on_open();
        assert_eq!(texture_uploads(&first, &mut gui), 1);
        assert_eq!(texture_uploads(&first, &mut gui), 0);
        let second = Context::default();
        gui.on_open();
        assert_eq!(texture_uploads(&second, &mut gui), 1);
    }

    /// A host that records the parameter changes the editor asks for, as a DAW receives them.
    #[derive(Default)]
    struct RecordingHost {
        log: std::sync::Mutex<Vec<(&'static str, ParamPtr, f32)>>,
    }

    impl GuiContext for RecordingHost {
        fn plugin_api(&self) -> PluginApi {
            PluginApi::Clap
        }
        fn request_resize(&self) -> bool {
            false
        }
        unsafe fn raw_begin_set_parameter(&self, param: ParamPtr) {
            self.log.lock().unwrap().push(("begin", param, 0.0));
        }
        unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
            self.log.lock().unwrap().push(("set", param, normalized));
        }
        unsafe fn raw_end_set_parameter(&self, param: ParamPtr) {
            self.log.lock().unwrap().push(("end", param, 0.0));
        }
        fn get_state(&self) -> PluginState {
            PluginState {
                version: String::new(),
                params: Default::default(),
                fields: Default::default(),
            }
        }
        fn set_state(&self, _state: PluginState) {}
    }

    impl RecordingHost {
        /// The values set for `param`, and whether the change was wrapped in begin/end.
        fn sets(&self, param: ParamPtr) -> (Vec<f32>, bool) {
            let log = self.log.lock().unwrap();
            let mine: Vec<_> = log.iter().filter(|(_, p, _)| *p == param).collect();
            let values = mine
                .iter()
                .filter(|(k, _, _)| *k == "set")
                .map(|e| e.2)
                .collect();
            let wrapped = mine.first().is_some_and(|e| e.0 == "begin")
                && mine.last().is_some_and(|e| e.0 == "end");
            (values, wrapped)
        }
    }

    /// One editor window: a context kept across frames, with the time advancing at 60 Hz (drags
    /// and double-clicks need both).
    struct Session {
        ctx: Context,
        time: f64,
    }

    impl Session {
        fn new(gui: &mut Gui) -> Self {
            gui.on_open();
            Self {
                ctx: Context::default(),
                time: 0.0,
            }
        }

        /// Runs one frame per event list and returns how many frame textures were uploaded.
        fn run(
            &mut self,
            gui: &mut Gui,
            host: &dyn GuiContext,
            frames: Vec<Vec<egui::Event>>,
        ) -> usize {
            let setter = ParamSetter::new(host);
            let (w, h) = (EDITOR_SIZE.0 as f32, EDITOR_SIZE.1 as f32);
            let mut uploads = 0;
            for events in frames {
                self.time += 1.0 / 60.0;
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                };
                let out = self.ctx.run(input, |ctx| gui.frame(ctx, &setter));
                uploads += out
                    .textures_delta
                    .set
                    .iter()
                    .filter(|(_, d)| d.image.size() == [w as usize, h as usize])
                    .count();
            }
            uploads
        }
    }

    fn button(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// A left-button drag from `from` to `to` in `steps` moves.
    fn drag(from: Pos2, to: Pos2, steps: usize) -> Vec<Vec<egui::Event>> {
        let mut f = vec![
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
        ];
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            f.push(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        f.push(vec![button(to, false)]);
        f.push(vec![]);
        f
    }

    fn double_click(at: Pos2) -> Vec<Vec<egui::Event>> {
        vec![
            vec![egui::Event::PointerMoved(at)],
            vec![button(at, true)],
            vec![button(at, false)],
            vec![button(at, true)],
            vec![button(at, false)],
            vec![],
        ]
    }

    /// A section-local point in editor coordinates.
    fn section(x: i32, y: i32) -> Pos2 {
        pos2(x as f32, (y + backspin_ui::HEIGHT) as f32)
    }

    fn centre(r: backspin_gfx::Rect) -> Pos2 {
        section((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
    }

    /// A Gross Beat knob drag asks the host for a lower value (time mix starts at its maximum),
    /// inside one gesture.
    #[test]
    fn knob_drag_sets_the_parameter() {
        let mut gui = gui_with(None);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let start = gui.params.time_mix.unmodulated_normalized_value();
        s.run(&mut gui, &host, drag(pos2(31.0, 29.0), pos2(31.0, 79.0), 8));
        let (values, wrapped) = host.sets(gui.params.time_mix.as_ptr());
        assert!(wrapped, "no begin/end gesture");
        assert!(
            values.last().is_some_and(|&v| v < start),
            "{values:?} from {start}"
        );
    }

    /// Dragging an envelope point changes the slot in the model, sends it to the audio engine
    /// and repaints the frame.
    #[test]
    fn envelope_point_drag_reaches_the_engine() {
        let (shared, mut queues) = Shared::new();
        let shared = Arc::new(shared);
        let params = Arc::new(BackspinParams::new(shared.clone()));
        let mut gui = Gui::new(params, shared);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        while queues.commands.pop().is_ok() {}
        let (group, index) = (gui.group(), gui.edited[gui.group()]);
        let env = gui.shared.model.lock().banks[group].slots[index]
            .env
            .clone();
        // The editor's rect (298, 5, W - 304, H - 34) and its mapping.
        let r = Rect::from_min_size(pos2(298.0, 5.0), vec2(526.0, 501.0));
        let i = env.points.len() - 1;
        let at = pos2(
            r.min.x + (env.abs_pos(i) / env.length) as f32 * (r.width() - 1.0),
            r.min.y + (1.0 - env.points[i].value) * (r.height() - 1.0),
        );
        let to = pos2(at.x, (at.y + 120.0).min(r.max.y - 2.0).max(r.min.y + 2.0));
        let to = if (to.y - at.y).abs() < 10.0 {
            pos2(at.x, at.y - 120.0)
        } else {
            to
        };
        let uploads = s.run(&mut gui, &host, drag(at, to, 10));
        let after = gui.shared.model.lock().banks[group].slots[index]
            .env
            .clone();
        assert_ne!(
            after.points[i].value, env.points[i].value,
            "the point did not move"
        );
        let mut sent = false;
        while let Ok(c) = queues.commands.pop() {
            if let crate::shared::Command::Slot {
                bank,
                index: k,
                slot,
                ..
            } = c
            {
                sent |= bank == group
                    && k == index
                    && slot.env.points[i].value == after.points[i].value;
            }
        }
        assert!(sent, "the edited slot never reached the audio engine");
        assert!(uploads > 0, "the frame was not repainted");
    }

    /// Loads the first library preset whose shaper `k`'s middle band has an LFO into the shape
    /// section, and selects that shaper.
    /// The editor with Gross Beat preset `gb` and the library's shape preset `name` loaded.
    fn gui_with_shape_named(gb: Option<&str>, name: &str) -> Option<Gui> {
        let lib = crate::sb::library();
        let all = lib.first()?;
        let mut gui = gui_with(gb);
        let (i, p) = all
            .presets
            .iter()
            .enumerate()
            .find(|(_, p)| p.name == name)?;
        gui.shared.sb.load(&p.bytes()?).ok()?;
        gui.sb_preset = Some(p.source.clone());
        gui.sb_pack = 0;
        let _ = i;
        *gui.sb_library.lock() = Some(lib.clone());
        gui.sb_loading = true;
        let st = gui.shared.sb.model.lock().state();
        let k = (0..16).find(|&k| st.shapers[k].visible && st.shapers[k].on)?;
        gui.select_shaper(k);
        Some(gui)
    }

    fn gui_with_shape() -> Option<(Gui, usize)> {
        let lib = crate::sb::library();
        let all = lib.first()?;
        let mut gui = gui_with(None);
        let (k, bytes) = all.presets.iter().take(300).find_map(|p| {
            let data = p.bytes()?.into_owned();
            let tree = backspin_shaperbox::read_preset(&data).ok()?;
            let st = backspin_shaperbox::state::State::from_tree(&tree);
            let k = (0..16).find(|&k| {
                st.shapers[k].visible
                    && st.shapers[k].on
                    && st.shapers[k].bands[1].mix == 1.0
                    && !st.shapers[k].bands[1].sections.is_empty()
            })?;
            Some((k, data))
        })?;
        gui.shared.sb.load(&bytes).ok()?;
        *gui.sb_library.lock() = Some(lib);
        gui.sb_loading = true;
        gui.select_shaper(k);
        gui.sb_band = 1;
        Some((gui, k))
    }

    /// A downward drag in the wave view lowers the band's mix, rebuilds the processors when the
    /// drag ends and repaints the frame.
    #[test]
    fn wave_view_drag_sets_the_band_mix() {
        let Some((mut gui, k)) = gui_with_shape() else {
            return;
        };
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        gui.shared.sb.latency_dirty.store(false, Ordering::Relaxed);
        let w = backspin_ui::sb::layout::WAVE;
        let at = centre(w);
        let uploads = s.run(&mut gui, &host, drag(at, at + vec2(0.0, 60.0), 6));
        let mix = gui.shared.sb.model.lock().state().shapers[k].bands[1].mix;
        assert!(mix < 0.9, "mix {mix}");
        assert!(
            gui.shared.sb.latency_dirty.load(Ordering::Relaxed),
            "processors not rebuilt"
        );
        assert!(uploads > 0, "the frame was not repainted");
    }

    /// Point editing in the wave view: a drag moves a point, a double-click on empty space adds
    /// one and a double-click on a point deletes it.
    #[test]
    fn wave_view_edits_points() {
        use backspin_shaperbox::wave::{Point, render_table};
        let Some((mut gui, k)) = gui_with_shape() else {
            return;
        };
        let points = |g: &Gui| {
            g.shared.sb.model.lock().state().shapers[k].bands[1].sections[0]
                .lfo
                .points
                .clone()
        };
        let before = points(&gui);
        let w = backspin_ui::sb::layout::WAVE;
        let pad = backspin_ui::sb::WAVE_PAD;
        let (fw, fh) = (
            (w.x1 - w.x0 - 1) as f32,
            (w.y1 - w.y0 - 1) as f32 - 2.0 * pad,
        );
        // Where the painter draws point `i`'s marker: on the curve.
        let marker = |pts: &[Point], i: usize| {
            let table = render_table(pts);
            let px = pts[i].x as f32 / 8192.0;
            let v = table[((px * table.len() as f32) as usize).min(table.len() - 1)];
            section(w.x0, w.y0) + vec2(px * fw, pad + fh * (1.0 - (v + 1.0) * 0.5))
        };
        // A point whose marker is clear of the others.
        let Some(i) = (0..before.len()).find(|&i| {
            (0..before.len())
                .all(|j| j == i || marker(&before, i).distance(marker(&before, j)) > 14.0)
        }) else {
            return;
        };
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let from = marker(&before, i);
        let down = before[i].y > 0.0;
        let to = from + vec2(0.0, if down { 30.0 } else { -30.0 });
        let uploads = s.run(&mut gui, &host, drag(from, to, 6));
        let moved = points(&gui);
        assert_eq!(moved.len(), before.len());
        assert!(
            if down {
                moved[i].y < before[i].y
            } else {
                moved[i].y > before[i].y
            },
            "point {i} did not move: {:?} -> {:?}",
            before[i],
            moved[i]
        );
        assert!((moved[i].x - before[i].x).abs() <= (8192.0 / fw) as i32 + 1);
        assert_eq!(
            gui.shared.sb.model.lock().state().shapers[k].bands[1].mix,
            1.0,
            "the point drag changed the band mix"
        );
        assert!(uploads > 0, "the frame was not repainted");
        // Empty space a third of the way between two markers, near the top or bottom edge.
        let a = marker(&moved, 0).x;
        let b = moved
            .get(1)
            .map_or(section(w.x1, 0).x, |_| marker(&moved, 1).x);
        let y = if marker(&moved, 0).y > section(0, (w.y0 + w.y1) / 2).y {
            w.y0 + 8
        } else {
            w.y1 - 8
        };
        let empty = pos2(a + (b - a) / 3.0, section(0, y).y);
        s.run(&mut gui, &host, vec![vec![]; 40]);
        s.run(&mut gui, &host, double_click(empty));
        let added = points(&gui);
        assert_eq!(added.len(), moved.len() + 1, "no point added");
        let new = added
            .iter()
            .position(|p| !moved.contains(p))
            .expect("the new point");
        s.run(&mut gui, &host, vec![vec![]; 40]);
        s.run(&mut gui, &host, double_click(marker(&added, new)));
        assert_eq!(points(&gui), moved, "the new point was not deleted");
    }

    /// Every shaper: a click on its rack cell selects it, the power button turns it on and off,
    /// and the rack and the processors follow.
    #[test]
    fn every_shaper_power_button_toggles() {
        let Some((mut gui, _)) = gui_with_shape() else {
            return;
        };
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let r = backspin_ui::sb::layout::RACK;
        let (cw, ch) = ((r.x1 - r.x0) / 4, (r.y1 - r.y0) / 4);
        let p = backspin_ui::sb::layout::SHAPER_ON;
        let power = section((p.x0 + p.x1) / 2, (p.y0 + p.y1) / 2);
        let click = |at: Pos2| {
            let mut f = vec![
                vec![egui::Event::PointerMoved(at)],
                vec![button(at, true)],
                vec![button(at, false)],
            ];
            // A pause, so the next click is not a double click.
            f.extend(std::iter::repeat_n(vec![], 30));
            f
        };
        let on = |g: &Gui, k: usize| {
            let st = g.shared.sb.model.lock().state();
            st.shapers[k].visible && st.shapers[k].on
        };
        for k in 0..16 {
            let cell = section(
                r.x0 + (k as i32 % 4) * cw + cw / 2,
                r.y0 + (k as i32 / 4) * ch + ch / 2,
            );
            s.run(&mut gui, &host, click(cell));
            assert_eq!(gui.sb_selected, k, "cell {k} did not select its shaper");
            let before = on(&gui, k);
            gui.shared.sb.latency_dirty.store(false, Ordering::Relaxed);
            s.run(&mut gui, &host, click(power));
            assert_eq!(on(&gui, k), !before, "power did not toggle shaper {k}");
            assert!(
                gui.shared.sb.latency_dirty.load(Ordering::Relaxed),
                "shaper {k}: processors not rebuilt"
            );
            assert_eq!(gui.sb_selected, k, "power changed the selection");
            s.run(&mut gui, &host, click(power));
            assert_eq!(on(&gui, k), before, "power did not toggle shaper {k} back");
        }
    }

    /// The "shape" pill turns the whole section off and on, and the mix knob still drags at
    /// its right edge, where the pill starts.
    #[test]
    fn shape_pill_toggles_the_section() {
        let Some((mut gui, _)) = gui_with_shape() else {
            return;
        };
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let p = backspin_ui::sb::layout::PILL;
        let pill = section((p.x0 + p.x1) / 2 + 10, (p.y0 + p.y1) / 2);
        let enabled = |g: &Gui| g.shared.sb.model.lock().enabled;
        assert!(enabled(&gui));
        let mut click = vec![
            vec![egui::Event::PointerMoved(pill)],
            vec![button(pill, true)],
            vec![button(pill, false)],
        ];
        click.extend(std::iter::repeat_n(vec![], 30));
        let uploads = s.run(&mut gui, &host, click.clone());
        assert!(!enabled(&gui), "the pill did not turn the section off");
        assert!(uploads > 0, "the frame was not repainted");
        s.run(&mut gui, &host, click);
        assert!(enabled(&gui), "the pill did not turn the section back on");
        let k = backspin_ui::sb::layout::KNOB;
        let edge = section(k.x1 - 4, (k.y0 + k.y1) / 2);
        s.run(&mut gui, &host, drag(edge, edge + vec2(0.0, 80.0), 6));
        let mix = gui.shared.sb.model.lock().state().master_mix;
        assert!(mix < 0.9, "the knob edge did not drag the mix: {mix}");
        assert!(enabled(&gui), "the knob drag toggled the section");
    }

    /// Every toggle of the Gross Beat side: Step, Slide, Freeze, the snap cells, the envelope
    /// type and the slot's Link, Key-held and One-shot change their state and repaint the frame.
    #[test]
    fn every_gross_beat_toggle_works() {
        let mut gui = gui_with(Some("Patterns"));
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let click = |at: Pos2| {
            let mut f = vec![
                vec![egui::Event::PointerMoved(at)],
                vec![button(at, true)],
                vec![button(at, false)],
            ];
            f.extend(std::iter::repeat_n(vec![], 30));
            f
        };
        // Step, Slide and Freeze, each on and off again.
        type Flag = fn(&Gui) -> bool;
        let flags: [(f32, Flag); 3] = [
            (275.0, |g| g.step),
            (301.0, |g| g.slide),
            (327.0, |g| g.freeze),
        ];
        for (y, flag) in flags {
            let at = pos2(281.0, y + 13.0);
            let before = flag(&gui);
            let uploads = s.run(&mut gui, &host, click(at));
            assert_eq!(flag(&gui), !before, "toggle at y {y} did not change");
            assert!(uploads > 0, "toggle at y {y} did not repaint");
            s.run(&mut gui, &host, click(at));
            assert_eq!(flag(&gui), before, "toggle at y {y} did not change back");
        }
        // The snap cells.
        for i in (0..7).rev() {
            s.run(
                &mut gui,
                &host,
                click(pos2(281.0, 40.0 + i as f32 * 27.0 + 13.0)),
            );
            assert_eq!(gui.snap, i, "snap cell {i}");
        }
        // The envelope type: volume, then time.
        for k in [1, 0] {
            s.run(
                &mut gui,
                &host,
                click(pos2(281.0, 470.0 + k as f32 * 26.0 + 13.0)),
            );
            assert_eq!(gui.group(), k, "envelope type {k}");
        }
        // Link (time only), Key-held and One-shot of the edited slot.
        let y = EDITOR_SIZE.1 as f32 - backspin_ui::SB_HEIGHT as f32 - 25.0 + 2.0 + 11.0;
        let slot = |g: &Gui| {
            let m = g.shared.model.lock();
            let s = &m.banks[0].slots[g.edited[0]];
            [s.link, s.key_held, s.one_shot]
        };
        for (k, x) in [457.0f32, 480.0, 503.0].into_iter().enumerate() {
            let before = slot(&gui)[k];
            let uploads = s.run(&mut gui, &host, click(pos2(x + 11.0, y)));
            assert_ne!(slot(&gui)[k], before, "slot button {k} did not change");
            assert!(uploads > 0, "slot button {k} did not repaint");
            s.run(&mut gui, &host, click(pos2(x + 11.0, y)));
            assert_eq!(slot(&gui)[k], before, "slot button {k} did not change back");
        }
    }

    /// A double-click on a rack cell turns that shaper off, and another turns it back on.
    #[test]
    fn rack_double_click_toggles_the_shaper() {
        let Some((mut gui, k)) = gui_with_shape() else {
            return;
        };
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let r = backspin_ui::sb::layout::RACK;
        let (cw, ch) = ((r.x1 - r.x0) / 4, (r.y1 - r.y0) / 4);
        let cell = section(
            r.x0 + (k as i32 % 4) * cw + cw / 2,
            r.y0 + (k as i32 / 4) * ch + ch / 2,
        );
        let on = |g: &Gui| {
            let st = g.shared.sb.model.lock().state();
            st.shapers[k].visible && st.shapers[k].on
        };
        assert!(on(&gui));
        let uploads = s.run(&mut gui, &host, double_click(cell));
        assert!(!on(&gui), "the double-click did not turn it off");
        assert!(uploads > 0, "the frame was not repainted");
        // A pause, so the next two clicks are not a triple click.
        s.run(&mut gui, &host, vec![vec![]; 40]);
        s.run(&mut gui, &host, double_click(cell));
        assert!(on(&gui), "the second double-click did not turn it back on");
    }

    /// Clamp's ceiling knob and its toggle ask the host for their parameters.
    #[test]
    fn clamp_controls_set_their_parameters() {
        let mut gui = gui_with(None);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        s.run(&mut gui, &host, vec![vec![]]);
        let ceil = backspin_ui::sb::layout::CLAMP_KNOBS[2];
        s.run(
            &mut gui,
            &host,
            drag(centre(ceil), centre(ceil) + vec2(0.0, 40.0), 6),
        );
        let (values, wrapped) = host.sets(gui.params.clamp.ceiling.as_ptr());
        assert!(wrapped, "no begin/end gesture");
        assert!(values.last().is_some_and(|&v| v < 1.0), "{values:?}");
        let toggle = centre(backspin_ui::sb::layout::CLAMP);
        s.run(
            &mut gui,
            &host,
            vec![
                vec![egui::Event::PointerMoved(toggle)],
                vec![button(toggle, true)],
                vec![button(toggle, false)],
                vec![],
            ],
        );
        let (values, wrapped) = host.sets(gui.params.clamp.on.as_ptr());
        assert!(wrapped && values == [1.0], "{values:?}");
    }

    /// A preset file the dialog returns loads into the model and names the current preset.
    #[test]
    fn picked_preset_file_loads() {
        let Some(dir) = backspin_presets::paths::factory_presets() else {
            return;
        };
        let mut gui = gui_with(None);
        let host = NoHost;
        let setter = ParamSetter::new(&host);
        super::file_dialog::run(
            &mut gui,
            &setter,
            super::file_dialog::Action::GrossBeatPreset,
            &dir.join("Stutter.fst"),
        );
        assert_eq!(
            gui.gb_preset.as_ref().map(|p| p.name.as_str()),
            Some("Stutter")
        );
        let named = gui.shared.model.lock().banks[0]
            .slots
            .iter()
            .filter(|s| !s.name.is_empty())
            .count();
        assert!(named > 0, "the preset's slots did not load");
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// The slots named in the time bank (a loaded factory preset names some).
    fn named_slots(gui: &Gui) -> usize {
        gui.shared.model.lock().banks[0]
            .slots
            .iter()
            .filter(|s| !s.name.is_empty())
            .count()
    }

    /// The Gross Beat browser: typing in the search box and pressing Enter loads the first
    /// match, the arrow keys move the cursor, and Escape closes the browser.
    #[test]
    fn browser_search_and_keys_load_presets() {
        let folders = backspin_presets::gross_beat_folders(crate::sb::embedded());
        let factory = &folders[0];
        let Some(stutter) = factory.presets.iter().find(|p| p.name == "Stutter") else {
            return;
        };
        if factory.presets.len() < 3 {
            return;
        }
        let mut gui = gui_with(None);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        gui.browser.open(browser::Kind::GrossBeat);
        s.run(
            &mut gui,
            &host,
            vec![vec![], vec![egui::Event::Text("stutt".into())], vec![]],
        );
        s.run(&mut gui, &host, vec![vec![key(egui::Key::Enter)], vec![]]);
        assert_eq!(
            gui.gb_preset.as_ref().map(|p| &p.source),
            Some(&stutter.source),
            "the search did not load Stutter"
        );
        assert!(named_slots(&gui) > 0, "the preset did not reach the model");
        // Clearing the search lists the folder with the cursor on the loaded preset; Up twice
        // and Enter loads the preset two rows above it.
        let at = factory
            .presets
            .iter()
            .position(|p| p.name == "Stutter")
            .unwrap();
        if at < 2 {
            return;
        }
        gui.browser.search.clear();
        s.run(&mut gui, &host, vec![vec![], vec![]]);
        s.run(
            &mut gui,
            &host,
            vec![
                vec![key(egui::Key::ArrowUp)],
                vec![key(egui::Key::ArrowUp)],
                vec![key(egui::Key::Enter)],
                vec![],
            ],
        );
        assert_eq!(
            gui.gb_preset.as_ref().map(|p| &p.source),
            Some(&factory.presets[at - 2].source)
        );
        s.run(&mut gui, &host, vec![vec![key(egui::Key::Escape)], vec![]]);
        assert!(
            gui.browser.open.is_none(),
            "Escape did not close the browser"
        );
    }

    /// A click on a row loads that preset.
    #[test]
    fn browser_click_loads_a_preset() {
        let folders = backspin_presets::gross_beat_folders(crate::sb::embedded());
        let factory = &folders[0];
        if factory.presets.len() < 2 {
            return;
        }
        let key = factory
            .path
            .as_ref()
            .map_or_else(|| factory.name.clone(), |p| p.display().to_string());
        let mut gui = gui_with(None);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        gui.browser.open(browser::Kind::GrossBeat);
        s.run(&mut gui, &host, vec![vec![], vec![]]);
        let row = s
            .ctx
            .read_response(Id::new(("browser_row", &key, &factory.presets[1].name)))
            .expect("the second row is shown");
        s.run(&mut gui, &host, click(row.rect.center()));
        assert_eq!(
            gui.gb_preset.as_ref().map(|p| &p.source),
            Some(&factory.presets[1].source)
        );
    }

    /// A preset dragged onto a folder of the tree moves on disk, and the list follows.
    #[test]
    fn browser_drag_moves_a_preset() {
        let root = std::env::temp_dir().join(format!("backspin-browser-{}", std::process::id()));
        let (lib, sub) = (root.join("Lib"), root.join("Lib/Sub"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(lib.join("A.fst"), b"a").unwrap();
        std::fs::write(sub.join("B.fst"), b"b").unwrap();
        // SAFETY: the only test in this binary that sets the variable. The other tests only
        // read it, and they accept any imported folder.
        unsafe { std::env::set_var("BACKSPIN_CONFIG_DIR", root.join("config")) };
        backspin_presets::import_folder(&lib).unwrap();
        let mut gui = gui_with(None);
        let host = RecordingHost::default();
        let mut s = Session::new(&mut gui);
        gui.browser.show_folder(&lib);
        s.run(&mut gui, &host, vec![vec![], vec![]]);
        let (lib_key, sub_key) = (lib.display().to_string(), sub.display().to_string());
        let row = s
            .ctx
            .read_response(Id::new(("browser_row", &lib_key, "A")))
            .expect("the row of A");
        let folder = s
            .ctx
            .read_response(Id::new(("browser_folder", &sub_key)))
            .expect("the Sub folder");
        s.run(
            &mut gui,
            &host,
            drag(row.rect.center(), folder.rect.center(), 8),
        );
        assert!(
            sub.join("A.fst").is_file(),
            "A did not move: {:?}",
            gui.browser.error
        );
        assert!(!lib.join("A.fst").exists());
        s.run(&mut gui, &host, vec![vec![], vec![]]);
        assert!(
            s.ctx
                .read_response(Id::new(("browser_row", &sub_key, "A")))
                .is_some(),
            "the list does not show the moved preset"
        );
        backspin_presets::forget_folder(&lib).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The browser open on each kind of preset, for a look at the layout.
    #[test]
    #[ignore = "writes images; run on demand"]
    fn browser_snapshots() {
        render("browser_gross_beat", gui_with(Some("Repeater")), |g| {
            g.gb_preset = backspin_presets::gross_beat_folders(crate::sb::embedded())[0]
                .presets
                .iter()
                .find(|p| p.name == "Repeater")
                .cloned();
            g.browser.open(browser::Kind::GrossBeat);
        });
        render("browser_gross_beat_search", gui_with(None), |g| {
            g.browser.open(browser::Kind::GrossBeat);
            g.browser.search = "st".into();
        });
        render("browser_shape", gui_with(None), |g| {
            let lib = crate::sb::library();
            *g.sb_library.lock() = Some(lib);
            g.sb_loading = true;
            g.sb_pack = 1;
            g.browser.open(browser::Kind::Shape);
        });
    }
}
