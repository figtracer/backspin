//! The envelope editor (TMEnvEditor as Gross Beat configures it, ui.md section 5).
//!
//! The whole slot is always visible: no zoom, no scroll. Multi-select is on and the length is
//! fixed: the first point stays at 0 and the last at the slot length.

use super::paint::*;
use super::{Gui, SNAP_DIV, menus};
use backspin_engine::envelope::{Cursor, Envelope};
use backspin_engine::time_text;
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{
    self, CursorIcon, Id, Key, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2,
};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;

/// The hit radius of points and tension handles (S(8)).
const HIT: f32 = 8.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Hover {
    #[default]
    None,
    Point(usize),
    /// The tension handle of the segment that ends at this point.
    Handle(usize),
}

#[derive(Clone, Debug, Default)]
enum Drag {
    #[default]
    None,
    /// Moving the selected points: their start positions and values, and the pointer start.
    Points {
        anchor: usize,
        xs: Vec<f64>,
        vs: Vec<f32>,
        start: (f64, f64),
    },
    /// Moving a tension: the start tensions and the accumulated travel.
    Tension {
        i: usize,
        start: Vec<f32>,
        travel: f32,
    },
    /// Rubber band selection from this pointer position; `add` keeps the old selection.
    Rubber {
        from: Pos2,
        add: bool,
        base: Vec<bool>,
    },
    /// Step editing: the last pointer position.
    Step { last: Pos2, pulse: bool },
    /// Step erase: the last pointer x.
    Erase { last: f32 },
}

/// The editor's interaction state.
#[derive(Default)]
pub struct EnvState {
    hover: Hover,
    drag: Drag,
    /// The point the point menu belongs to, and where it opened.
    pub point_menu: Option<(usize, Pos2)>,
    /// The locator trail per group: past x positions, newest first.
    trail: [VecDeque<f32>; 2],
}

impl EnvState {
    pub fn clear(&mut self) {
        self.hover = Hover::None;
        self.drag = Drag::None;
        self.point_menu = None;
    }
}

/// Maps between envelope coordinates and screen pixels.
#[derive(Clone, Copy)]
struct View {
    r: Rect,
    len: f64,
}

impl View {
    fn x(&self, pos: f64) -> f32 {
        self.r.min.x + (pos / self.len) as f32 * self.r.width()
    }
    fn y(&self, v: f64) -> f32 {
        self.r.min.y + (1.0 - v as f32) * self.r.height()
    }
    fn pos(&self, x: f32) -> f64 {
        ((x - self.r.min.x) / self.r.width()) as f64 * self.len
    }
    fn val(&self, y: f32) -> f64 {
        (1.0 - (y - self.r.min.y) / self.r.height()) as f64
    }
}

/// The values of an envelope at every pixel column (the non-looping stepper from 0).
fn columns(env: &Envelope, v: &View) -> Vec<Pos2> {
    let n = v.r.width().round().max(2.0) as usize;
    let step = v.len / (n - 1) as f64;
    let mut c = Cursor::new(env);
    c.step_once(env, 0.0);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if i > 0 {
            c.step_once(env, step);
        }
        out.push(pos2(
            v.r.min.x + i as f32 * v.r.width() / (n - 1) as f32,
            v.y(c.out),
        ));
    }
    out
}

/// The tension handle position of the segment that ends at point `i`.
fn handle_pos(env: &Envelope, v: &View, i: usize) -> Option<Pos2> {
    if i == 0 {
        return None;
    }
    let (a, b) = (env.abs_pos(i - 1), env.abs_pos(i));
    if v.x(b) - v.x(a) <= HIT {
        return None;
    }
    let m = (a + b) * 0.5;
    Some(pos2(v.x(m), v.y(env.value_at(m))))
}

/// The time and value grids of the snap setting.
fn grid(gui: &Gui) -> (f64, f64) {
    let div = SNAP_DIV[gui.snap] as f64;
    (1.0 / div, 1.0 / (8.0 * div))
}

fn snap(x: f64, g: f64) -> f64 {
    (x / g).round() * g
}

/// The vertical axis text of a value (0x6bc10 kind 1): the delay `-(1 - v) * 8` beats.
fn value_text(v: f64) -> String {
    format!("-{}", time_text((1.0 - v) * 8.0, false))
}

