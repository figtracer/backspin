//! The preset browser (a Backspin addition): a panel over the envelope with the folders on the
//! left and the presets of the chosen folder on the right. It shows the Gross Beat state presets
//! (Factory, User and the imported folders) or the shape presets (ShaperBox's packs). A search
//! box filters every folder. Presets in writable folders move by a drag onto a folder of the
//! tree or from the right-click menu, which also renames them; read-only presets copy to the
//! user's folder. The disk work is in `backspin_presets`; errors show in the panel's footer.

use super::menus::load_preset;
use super::paint::*;
use super::{Gui, file_dialog, sb_section};
use backspin_presets::{Folder, Group, Preset, Source};
use backspin_ui::sb::ACCENT;
use nih_plug::prelude::ParamSetter;
use nih_plug_egui::egui::{
    self, Align, Align2, Color32, Context, CursorIcon, DragAndDrop, Id, Key, LayerId, Layout,
    Order, Rect, Response, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui,
    UiBuilder, pos2, vec2,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// The left edge of the panel: the slot column stays visible beside it.
const LEFT: f32 = 266.0;
/// The panel's bottom: the shape section stays visible under it.
const BOTTOM: f32 = backspin_ui::HEIGHT as f32;
const PAD: f32 = 8.0;
const HEADER: f32 = 24.0;
const FOOTER: f32 = 16.0;
const ROW: f32 = 20.0;
const TREE_WIDTH: f32 = 190.0;
const INDENT: f32 = 14.0;
/// The panel is two panes side by side from this width, else the tree sits above the list.
const WIDE: f32 = 480.0;

const BG: Color32 = rgb(0x2b3a45);
const PANE: Color32 = rgb(0x30414d);
const BORDER: Color32 = rgb(0x4a5c69);
const TEXT: Color32 = rgb(0xd0dfea);
const DIM: Color32 = rgb(0x8595a0);

/// The file manager's name in the menus.
const REVEAL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(windows) {
    "Show in Explorer"
} else {
    "Show in file manager"
};

/// Which presets the browser shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    GrossBeat,
    Shape,
}

/// A folder of the tree.
struct Node {
    /// Identifies the folder across rescans: its path, or its title when it is not on disk.
    key: String,
    name: String,
    /// The folder's names from its root: `User › Sub`.
    title: String,
    depth: usize,
    path: Option<PathBuf>,
    writable: bool,
    imported: bool,
    /// The shape library group this folder is, for the previous and next arrows.
    group: Option<usize>,
    children: Vec<usize>,
    presets: Vec<Preset>,
    /// How many presets this folder and its subfolders hold.
    count: usize,
}

/// One row of the preset list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// A folder's title, when the list spans several folders.
    Title(usize),
    /// Preset `1` of node `0`.
    Preset(usize, usize),
}

/// A preset file being dragged onto a folder of the tree.
struct Dragged(PathBuf);

/// A name prompt.
enum Prompt {
    /// Rename the preset file to the typed name.
    Rename(PathBuf, String),
    /// Create a subfolder with the typed name.
    NewFolder(PathBuf, String),
}

/// What a widget asked for; it runs after the widgets, which borrow the browser.
enum Act {
    Close,
    Refresh,
    Import,
    OpenFile,
    Select(usize),
    Toggle(usize),
    Load(usize, usize),
    Move(PathBuf, PathBuf),
    Copy(usize, usize, PathBuf),
    Rename(PathBuf),
    NewFolder(PathBuf),
    Forget(PathBuf),
    Reveal(PathBuf),
}

/// The browser's state.
pub struct Browser {
    pub open: Option<Kind>,
    /// The kind the folders were read for.
    kind: Kind,
    /// The folders; `None` until the next frame reads them.
    nodes: Option<Vec<Node>>,
    /// Counts the reads, so the rows are built again after one.
    scans: u64,
    pub search: String,
    /// The chosen folder's key.
    folder: Option<String>,
    collapsed: HashSet<String>,
    /// Counts the folds, so the shown tree is built again after one.
    folds: u64,
    /// The tree's nodes in display order, without the children of collapsed folders.
    shown: Vec<usize>,
    /// The scan and fold `shown` was built for.
    shown_for: Option<(u64, u64)>,
    rows: Vec<Row>,
    /// The scan, folder and search the rows were built for.
    rows_for: Option<(u64, Option<String>, String)>,
    /// The row the arrow keys move.
    cursor: Option<usize>,
    scroll_to_cursor: bool,
    focus_search: bool,
    pub error: Option<String>,
    prompt: Option<Prompt>,
    prompt_focus: bool,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            open: None,
            kind: Kind::GrossBeat,
            nodes: None,
            scans: 0,
            search: String::new(),
            folder: None,
            collapsed: HashSet::new(),
            folds: 0,
            shown: Vec::new(),
            shown_for: None,
            rows: Vec::new(),
            rows_for: None,
            cursor: None,
            scroll_to_cursor: false,
            focus_search: false,
            error: None,
            prompt: None,
            prompt_focus: false,
        }
    }
}

