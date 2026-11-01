//! File dialogs that do not block the editor. Opening a dialog from an egui frame re-enters
//! baseview while its window state is still borrowed (`RefCell already borrowed`, an abort): a
//! modal dialog runs a nested event loop, and a sheet takes key focus from the editor window,
//! which baseview reports at once. So a helper thread opens the dialog (on macOS rfd then shows
//! the sheet from the main queue, after the frame returns) and waits for it; later frames poll
//! for the path, and the action runs on the GUI thread.

use super::Gui;
use super::menus::{load_fnv_bytes, load_preset_bytes};
use backspin_engine::envelope::Envelope;
use nih_plug::prelude::ParamSetter;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

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

/// The dialog that is open, if any.
pub struct Pending {
    action: Action,
    /// The helper thread sends the picked path (`None` when cancelled).
    picked: Receiver<Option<PathBuf>>,
}

/// Wakes the helper thread that waits for the dialog.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `f` on the current thread until it completes.
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

/// Opens a dialog for `action` unless one is already open.
pub fn open(gui: &mut Gui, action: Action) {
    let mut slot = gui.file_dialog.lock();
    if slot.is_some() {
        return;
    }
    let d = rfd::AsyncFileDialog::new();
    let (d, kind) = match &action {
        Action::GrossBeatPreset => (d.add_filter("Gross Beat preset", &["fst"]), Kind::Open),
        Action::ShapePreset => (d.add_filter("Shape preset", &["dat"]), Kind::Open),
        Action::ImportFolder => (
            d.set_title("Import a Gross Beat preset folder"),
            Kind::Folder,
        ),
        Action::LoadEnvelope => (d.add_filter("Envelope state file", &["fnv"]), Kind::Open),
        Action::SaveEnvelope(_) => (d.add_filter("Envelope state file", &["fnv"]), Kind::Save),
        Action::Analyze(..) => (
            d.add_filter(
                "Audio files",
                &["wav", "aif", "aiff", "wv", "mp3", "ogg", "flac", "m4a"],
            ),
            Kind::Open,
        ),
    };
    let (tx, picked) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("backspin-file-dialog".into())
        .spawn(move || {
            let file = block_on(async {
                match kind {
                    Kind::Open => d.pick_file().await,
                    Kind::Folder => d.pick_folder().await,
                    Kind::Save => d.save_file().await,
                }
            });
            let _ = tx.send(file.map(|f| f.path().to_path_buf()));
        });
    if spawned.is_ok() {
        *slot = Some(Pending { action, picked });
    }
}

/// The kind of dialog an action opens.
enum Kind {
    Open,
    Folder,
    Save,
}

/// Polls the open dialog (every frame) and runs its action when it returns a path.
pub fn poll(gui: &mut Gui, setter: &ParamSetter) {
    let done = {
        let mut slot = gui.file_dialog.lock();
        let Some(p) = slot.as_ref() else { return };
        match p.picked.try_recv() {
            Ok(path) => slot.take().map(|p| (p.action, path)),
            Err(mpsc::TryRecvError::Empty) => return,
            // The helper thread died without an answer.
            Err(mpsc::TryRecvError::Disconnected) => slot.take().map(|p| (p.action, None)),
        }
    };
    if let Some((action, Some(path))) = done {
        run(gui, setter, action, &path);
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