/// Shows the editor in `r`.
pub fn show(gui: &mut Gui, ui: &mut Ui, setter: &ParamSetter, r: Rect) {
    let group = gui.group();
    let index = gui.edited[group];
    let (mut env, ghost) = {
        let m = gui.shared.model.lock();
        let other = &m.banks[group ^ 1].slots[gui.edited[group ^ 1]];
        (
            m.banks[group].slots[index].env.clone(),
            (!other.empty).then(|| other.env.clone()),
        )
    };
    let v = View { r, len: env.length };
    let resp = ui.interact(r, Id::new("envelope"), Sense::click_and_drag());
    let mut changed = false;
    if !gui.freeze {
        changed = interact(gui, ui, setter, &resp, &v, &mut env);
        changed |= shortcuts(gui, ui, &resp, &mut env);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
    }
    if let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| r.contains(*p)) {
        gui.hint = match gui.env.hover {
            Hover::Point(i) if !gui.freeze => format!(
                "Point {}/{}: {} {}",
                i + 1,
                env.points.len(),
                time_text(env.abs_pos(i), true),
                value_text(env.points[i].value as f64)
            ),
            Hover::Handle(i) if !gui.freeze => format!(
                "{:.0}% - Point {} to {} tension",
                env.points[i].tension * 100.0,
                i,
                i + 1
            ),
            _ => {
                let pos = v.pos(p.x).clamp(0.0, v.len);
                format!(
                    "In: {} out: {}",
                    time_text(pos, true),
                    value_text(env.value_at(pos))
                )
            }
        };
    }
    if changed {
        gui.edit_slot(group, index, false, |s| s.env = env.clone());
    }
    menus::point_menu(gui, ui, setter, &v.r);
    draw(gui, ui, &v, &env, ghost.as_ref(), group, index);
}

/// The mini preview of compact mode (72 x 72, editing frozen).
pub fn preview(gui: &mut Gui, ui: &mut Ui, r: Rect) {
    let env = gui.shared.model.lock().banks[0].slots[gui.edited[0]]
        .env
        .clone();
    let v = View { r, len: env.length };
    let p = ui.painter();
    p.rect_filled(r, 0.0, rgb(0x344b5b));
    p.add(Shape::line(columns(&env, &v), Stroke::new(1.0f32, TIME)));
    p.rect_stroke(
        r,
        0.0,
        Stroke::new(2.0f32, rgba(0x000000, 0x78)),
        egui::StrokeKind::Inside,
    );
    p.rect_stroke(
        r.shrink(2.0),
        0.0,
        Stroke::new(1.0f32, rgba(0x000000, 0x3c)),
        egui::StrokeKind::Inside,
    );
}