impl Browser {
    /// Opens the browser on `kind`.
    pub fn open(&mut self, kind: Kind) {
        if self.kind != kind {
            self.kind = kind;
            self.nodes = None;
            self.folder = None;
            self.collapsed.clear();
            self.search.clear();
        }
        self.open = Some(kind);
        self.cursor = None;
        self.scroll_to_cursor = true;
        self.focus_search = true;
        self.error = None;
    }

    /// Opens the browser on `kind`, or closes it when it shows `kind` already.
    pub fn toggle(&mut self, kind: Kind) {
        if self.open == Some(kind) {
            self.open = None;
        } else {
            self.open(kind);
        }
    }

    /// Reads the folders again on the next frame.
    pub fn refresh(&mut self) {
        self.nodes = None;
    }

    /// Opens the Gross Beat presets on the folder `dir`.
    pub fn show_folder(&mut self, dir: &Path) {
        self.open(Kind::GrossBeat);
        self.refresh();
        self.folder = Some(dir.display().to_string());
    }

    /// Records the result of a disk operation: an error shows in the footer, a success reads
    /// the folders again.
    fn done<T>(&mut self, result: std::io::Result<T>) -> Option<T> {
        match result {
            Ok(v) => {
                self.error = None;
                self.refresh();
                Some(v)
            }
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    fn nodes(&self) -> &[Node] {
        self.nodes.as_deref().unwrap_or(&[])
    }

    /// The chosen folder, or the first one.
    fn selected(&self) -> Option<usize> {
        let nodes = self.nodes();
        let chosen = self
            .folder
            .as_ref()
            .and_then(|key| nodes.iter().position(|n| &n.key == key));
        chosen.or((!nodes.is_empty()).then_some(0))
    }

    /// The nodes in tree order; with `shown`, only those whose parents are expanded.
    fn order(&self, shown: bool) -> Vec<usize> {
        fn walk(b: &Browser, n: usize, shown: bool, out: &mut Vec<usize>) {
            out.push(n);
            if !shown || !b.collapsed.contains(&b.nodes()[n].key) {
                for &c in &b.nodes()[n].children {
                    walk(b, c, shown, out);
                }
            }
        }
        let mut out = Vec::new();
        for n in 0..self.nodes().len() {
            if self.nodes()[n].depth == 0 {
                walk(self, n, shown, &mut out);
            }
        }
        out
    }

    /// Builds the shown tree and the rows when the scan, the folds, the folder or the search
    /// changed. Nothing is built on the other frames.
    fn ensure_rows(&mut self) {
        if self.shown_for != Some((self.scans, self.folds)) {
            self.shown = self.order(true);
            self.shown_for = Some((self.scans, self.folds));
        }
        if self
            .rows_for
            .as_ref()
            .is_some_and(|(scans, folder, search)| {
                *scans == self.scans && *folder == self.folder && *search == self.search
            })
        {
            return;
        }
        let search = self.search.trim().to_lowercase();
        let mut rows = Vec::new();
        let nodes = self.nodes();
        if search.is_empty() {
            if let Some(sel) = self.selected() {
                rows.extend((0..nodes[sel].presets.len()).map(|k| Row::Preset(sel, k)));
                let mut subs = Vec::new();
                for &c in &nodes[sel].children {
                    self.descendants(c, &mut subs);
                }
                for n in subs {
                    if !nodes[n].presets.is_empty() {
                        rows.push(Row::Title(n));
                        rows.extend((0..nodes[n].presets.len()).map(|k| Row::Preset(n, k)));
                    }
                }
            }
        } else {
            for n in self.order(false) {
                // "All presets" repeats every pack: skip it when there are packs.
                if self.kind == Kind::Shape && nodes[n].group == Some(0) && nodes.len() > 1 {
                    continue;
                }
                let hits: Vec<usize> = (0..nodes[n].presets.len())
                    .filter(|&k| nodes[n].presets[k].name.to_lowercase().contains(&search))
                    .collect();
                if !hits.is_empty() {
                    rows.push(Row::Title(n));
                    rows.extend(hits.into_iter().map(|k| Row::Preset(n, k)));
                }
            }
        }
        self.rows = rows;
        self.rows_for = Some((self.scans, self.folder.clone(), self.search.clone()));
        // A new list starts at its first preset, unless the current one is in it.
        self.cursor = None;
        self.scroll_to_cursor = true;
    }

    fn descendants(&self, n: usize, out: &mut Vec<usize>) {
        out.push(n);
        for &c in &self.nodes()[n].children {
            self.descendants(c, out);
        }
    }

    /// The row of `source` in the list.
    fn row_of(&self, source: Option<&Source>) -> Option<usize> {
        let source = source?;
        self.rows.iter().position(
            |r| matches!(r, Row::Preset(n, k) if &self.nodes()[*n].presets[*k].source == source),
        )
    }

    /// The next preset row from `from` in direction `step`, or the first one.
    fn step(&self, from: Option<usize>, step: i32) -> Option<usize> {
        let is_preset = |i: usize| matches!(self.rows.get(i), Some(Row::Preset(..)));
        match from {
            Some(c) => {
                let mut i = c as i32 + step;
                while i >= 0 && (i as usize) < self.rows.len() {
                    if is_preset(i as usize) {
                        return Some(i as usize);
                    }
                    i += step;
                }
                Some(c)
            }
            None => (0..self.rows.len()).find(|&i| is_preset(i)),
        }
    }

    /// The node that holds `source`.
    fn node_of(&self, source: Option<&Source>) -> Option<usize> {
        let source = source?;
        self.nodes()
            .iter()
            .position(|n| n.presets.iter().any(|p| &p.source == source))
    }
}

/// Flattens `f` and its subfolders into `nodes`; returns `f`'s index.
fn add_folder(nodes: &mut Vec<Node>, f: &Folder, depth: usize, parent_title: &str) -> usize {
    let key = f
        .path
        .as_ref()
        .map_or_else(|| f.name.clone(), |p| p.display().to_string());
    let title = if depth == 0 {
        f.name.clone()
    } else {
        format!("{parent_title} \u{203a} {}", f.name)
    };
    let i = nodes.len();
    nodes.push(Node {
        key,
        name: f.name.clone(),
        title: title.clone(),
        depth,
        path: f.path.clone(),
        writable: f.writable,
        imported: f.imported,
        group: None,
        children: Vec::new(),
        presets: f.presets.clone(),
        count: f.count(),
    });
    let children = f
        .children
        .iter()
        .map(|c| add_folder(nodes, c, depth + 1, &title))
        .collect();
    nodes[i].children = children;
    i
}

/// The shape packs as a tree: "All presets", then each category with its packs.
fn shape_nodes(lib: &[Group]) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let leaf = |name: &str, title: &str, depth, group, presets: &[Preset]| Node {
        key: title.into(),
        name: name.into(),
        title: title.into(),
        depth,
        path: None,
        writable: false,
        imported: false,
        group,
        children: Vec::new(),
        presets: presets.to_vec(),
        count: presets.len(),
    };
    for (g, grp) in lib.iter().enumerate() {
        match grp.name.split_once('@') {
            Some((category, pack)) => {
                let parent = nodes
                    .iter()
                    .position(|n| n.depth == 0 && n.group.is_none() && n.name == category)
                    .unwrap_or_else(|| {
                        nodes.push(leaf(category, category, 0, None, &[]));
                        nodes.len() - 1
                    });
                let title = format!("{category} \u{203a} {pack}");
                let i = nodes.len();
                nodes.push(leaf(pack, &title, 1, Some(g), &grp.presets));
                nodes[parent].children.push(i);
                nodes[parent].count += grp.presets.len();
            }
            None => nodes.push(leaf(&grp.name, &grp.name, 0, Some(g), &grp.presets)),
        }
    }
    nodes
}

/// Reads the folders of `kind`; `None` while the shape library still loads.
fn scan(gui: &Gui, kind: Kind) -> Option<Vec<Node>> {
    match kind {
        Kind::GrossBeat => {
            let mut nodes = Vec::new();
            for f in backspin_presets::gross_beat_folders(crate::sb::embedded()) {
                add_folder(&mut nodes, &f, 0, "");
            }
            Some(nodes)
        }
        Kind::Shape => gui.sb_library.lock().as_deref().map(shape_nodes),
    }
}

/// The source of the preset the section of `kind` has loaded, from the editor's `gb_preset` and
/// `sb_preset` (passed as fields, so the browser can be borrowed at the same time).
fn current<'a>(gb: &'a Option<Preset>, sb: &'a Option<Source>, kind: Kind) -> Option<&'a Source> {
    match kind {
        Kind::GrossBeat => gb.as_ref().map(|p| &p.source),
        Kind::Shape => sb.as_ref(),
    }
}

/// The highlight colour of `kind`.
fn accent(kind: Kind) -> Color32 {
    match kind {
        Kind::GrossBeat => SELECT_RING,
        Kind::Shape => rgb(ACCENT),
    }
}

/// Shows the browser when it is open.
pub fn show(gui: &mut Gui, ctx: &Context, setter: &ParamSetter) {
    let Some(kind) = gui.browser.open else { return };
    let had_prompt = gui.browser.prompt.is_some();
    if had_prompt {
        prompt(gui, ctx);
    }
    // The keys are read before the widgets, so the search box cannot take them first.
    let (esc, up, down, enter) = if had_prompt {
        (false, false, false, false)
    } else {
        ctx.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
            )
        })
    };
    if esc {
        gui.browser.open = None;
        return;
    }
    if gui.browser.nodes.is_none()
        && let Some(nodes) = scan(gui, kind)
    {
        let source = current(&gui.gb_preset, &gui.sb_preset, kind);
        let b = &mut gui.browser;
        b.nodes = Some(nodes);
        b.scans += 1;
        if b.folder.is_none() {
            let node = match kind {
                Kind::GrossBeat => b.node_of(source),
                Kind::Shape => b.nodes().iter().position(|n| n.group == Some(gui.sb_pack)),
            };
            b.folder = node.map(|n| b.nodes()[n].key.clone());
        }
    }
    gui.browser.ensure_rows();
    let source = current(&gui.gb_preset, &gui.sb_preset, kind);
    let mut acts = Vec::new();
    {
        let b = &mut gui.browser;
        if b.cursor.is_none() && b.scroll_to_cursor {
            b.cursor = b.row_of(source).or_else(|| b.step(None, 1));
        }
        if up || down {
            b.cursor = b.step(b.cursor, if down { 1 } else { -1 });
            b.scroll_to_cursor = true;
        }
        if enter && let Some(Row::Preset(n, k)) = b.cursor.map(|c| b.rows[c]) {
            acts.push(Act::Load(n, k));
            // Enter takes the focus from the search box; keep it there.
            b.focus_search = ctx.memory(|m| m.has_focus(Id::new("browser_search")));
        }
    }
    let screen = ctx.screen_rect();
    let rect = if screen.width() >= WIDE {
        Rect::from_min_max(
            pos2(LEFT, 0.0),
            pos2(screen.max.x, BOTTOM.min(screen.max.y)),
        )
    } else {
        screen
    };
    // No fade in: the frame under the panel would show through while it fades.
    egui::Area::new(Id::new("preset_browser"))
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            let p = ui.painter();
            p.rect_filled(rect, 0.0, BG);
            p.rect_stroke(rect, 0.0, Stroke::new(1.0f32, BORDER), StrokeKind::Inside);
            let b = &mut gui.browser;
            let header_r = Rect::from_min_size(
                rect.min + vec2(PAD, PAD),
                vec2(rect.width() - 2.0 * PAD, HEADER),
            );
            header(b, ui, header_r, kind, &mut acts);
            let body = Rect::from_min_max(
                pos2(rect.min.x + PAD, header_r.max.y + PAD),
                pos2(rect.max.x - PAD, rect.max.y - FOOTER - PAD),
            );
            let (tree_r, list_r) = if body.width() >= WIDE - 2.0 * PAD {
                (
                    body.with_max_x(body.min.x + TREE_WIDTH),
                    body.with_min_x(body.min.x + TREE_WIDTH + PAD),
                )
            } else {
                let split = body.min.y + (body.height() * 0.35).round();
                (body.with_max_y(split), body.with_min_y(split + PAD))
            };
            tree(b, ui, tree_r, kind, &mut acts);
            list(b, ui, list_r, kind, source, &mut acts);
            let footer_r = Rect::from_min_max(
                pos2(rect.min.x + PAD, rect.max.y - FOOTER - PAD / 2.0),
                pos2(rect.max.x - PAD, rect.max.y - PAD / 2.0),
            );
            footer(b, ui, footer_r, kind);
            dragged(ui);
        });
    for a in acts {
        act(gui, setter, kind, a);
    }
}

