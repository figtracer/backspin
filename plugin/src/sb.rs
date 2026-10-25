//! The ShaperBox section of the plugin: its persisted state, the processors handed to the audio
//! thread, and the local preset library (`~/Library/Cableguys/ShaperBox3`, read at runtime).

use backspin_shaperbox::graph::Processor;
use backspin_shaperbox::state::State;
use backspin_shaperbox::{Tree, read_preset, write_preset};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The section's model: on or off, and the state tree.
#[derive(Clone, Default)]
pub struct Model {
    pub enabled: bool,
    pub tree: Option<Tree>,
}

impl Model {
    /// Persisted form: one byte for `enabled`, then the preset file bytes (empty when unset).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![self.enabled as u8];
        if let Some(t) = &self.tree {
            out.extend(write_preset(t));
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let Some((&e, rest)) = b.split_first() else {
            return Self::default();
        };
        Self {
            enabled: e != 0,
            tree: (!rest.is_empty()).then(|| read_preset(rest).ok()).flatten(),
        }
    }

    pub fn state(&self) -> State {
        self.tree.as_ref().map(State::from_tree).unwrap_or_default()
    }

    /// The preset name, if a state is loaded.
    pub fn name(&self) -> Option<String> {
        self.tree.as_ref().map(|t| State::from_tree(t).name)
    }
}

/// The shared side: the model, the sample rate the processors are built for, and the queues
/// that carry new processors to the audio thread and old ones back.
pub struct Shared {
    pub model: Mutex<Model>,
    pub sample_rate: AtomicU32,
    /// Set when the latency the host knows is out of date.
    pub latency_dirty: AtomicBool,
    to_audio: Mutex<Producer<Option<Box<Processor>>>>,
    garbage: Mutex<Consumer<Box<Processor>>>,
}

/// The audio thread's ends.
pub struct Audio {
    from_ui: Consumer<Option<Box<Processor>>>,
    garbage: Producer<Box<Processor>>,
    pub processor: Option<Box<Processor>>,
}

pub fn new() -> (Shared, Audio) {
    let (tp, tc) = RingBuffer::new(16);
    let (gp, gc) = RingBuffer::new(16);
    (
        Shared {
            model: Mutex::new(Model::default()),
            sample_rate: AtomicU32::new(44100f32.to_bits()),
            latency_dirty: AtomicBool::new(false),
            to_audio: Mutex::new(tp),
            garbage: Mutex::new(gc),
        },
        Audio {
            from_ui: tc,
            garbage: gp,
            processor: None,
        },
    )
}

impl Shared {
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate.load(Ordering::Relaxed))
    }

    /// A processor for the model, or `None` when the section is off.
    pub fn build(&self) -> Option<Box<Processor>> {
        let m = self.model.lock();
        m.enabled
            .then(|| Box::new(Processor::new(m.state(), self.sample_rate())))
    }

    /// Sends the model to the audio thread (from the editor).
    pub fn push(&self) {
        while self.garbage.lock().pop().is_ok() {}
        let p = self.build();
        let _ = self.to_audio.lock().push(p);
        self.latency_dirty.store(true, Ordering::Relaxed);
    }

    /// Loads a preset file into the model and turns the section on.
    pub fn load_file(&self, path: &std::path::Path) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let tree = read_preset(&data)?;
        {
            let mut m = self.model.lock();
            m.tree = Some(tree);
            m.enabled = true;
        }
        self.push();
        Ok(())
    }

    pub fn set_enabled(&self, on: bool) {
        self.model.lock().enabled = on;
        self.push();
    }
}

impl Audio {
    /// Takes the newest processor from the editor (at a block start).
    pub fn apply(&mut self) {
        while let Ok(p) = self.from_ui.pop() {
            if let Some(old) = std::mem::replace(&mut self.processor, p) {
                let _ = self.garbage.push(old);
            }
        }
    }
}

/// A preset of the local library.
#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub path: PathBuf,
}

/// The factory library: every plain preset file with its program name, sorted by name.
pub fn library() -> Vec<Preset> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let root = PathBuf::from(home).join("Library/Cableguys/ShaperBox3");
    let mut out = Vec::new();
    for a in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
                let path = f.path();
                if path.extension().is_some_and(|e| e == "dat")
                    && let Ok(data) = std::fs::read(&path)
                    && let Ok(t) = read_preset(&data)
                {
                    let name = State::from_tree(&t).name;
                    out.push(Preset {
                        name: if name.is_empty() {
                            "Untitled".into()
                        } else {
                            name
                        },
                        path,
                    });
                }
            }
        }
    }
    out.sort_by_cached_key(|p| p.name.to_lowercase());
    out
}