/// Mouse gestures (EnvVisualMouseDown 0xb25c0). Returns true if the envelope changed.
fn interact(
    gui: &mut Gui,
    ui: &mut Ui,
    setter: &ParamSetter,
    resp: &egui::Response,
    v: &View,
    env: &mut Envelope,
) -> bool {
    let mods = ui.input(|i| i.modifiers);
    let snap_on = (gui.snap > 0) ^ mods.alt;
    let (gx, gy) = grid(gui);
    let mut changed = false;
    let pointer = resp
        .interact_pointer_pos()
        .or_else(|| ui.ctx().pointer_hover_pos());

    // Hover.
    if matches!(gui.env.drag, Drag::None)
        && let Some(p) = pointer.filter(|p| v.r.contains(*p))
    {
        gui.env.hover = Hover::None;
        let mut best = HIT;
        if !gui.step {
            for (i, _) in env.points.iter().enumerate() {
                let q = pos2(v.x(env.abs_pos(i)), v.y(env.points[i].value as f64));
                let d = q.distance(p);
                if d <= best {
                    best = d;
                    gui.env.hover = Hover::Point(i);
                }
            }
        }
        if gui.env.hover == Hover::None && (!gui.step || snap_on) {
            for i in 1..env.points.len() {
                if let Some(q) = handle_pos(env, v, i)
                    && q.distance(p) <= HIT
                {
                    gui.env.hover = Hover::Handle(i);
                }
            }
        }
        ui.ctx().set_cursor_icon(match gui.env.hover {
            Hover::Point(_) => CursorIcon::Grab,
            Hover::Handle(_) => CursorIcon::ResizeVertical,
            Hover::None if gui.step => CursorIcon::Cell,
            Hover::None => CursorIcon::Crosshair,
        });
    }

    let Some(p) = pointer else { return false };
    let pos = v.pos(p.x).clamp(0.0, v.len);
    let val = v.val(p.y).clamp(0.0, 1.0);
    let pressed = |b| ui.input(|i| i.pointer.button_pressed(b)) && v.r.contains(p);
    let double =
        ui.input(|i| i.pointer.button_double_clicked(PointerButton::Primary)) && v.r.contains(p);

    // Double-click: delete a point, reset a tension, or add a point.
    if double {
        match gui.env.hover {
            Hover::Point(i) => {
                env.delete_point(i);
                gui.env.hover = Hover::None;
            }
            Hover::Handle(i) => env.points[i].tension = 0.0,
            Hover::None => {
                add_point(gui, env, v, p, snap_on, mods.shift);
            }
        }
        env.prepare();
        gui.env.drag = Drag::None;
        return true;
    }

    if pressed(PointerButton::Primary) {
        gui.env.point_menu = None;
        if gui.step {
            gui.env.drag = Drag::Step {
                last: p,
                pulse: mods.shift,
            };
            changed |= step_edit(gui, env, v, p, p, snap_on, mods.shift);
        } else {
            match gui.env.hover {
                Hover::Point(i) if mods.alt => {
                    env.delete_point(i);
                    gui.env.hover = Hover::None;
                    changed = true;
                }
                Hover::Point(i) => {
                    if !env.points[i].selected() {
                        env.deselect_all();
                        env.points[i].set_selected(true);
                        changed = true;
                    }
                    let xs = (0..env.points.len()).map(|k| env.abs_pos(k)).collect();
                    let vs = env.points.iter().map(|q| q.value).collect();
                    gui.env.drag = Drag::Points {
                        anchor: i,
                        xs,
                        vs,
                        start: (pos, val),
                    };
                }
                Hover::Handle(i) if mods.alt => {
                    env.points[i].tension = 0.0;
                    env.prepare();
                    changed = true;
                }
                Hover::Handle(i) => {
                    let start = env.points.iter().map(|q| q.tension).collect();
                    gui.env.drag = Drag::Tension {
                        i,
                        start,
                        travel: 0.0,
                    };
                }
                Hover::None if mods.command || mods.shift => {
                    let base = env
                        .points
                        .iter()
                        .map(|q| q.selected() && mods.shift)
                        .collect();
                    gui.env.drag = Drag::Rubber {
                        from: p,
                        add: mods.shift,
                        base,
                    };
                }
                Hover::None => {
                    if env.selected_count() > 0 {
                        env.deselect_all();
                        changed = true;
                    }
                }
            }
        }
    } else if pressed(PointerButton::Secondary) {
        if gui.step {
            gui.env.drag = Drag::Erase { last: p.x };
            changed |= erase(env, v, p.x, p.x);
        } else {
            match gui.env.hover {
                Hover::Point(i) if mods.alt => {
                    env.delete_point(i);
                    gui.env.hover = Hover::None;
                    changed = true;
                }
                Hover::Point(i) => {
                    if !env.points[i].selected() {
                        env.deselect_all();
                        env.points[i].set_selected(true);
                        changed = true;
                    }
                    gui.env.point_menu = Some((i, p));
                }
                Hover::Handle(i) => {
                    env.points[i].tension = 0.0;
                    env.prepare();
                    changed = true;
                }
                Hover::None => {
                    if let Some(i) = add_point(gui, env, v, p, snap_on, mods.shift) {
                        let xs = (0..env.points.len()).map(|k| env.abs_pos(k)).collect();
                        let vs = env.points.iter().map(|q| q.value).collect();
                        gui.env.drag = Drag::Points {
                            anchor: i,
                            xs,
                            vs,
                            start: (env.abs_pos(i), env.points[i].value as f64),
                        };
                    }
                    changed = true;
                }
            }
        }
    } else if ui.input(|i| i.pointer.button_pressed(PointerButton::Middle))
        && let Hover::Point(i) = gui.env.hover
    {
        let on = env.points[i].selected();
        env.points[i].set_selected(!on);
        changed = true;
    }

    let down = ui.input(|i| i.pointer.any_down());
    match &mut gui.env.drag {
        Drag::None => {}
        Drag::Points {
            anchor,
            xs,
            vs,
            start,
        } => {
            let (anchor, xs, vs, start) = (*anchor, xs.clone(), vs.clone(), *start);
            let free = mods.shift && mods.command;
            let snapping = snap_on && !free;
            let mut dpos = if mods.command && !free {
                0.0
            } else {
                pos - start.0
            };
            let mut dval = if mods.shift && !free {
                0.0
            } else {
                val - start.1
            };
            if snapping {
                if dpos != 0.0 {
                    dpos = snap(xs[anchor] + dpos, gx) - xs[anchor];
                }
                if dval != 0.0 {
                    dval = snap(vs[anchor] as f64 + dval, gy) - vs[anchor] as f64;
                }
            }
            move_points(env, &xs, &vs, dpos, dval, gui.slide);
            changed = true;
            if !down {
                gui.env.drag = Drag::None;
            }
        }
        Drag::Tension { i, start, travel } => {
            let i = *i;
            let dy = ui.input(|inp| inp.pointer.delta().y);
            let fine = if mods.command { 20.0 } else { 1.0 };
            *travel += -dy / fine;
            let falling = env.points[i].value < env.points[i - 1].value;
            let d = *travel / 200.0 * if falling { -1.0 } else { 1.0 };
            for (k, q) in env.points.iter_mut().enumerate() {
                if k == i || (q.selected() && env_selected_peer(k, i)) {
                    q.tension = (start[k] + d).clamp(-1.0, 1.0);
                }
            }
            gui.last_tension = env.points[i].tension;
            env.prepare();
            changed = true;
            ui.ctx().set_cursor_icon(CursorIcon::None);
            if !down {
                gui.env.drag = Drag::None;
            }
        }
        Drag::Rubber { from, add, base } => {
            let rr = Rect::from_two_pos(*from, p);
            let abs: Vec<f64> = (0..env.points.len()).map(|k| env.abs_pos(k)).collect();
            for (k, q) in env.points.iter_mut().enumerate() {
                let inside = rr.contains(pos2(v.x(abs[k]), v.y(q.value as f64)));
                q.set_selected(base[k] || inside);
            }
            ui.painter().rect(
                rr,
                3.0,
                rgba(0xa8e44a, 0x14),
                Stroke::new(1.0f32, if *add { rgb(0x4492fb) } else { SELECT_RING }),
                egui::StrokeKind::Middle,
            );
            changed = true;
            if !down {
                gui.env.drag = Drag::None;
            }
        }
        Drag::Step { last, pulse } => {
            let (from, pulse) = (*last, *pulse);
            changed |= step_edit(gui, env, v, from, p, snap_on, pulse);
            if let Drag::Step { last, .. } = &mut gui.env.drag {
                *last = p;
            }
            if !down {
                gui.env.drag = Drag::None;
            }
        }
        Drag::Erase { last } => {
            let from = *last;
            *last = p.x;
            changed |= erase(env, v, from, p.x);
            if !down {
                gui.env.drag = Drag::None;
            }
        }
    }
    let _ = setter;
    changed
}