/// The title, the search box and the buttons.
fn header(b: &mut Browser, ui: &mut Ui, r: Rect, kind: Kind, acts: &mut Vec<Act>) {
    ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
        ui.horizontal_centered(|ui| {
            let title = match kind {
                Kind::GrossBeat => "Presets",
                Kind::Shape => "Shape presets",
            };
            ui.label(RichText::new(title).size(14.0).strong().color(TEXT));
            ui.add_space(4.0);
            let search = ui.add(
                TextEdit::singleline(&mut b.search)
                    .id(Id::new("browser_search"))
                    .hint_text("Search")
                    .desired_width(170.0),
            );
            if b.focus_search {
                search.request_focus();
                b.focus_search = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, Icon::Close, "Close (Esc)").clicked() {
                    acts.push(Act::Close);
                }
                if icon_button(ui, Icon::Refresh, "Read the folders again").clicked() {
                    acts.push(Act::Refresh);
                }
                if ui
                    .button("Open\u{2026}")
                    .on_hover_text("Open a preset file")
                    .clicked()
                {
                    acts.push(Act::OpenFile);
                }
                if kind == Kind::GrossBeat
                    && ui
                        .button("Import\u{2026}")
                        .on_hover_text("Add a folder of presets to the list")
                        .clicked()
                {
                    acts.push(Act::Import);
                }
            });
        });
    });
}

