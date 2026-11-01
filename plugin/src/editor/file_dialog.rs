//! File dialogs that do not block the editor. A modal dialog opened from an egui frame runs a
//! nested event loop inside baseview's frame callback, and the next event re-enters baseview
//! while its window state is still borrowed (`RefCell already borrowed`, an abort). So dialogs
//! are asynchronous: a frame starts one, later frames poll it, and the action runs when the
//! host's dialog returns a path.

use super::Gui;
use super::menus::{load_fnv_bytes, load_preset_bytes};
use backspin_engine::envelope::Envelope;
use nih_plug::prelude::ParamSetter;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// What to do with the path a dialog returns.
pub enum Action {
    /// Load a Gross Beat state preset (`.fst`).
    GrossBeatPreset,
    /// Load a shape preset (`.dat`).
    ShapePreset,
    /// Add a folder of Gross Beat presets to the presets list.
    ImportFolder,
    /// Load an envelope state file into the edited slot.
    LoadEnvelope,
    /// Save this envelope (the edited slot's when the dialog opened) as a state file.
    SaveEnvelope(Envelope),
    /// Analyze an audio file into the slot of `bank`/`index`.
    Analyze(usize, usize),
}

type Pick = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>>;

/// The dialog that is open, if any.
pub struct Pending {
    action: Action,
    pick: Pick,
}

/// Opens a dialog for `action` unless one is already open.
pub fn open(gui: &mut Gui, action: Action) {
    let mut slot = gui.file_dialog.lock();
    if slot.is_some() {
        return;
    }
    let d = rfd::AsyncFileDialog::new();
    let pick: Pick = match &action {
        Action::GrossBeatPreset => {
            Box::pin(d.add_filter("Gross Beat preset", &["fst"]).pick_file())
        }
        Action::ShapePreset => Box::pin(d.add_filter("Shape preset", &["dat"]).pick_file()),
        Action::ImportFolder => Box::pin(
            d.set_title("Import a Gross Beat preset folder")
                .pick_folder(),
        ),
        Action::LoadEnvelope => Box::pin(d.add_filter("Envelope state file", &["fnv"]).pick_file()),
        Action::SaveEnvelope(_) => {
            Box::pin(d.add_filter("Envelope state file", &["fnv"]).save_file())
        }
        Action::Analyze(..) => Box::pin(
            d.add_filter(
                "Audio files",
                &["wav", "aif", "aiff", "wv", "mp3", "ogg", "flac", "m4a"],
            )
            .pick_file(),
        ),
    };
    *slot = Some(Pending { action, pick });
}

/// Polls the open dialog (every frame) and runs its action when it returns a path.
pub fn poll(gui: &mut Gui, setter: &ParamSetter) {
    let done = {
        let mut slot = gui.file_dialog.lock();
        let Some(p) = slot.as_mut() else { return };
        let mut cx = Context::from_waker(Waker::noop());
        match p.pick.as_mut().poll(&mut cx) {
            Poll::Pending => return,
            Poll::Ready(file) => slot.take().map(|p| (p.action, file)),
        }
    };
    if let Some((action, Some(file))) = done {
        run(gui, setter, action, file.path());
    }
}

pub(super) fn run(gui: &mut Gui, setter: &ParamSetter, action: Action, path: &Path) {
    match action {
        Action::GrossBeatPreset => {
            if let Ok(data) = std::fs::read(path) {
                load_preset_bytes(gui, setter, &data);
                gui.gb_preset = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
        }
        Action::ShapePreset => {
            let _ = gui.shared.sb.load_file(path);
        }
        Action::ImportFolder => {
            let _ = backspin_presets::import_folder(path);
        }
        Action::LoadEnvelope => load_fnv_bytes(gui, std::fs::read(path).ok().map(Into::into)),
        Action::SaveEnvelope(env) => {
            let _ = backspin_engine::envelope::write_fnv(path, &env);
        }
        Action::Analyze(bank, index) => super::dialogs::analyze_path(gui, bank, index, path),
    }
}