/// Every selected point gets the same tension change as the dragged one.
fn env_selected_peer(k: usize, i: usize) -> bool {
    k != i
}

/// Adds a point at the pointer ("add point"). Returns its index, or `None` if the click was on
/// the curve with snap off (the point goes on the curve and the gesture ends).
fn add_point(
    gui: &mut Gui,
    env: &mut Envelope,
    v: &View,
    p: Pos2,
    snap_on: bool,
    shift: bool,
) -> Option<usize> {
    let (gx, gy) = grid(gui);
    let mut pos = v.pos(p.x).clamp(0.0, v.len);
    let mut val = v.val(p.y).clamp(0.0, 1.0);
    let on_curve = env.value_at(pos);
    let mut follow = true;
    if snap_on {
        pos = snap(pos, gx).clamp(0.0, v.len);
        val = snap(val, gy).clamp(0.0, 1.0);
    } else if (v.y(on_curve) - p.y).abs() <= HIT {
        val = on_curve;
        follow = false;
    }
    if shift {
        val = env.value_at(pos);
    }
    let i = env.insert_point(pos, val as f32, gui.last_tension, true, false);
    env.points[i].shape = gui.default_shape;
    env.points[i].set_selected(true);
    env.prepare();
    follow.then_some(i)
}

/// Moves the selected points by `dpos` and `dval` from their start positions. Time stays between
/// the neighbouring unselected points and inside the length; the first and the last point keep
/// their time. With `slide`, every point after the selection moves by the same time.
fn move_points(env: &mut Envelope, xs: &[f64], vs: &[f32], dpos: f64, dval: f64, slide: bool) {
    let n = env.points.len();
    if n != xs.len() {
        return;
    }
    let sel: Vec<bool> = env.points.iter().map(|q| q.selected()).collect();
    let last_sel = (0..n).rev().find(|&k| sel[k]).unwrap_or(0);
    let moves = |k: usize| k != 0 && k != n - 1 && (sel[k] || (slide && k > last_sel));
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for k in 0..n {
        if !moves(k) {
            continue;
        }
        let prev = (0..k).rev().find(|&j| !moves(j)).map_or(0.0, |j| xs[j]);
        let next = (k + 1..n)
            .find(|&j| !moves(j))
            .map_or(env.length, |j| xs[j]);
        lo = lo.max(prev - xs[k]);
        hi = hi.min(next - xs[k]);
    }
    let dt = if lo <= hi { dpos.clamp(lo, hi) } else { 0.0 };
    let mut abs: Vec<f64> = xs.to_vec();
    for k in 0..n {
        if moves(k) {
            abs[k] = xs[k] + dt;
        }
        if sel[k] {
            env.points[k].value = (vs[k] as f64 + dval).clamp(0.0, 1.0) as f32;
        }
    }
    let mut prev = 0.0;
    for (k, q) in env.points.iter_mut().enumerate() {
        q.dx = if k == 0 { abs[0] } else { abs[k] - prev };
        prev = abs[k];
    }
    env.prepare();
}