#[derive(Clone, Copy)]
enum Icon {
    Close,
    Refresh,
}

/// A small square button with a drawn icon.
fn icon_button(ui: &mut Ui, icon: Icon, hint: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
    let resp = resp.on_hover_text(hint);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, 3.0, rgba(0xffffff, 0x18));
    }
    let c = r.center();
    let s = Stroke::new(1.5f32, TEXT);
    match icon {
        Icon::Close => {
            p.line_segment([c + vec2(-4.0, -4.0), c + vec2(4.0, 4.0)], s);
            p.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], s);
        }
        Icon::Refresh => {
            let pts: Vec<_> = (0..=20)
                .map(|i| {
                    let a = -1.2 + i as f32 / 20.0 * 5.2;
                    c + vec2(a.cos(), a.sin()) * 5.0
                })
                .collect();
            p.add(egui::Shape::line(pts, s));
            let tip = c + vec2((-1.2f32).cos(), (-1.2f32).sin()) * 5.0;
            p.line_segment([tip, tip + vec2(-3.5, -0.5)], s);
            p.line_segment([tip, tip + vec2(-0.5, 3.5)], s);
        }
    }
    resp
}

/// The folder tree.
fn tree(b: &mut Browser, ui: &mut Ui, r: Rect, kind: Kind, acts: &mut Vec<Act>) {
    ui.painter().rect_filled(r, 3.0, PANE);
    let sel = b.selected();
    let accent = accent(kind);
    ui.scope_builder(UiBuilder::new().max_rect(r.shrink(2.0)), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt("browser_tree")
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .show_rows(ui, ROW, b.shown.len(), |ui, range| {
                for i in range {
                    let n = b.shown[i];
                    let node = &b.nodes()[n];
                    let (_, rr) = ui.allocate_space(vec2(ui.available_width(), ROW));
                    let resp =
                        ui.interact(rr, Id::new(("browser_folder", &node.key)), Sense::click());
                    let drop = node.writable && resp.dnd_hover_payload::<Dragged>().is_some();
                    let p = ui.painter().with_clip_rect(rr);
                    if Some(n) == sel {
                        p.rect_filled(rr, 2.0, rgba(0xffffff, 0x1e));
                    } else if resp.hovered() {
                        p.rect_filled(rr, 2.0, rgba(0xffffff, 0x0c));
                    }
                    if drop {
                        p.rect_stroke(rr, 2.0, Stroke::new(1.0f32, accent), StrokeKind::Inside);
                    }
                    let x = rr.min.x + 4.0 + node.depth as f32 * INDENT;
                    let cy = rr.center().y;
                    if !node.children.is_empty() {
                        let open = !b.collapsed.contains(&node.key);
                        let pts = if open {
                            vec![
                                pos2(x + 1.0, cy - 2.0),
                                pos2(x + 9.0, cy - 2.0),
                                pos2(x + 5.0, cy + 3.0),
                            ]
                        } else {
                            vec![
                                pos2(x + 2.0, cy - 4.0),
                                pos2(x + 7.0, cy),
                                pos2(x + 2.0, cy + 4.0),
                            ]
                        };
                        p.add(egui::Shape::convex_polygon(pts, DIM, Stroke::NONE));
                    }
                    let count = format!("{}", node.count);
                    let cw = p
                        .text(
                            pos2(rr.max.x - 6.0, cy),
                            Align2::RIGHT_CENTER,
                            &count,
                            font(11.0),
                            DIM,
                        )
                        .width();
                    let name_r = rr.with_min_x(x + 14.0).with_max_x(rr.max.x - 10.0 - cw);
                    p.with_clip_rect(name_r).text(
                        pos2(name_r.min.x, cy),
                        Align2::LEFT_CENTER,
                        &node.name,
                        font(13.0),
                        TEXT,
                    );
                    if resp.clicked() {
                        let on_arrow = !node.children.is_empty()
                            && resp
                                .interact_pointer_pos()
                                .is_some_and(|pp| pp.x < x + 12.0);
                        acts.push(if on_arrow {
                            Act::Toggle(n)
                        } else {
                            Act::Select(n)
                        });
                    }
                    if resp.double_clicked() && !node.children.is_empty() {
                        acts.push(Act::Toggle(n));
                    }
                    if node.writable
                        && let Some(d) = resp.dnd_release_payload::<Dragged>()
                        && let Some(dir) = &node.path
                    {
                        acts.push(Act::Move(d.0.clone(), dir.clone()));
                    }
                    if kind == Kind::GrossBeat && (node.path.is_some() || node.imported) {
                        resp.context_menu(|ui| folder_menu(ui, node, acts));
                    }
                }
            });
    });
}

/// The right-click menu of a folder.
fn folder_menu(ui: &mut Ui, node: &Node, acts: &mut Vec<Act>) {
    if node.writable
        && let Some(dir) = &node.path
        && ui.button("New folder\u{2026}").clicked()
    {
        acts.push(Act::NewFolder(dir.clone()));
        ui.close_menu();
    }
    if let Some(dir) = &node.path
        && dir.is_dir()
        && ui.button(REVEAL).clicked()
    {
        acts.push(Act::Reveal(dir.clone()));
        ui.close_menu();
    }
    if node.imported
        && let Some(dir) = &node.path
        && ui.button("Remove from list").clicked()
    {
        acts.push(Act::Forget(dir.clone()));
        ui.close_menu();
    }
}

/// The preset list of the chosen folder, or of the search.
fn list(
    b: &mut Browser,
    ui: &mut Ui,
    r: Rect,
    kind: Kind,
    source: Option<&Source>,
    acts: &mut Vec<Act>,
) {
    ui.painter().rect_filled(r, 3.0, PANE);
    if b.rows.is_empty() {
        let text = if b.nodes.is_none() {
            "Loading\u{2026}"
        } else if b.search.trim().is_empty() {
            "No presets"
        } else {
            "No match"
        };
        ui.painter()
            .text(r.center(), Align2::CENTER_CENTER, text, font(13.0), DIM);
        return;
    }
    let accent = accent(kind);
    let mut area = ScrollArea::vertical()
        .id_salt("browser_list")
        .auto_shrink([false, false])
        .drag_to_scroll(false);
    if b.scroll_to_cursor {
        let row = b.cursor.unwrap_or(0) as f32;
        area = area.vertical_scroll_offset((row * ROW - r.height() / 2.0 + ROW / 2.0).max(0.0));
        b.scroll_to_cursor = false;
    }
    let cursor = b.cursor;
    ui.scope_builder(UiBuilder::new().max_rect(r.shrink(2.0)), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        area.show_rows(ui, ROW, b.rows.len(), |ui, range| {
            for i in range {
                let (_, rr) = ui.allocate_space(vec2(ui.available_width(), ROW));
                let p = ui.painter().with_clip_rect(rr);
                let cy = rr.center().y;
                match b.rows[i] {
                    Row::Title(n) => {
                        p.text(
                            pos2(rr.min.x + 6.0, cy + 1.0),
                            Align2::LEFT_CENTER,
                            &b.nodes()[n].title,
                            font(11.0),
                            DIM,
                        );
                    }
                    Row::Preset(n, k) => {
                        let node = &b.nodes()[n];
                        let preset = &node.presets[k];
                        let file = match &preset.source {
                            Source::File(f) if kind == Kind::GrossBeat && node.writable => Some(f),
                            _ => None,
                        };
                        let sense = if file.is_some() {
                            Sense::click_and_drag()
                        } else {
                            Sense::click()
                        };
                        let id = Id::new(("browser_row", &node.key, &preset.name));
                        let resp = ui.interact(rr, id, sense);
                        if let Some(f) = file {
                            resp.dnd_set_drag_payload(Dragged(f.clone()));
                        }
                        let is_current = Some(&preset.source) == source;
                        if is_current {
                            p.rect_filled(rr, 2.0, alpha(accent, 0x58));
                        } else if resp.hovered() {
                            p.rect_filled(rr, 2.0, rgba(0xffffff, 0x0c));
                        }
                        if cursor == Some(i) {
                            p.rect_stroke(rr, 2.0, Stroke::new(1.0f32, accent), StrokeKind::Inside);
                        }
                        p.text(
                            pos2(rr.min.x + 8.0, cy),
                            Align2::LEFT_CENTER,
                            &preset.name,
                            font(13.0),
                            if is_current { Color32::WHITE } else { TEXT },
                        );
                        if resp.clicked() {
                            acts.push(Act::Load(n, k));
                        }
                        resp.context_menu(|ui| preset_menu(ui, b, kind, n, k, acts));
                    }
                }
            }
        });
    });
}