/// Step editing between two pointer positions: every pixel column crossed gets a point with the
/// value interpolated along the drag. A point inside the column (or the snap step) moves,
/// otherwise a new point goes in. Shift ("pulse mode") makes new points Hold with tension 0.
fn step_edit(
    gui: &mut Gui,
    env: &mut Envelope,
    v: &View,
    from: Pos2,
    to: Pos2,
    snap_on: bool,
    pulse: bool,
) -> bool {
    let (gx, gy) = grid(gui);
    let (x0, x1) = (from.x.min(to.x), from.x.max(to.x));
    let mut changed = false;
    let mut x = x0.floor();
    while x <= x1.ceil() {
        let t = if (to.x - from.x).abs() < 1e-3 {
            1.0
        } else {
            (x - from.x) / (to.x - from.x)
        };
        let y = from.y + (to.y - from.y) * t.clamp(0.0, 1.0);
        let mut pos = v.pos(x).clamp(0.0, v.len);
        let mut val = v.val(y).clamp(0.0, 1.0);
        let tol = if snap_on {
            pos = snap(pos, gx).clamp(0.0, v.len);
            val = snap(val, gy).clamp(0.0, 1.0);
            gx * 0.5
        } else {
            v.len / v.r.width() as f64 * 0.5
        };
        let found = (0..env.points.len()).find(|&k| (env.abs_pos(k) - pos).abs() <= tol);
        match found {
            Some(k) => {
                if env.points[k].value != val as f32 {
                    env.points[k].value = val as f32;
                    changed = true;
                }
            }
            None => {
                let i = env.insert_point(
                    pos,
                    val as f32,
                    if pulse { 0.0 } else { gui.last_tension },
                    true,
                    false,
                );
                env.points[i].shape = if pulse { 2 } else { gui.default_shape };
                changed = true;
            }
        }
        x += if snap_on {
            (gx / v.len) as f32 * v.r.width()
        } else {
            1.0
        }
        .max(1.0);
    }
    env.prepare();
    changed
}

/// Step erase: deletes every point in the columns crossed (except the first and the last).
fn erase(env: &mut Envelope, v: &View, a: f32, b: f32) -> bool {
    let (lo, hi) = (v.pos(a.min(b) - 0.5), v.pos(a.max(b) + 0.5));
    let mut changed = false;
    let mut k = env.points.len();
    while k > 0 {
        k -= 1;
        let x = env.abs_pos(k);
        if x >= lo && x <= hi && env.can_delete(k) {
            env.delete_point(k);
            changed = true;
        }
    }
    changed
}