/// The right-click menu of a preset.
fn preset_menu(ui: &mut Ui, b: &Browser, kind: Kind, n: usize, k: usize, acts: &mut Vec<Act>) {
    let node = &b.nodes()[n];
    let preset = &node.presets[k];
    if ui.button("Load").clicked() {
        acts.push(Act::Load(n, k));
        ui.close_menu();
    }
    let file = match &preset.source {
        Source::File(f) => Some(f),
        Source::Embedded(_) => None,
    };
    if kind == Kind::GrossBeat {
        if let Some(f) = file
            && node.writable
        {
            if ui.button("Rename\u{2026}").clicked() {
                acts.push(Act::Rename(f.clone()));
                ui.close_menu();
            }
            ui.menu_button("Move to", |ui| {
                for m in b.order(false) {
                    let target = &b.nodes()[m];
                    if m != n
                        && target.writable
                        && let Some(dir) = &target.path
                        && ui.button(&target.title).clicked()
                    {
                        acts.push(Act::Move(f.clone(), dir.clone()));
                        ui.close_menu();
                    }
                }
            });
        }
        if let Some(user) = backspin_presets::paths::user_presets()
            && node.path.as_deref() != Some(user.as_path())
            && ui.button("Copy to User").clicked()
        {
            acts.push(Act::Copy(n, k, user));
            ui.close_menu();
        }
    }
    if let Some(f) = file
        && ui.button(REVEAL).clicked()
    {
        acts.push(Act::Reveal(f.clone()));
        ui.close_menu();
    }
}