/// The EnvOptionsMenu shortcuts (Ctrl+A, D, I, X, C, V, R, B and Del) while the pointer is over
/// the editor.
fn shortcuts(gui: &mut Gui, ui: &Ui, resp: &egui::Response, env: &mut Envelope) -> bool {
    if !resp.hovered() {
        return false;
    }
    let (cmd, keys) = ui.input(|i| {
        let keys: Vec<Key> = [
            Key::A,
            Key::D,
            Key::I,
            Key::X,
            Key::C,
            Key::V,
            Key::R,
            Key::B,
            Key::Delete,
            Key::Backspace,
        ]
        .into_iter()
        .filter(|k| i.key_pressed(*k))
        .collect();
        (i.modifiers.command, keys)
    });
    let mut changed = false;
    for k in keys {
        changed = true;
        match (cmd, k) {
            (true, Key::A) => env.select_all(),
            (true, Key::D) => env.deselect_all(),
            (true, Key::I) => env.invert_selection(),
            (true, Key::X) => env.copy_points(true),
            (true, Key::C) => env.copy_points(false),
            (true, Key::V) => {
                if let Err(e) = env.paste_points(false) {
                    super::refusal(gui, e);
                }
            }
            (true, Key::R) => {
                if let Err(e) = env.paste_points(true) {
                    super::refusal(gui, e);
                }
            }
            (true, Key::B) => {
                if let Err(e) = env.duplicate() {
                    super::refusal(gui, e);
                }
            }
            (_, Key::Delete | Key::Backspace) => {
                if let Err(e) = env.delete_selected() {
                    super::refusal(gui, e);
                }
            }
            _ => changed = false,
        }
    }
    changed
}

/// Draws what the code-exact frame does not (it paints the background, grid, watermark, axis
/// tags, curve and points): the freeze tint, the other group's ghost, hover feedback and, below,
/// the playback locator.
fn draw(
    gui: &mut Gui,
    ui: &Ui,
    v: &View,
    env: &Envelope,
    ghost: Option<&Envelope>,
    group: usize,
    index: usize,
) {
    let p = ui.painter().with_clip_rect(v.r);
    if gui.freeze {
        p.rect_filled(v.r, 0.0, rgba(0x2fc8fb, 32));
    }
    // The other group's envelope as a ghost line.
    if let Some(g) = ghost {
        let c = if group == 0 {
            rgb(0x7d6e5c)
        } else {
            rgb(0x5c8556)
        };
        p.add(Shape::line(columns(g, v), Stroke::new(1.0f32, c)));
    }
    let gc = group_color(group);
    if !gui.freeze {
        let pc = lighten(gc, 0x20);
        if let Hover::Handle(i) = gui.env.hover
            && let Some(q) = handle_pos(env, v, i)
        {
            p.circle_stroke(
                q,
                env.points[i].tension.abs() * 1.9 + 2.5,
                Stroke::new(1.0f32, pc),
            );
        }
        if let Hover::Point(i) = gui.env.hover
            && let Some(q) = env.points.get(i)
        {
            let c = pos2(v.x(env.abs_pos(i)), v.y(q.value as f64));
            p.circle_filled(c, 9.0, rgba(0xffffff, 0x20));
        }
    }
    // The playback locator with its fading trail.
    let status = &gui.shared.status;
    let playing = status.playing.load(Ordering::Relaxed);
    let trail = &mut gui.env.trail[group];
    if playing && status.active[group].load(Ordering::Relaxed) == index as i32 {
        let x = v.x(status.position(group));
        trail.push_front(x);
        trail.truncate(24);
        let lc = lighten(gc, 0x40);
        for (k, w) in trail.iter().zip(trail.iter().skip(1)).enumerate() {
            let (x1, x0) = (*w.0, *w.1);
            if x0 < x1 && x1 - x0 < v.r.width() * 0.5 {
                let a = (0.82f32.powi(k as i32 + 1) * 128.0) as u8;
                p.rect_filled(
                    Rect::from_min_max(pos2(x0, v.r.min.y), pos2(x1, v.r.max.y)),
                    0.0,
                    alpha(lc, a / 3),
                );
            }
        }
        p.line_segment(
            [pos2(x, v.r.min.y), pos2(x, v.r.max.y)],
            Stroke::new(1.0f32, lc),
        );
        ui.ctx().request_repaint();
    } else {
        trail.clear();
    }
}