/// The footer: the last error, or a short help text, and the count of the listed presets.
fn footer(b: &mut Browser, ui: &mut Ui, r: Rect, kind: Kind) {
    let p = ui.painter().with_clip_rect(r);
    let n = b
        .rows
        .iter()
        .filter(|row| matches!(row, Row::Preset(..)))
        .count();
    let count = if n == 1 {
        "1 preset".to_string()
    } else {
        format!("{n} presets")
    };
    let cw = p
        .text(
            pos2(r.max.x - 2.0, r.center().y),
            Align2::RIGHT_CENTER,
            count,
            font(11.0),
            DIM,
        )
        .width();
    let left = r.with_max_x(r.max.x - cw - 12.0);
    if let Some(e) = &b.error {
        let resp = ui.interact(left, Id::new("browser_error"), Sense::click());
        p.with_clip_rect(left).text(
            pos2(left.min.x + 2.0, left.center().y),
            Align2::LEFT_CENTER,
            e,
            font(11.0),
            VOLUME,
        );
        if resp.clicked() {
            b.error = None;
        }
    } else if kind == Kind::GrossBeat {
        p.with_clip_rect(left).text(
            pos2(left.min.x + 2.0, left.center().y),
            Align2::LEFT_CENTER,
            "Drag a preset onto a folder to move it. Right-click for more.",
            font(11.0),
            DIM,
        );
    }
}

/// The name of the preset being dragged, next to the pointer.
fn dragged(ui: &Ui) {
    let Some(d) = DragAndDrop::payload::<Dragged>(ui.ctx()) else {
        return;
    };
    let Some(pos) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    let name =
        d.0.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
    let p = ui
        .ctx()
        .layer_painter(LayerId::new(Order::Tooltip, Id::new("browser_drag")));
    let galley = p.layout_no_wrap(name, font(13.0), Color32::WHITE);
    let at = pos + vec2(12.0, 6.0);
    p.rect_filled(
        Rect::from_min_size(at - vec2(4.0, 2.0), galley.size() + vec2(8.0, 4.0)),
        3.0,
        rgba(0x1b2328, 0xe0),
    );
    p.galley(at, galley, Color32::WHITE);
}

/// The rename and new folder prompts.
fn prompt(gui: &mut Gui, ctx: &Context) {
    let Some(p) = gui.browser.prompt.take() else {
        return;
    };
    let (title, mut text) = match &p {
        Prompt::Rename(_, t) => ("Rename preset", t.clone()),
        Prompt::NewFolder(_, t) => ("New folder", t.clone()),
    };
    let (mut ok, mut cancel) = (false, false);
    let modal = egui::Modal::new(Id::new("browser_prompt")).show(ctx, |ui| {
        ui.set_width(240.0);
        ui.label(RichText::new(title).strong());
        let edit = ui.add(TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
        if gui.browser.prompt_focus {
            edit.request_focus();
            gui.browser.prompt_focus = false;
        }
        ui.horizontal(|ui| {
            ok = ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter));
            cancel = ui.button("Cancel").clicked();
        });
    });
    let cancel = cancel || modal.should_close();
    if ok {
        let b = &mut gui.browser;
        match p {
            Prompt::Rename(file, _) => {
                if let Some(new) = b.done(backspin_presets::rename_preset(&file, &text))
                    && gui.gb_preset.as_ref().map(|p| &p.source) == Some(&Source::File(file))
                {
                    gui.gb_preset = Some(Preset {
                        name: text.trim().to_string(),
                        source: Source::File(new),
                    });
                }
            }
            Prompt::NewFolder(dir, _) => {
                b.done(backspin_presets::create_folder(&dir, &text));
            }
        }
    } else if !cancel {
        gui.browser.prompt = Some(match p {
            Prompt::Rename(file, _) => Prompt::Rename(file, text),
            Prompt::NewFolder(dir, _) => Prompt::NewFolder(dir, text),
        });
    }
}

/// Runs what a widget asked for.
fn act(gui: &mut Gui, setter: &ParamSetter, kind: Kind, a: Act) {
    match a {
        Act::Close => gui.browser.open = None,
        Act::Refresh => gui.browser.refresh(),
        Act::Import => file_dialog::open(gui, file_dialog::Action::ImportFolder),
        Act::OpenFile => file_dialog::open(
            gui,
            match kind {
                Kind::GrossBeat => file_dialog::Action::GrossBeatPreset,
                Kind::Shape => file_dialog::Action::ShapePreset,
            },
        ),
        Act::Select(n) => {
            let b = &mut gui.browser;
            let key = b.nodes()[n].key.clone();
            if b.collapsed.remove(&key) {
                b.folds += 1;
            }
            b.folder = Some(key);
            if let Some(g) = b.nodes()[n].group {
                gui.sb_pack = g;
            }
        }
        Act::Toggle(n) => {
            let b = &mut gui.browser;
            let key = b.nodes()[n].key.clone();
            if !b.collapsed.remove(&key) {
                b.collapsed.insert(key);
            }
            b.folds += 1;
        }
        Act::Load(n, k) => {
            let b = &mut gui.browser;
            let preset = b.nodes()[n].presets[k].clone();
            let group = b.nodes()[n].group;
            b.cursor = b.rows.iter().position(|r| *r == Row::Preset(n, k));
            match kind {
                Kind::GrossBeat => {
                    load_preset(gui, setter, &preset);
                    gui.gb_preset = Some(preset);
                }
                Kind::Shape => {
                    sb_section::load_sb(gui, &preset);
                    if let Some(g) = group {
                        gui.sb_pack = g;
                    }
                }
            }
        }
        Act::Move(file, dir) => {
            if let Some(new) = gui.browser.done(backspin_presets::move_preset(&file, &dir))
                && let Some(p) = &mut gui.gb_preset
                && p.source == Source::File(file)
            {
                p.source = Source::File(new);
            }
        }
        Act::Copy(n, k, dir) => {
            let preset = gui.browser.nodes()[n].presets[k].clone();
            gui.browser
                .done(backspin_presets::copy_preset(&preset, &dir));
        }
        Act::Rename(file) => {
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            gui.browser.prompt = Some(Prompt::Rename(file, stem));
            gui.browser.prompt_focus = true;
        }
        Act::NewFolder(dir) => {
            gui.browser.prompt = Some(Prompt::NewFolder(dir, String::new()));
            gui.browser.prompt_focus = true;
        }
        Act::Forget(dir) => {
            let b = &mut gui.browser;
            if b.folder.as_deref() == Some(&dir.display().to_string()) {
                b.folder = None;
            }
            b.done(backspin_presets::forget_folder(&dir));
        }
        Act::Reveal(path) => {
            if let Err(e) = reveal(&path) {
                gui.browser.error = Some(e.to_string());
            }
        }
    }
}

/// Shows `path` in the system's file manager.
fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let child = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(windows)]
    let child = Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let child = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    child.map(drop)
}
