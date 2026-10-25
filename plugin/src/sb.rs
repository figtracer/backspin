//! The ShaperBox section of the plugin: its persisted state, the processors handed to the audio
//! thread, and the local preset library (`~/Library/Cableguys/ShaperBox3`, read at runtime).

use backspin_shaperbox::graph::Processor;
use backspin_shaperbox::state::State;
use backspin_shaperbox::{Tree, Var, read_preset, write_preset};
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

    /// Sets the master mix; `push` rebuilds the processors (at the end of a knob drag).
    pub fn set_mix(&self, v: f32, push: bool) {
        {
            let mut m = self.model.lock();
            let Some(t) = m.tree.as_mut() else { return };
            let is = |c: &Tree| matches!(c.prop("id"), Some(Var::String(x)) if x == "masterMix");
            match t.children.iter_mut().find(|c| is(c)) {
                Some(leaf) => match leaf.props.iter_mut().find(|(k, _)| k == "value") {
                    Some((_, old)) => *old = Var::Double(v as f64),
                    None => leaf.props.push(("value".into(), Var::Double(v as f64))),
                },
                None => t.children.push(Tree {
                    kind: "float".into(),
                    props: vec![
                        ("id".into(), Var::String("masterMix".into())),
                        ("value".into(), Var::Double(v as f64)),
                    ],
                    children: Vec::new(),
                }),
            }
        }
        if push {
            self.push();
        }
    }

    /// Turns shaper `k` on or off (on also makes it visible: the plugin processes visible shapers
    /// only).
    pub fn set_shaper(&self, k: usize, on: bool) {
        {
            let mut m = self.model.lock();
            let Some(t) = m.tree.as_mut() else { return };
            set_shaper_leaf(t, k, "on", Var::Bool(on));
            if on {
                set_shaper_leaf(t, k, "VISIBLE", Var::Bool(true));
            }
        }
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

/// A pack of the library (`Category@Pack` in the plugin's database).
#[derive(Clone, Debug)]
pub struct Pack {
    pub name: String,
    pub presets: Vec<Preset>,
}

impl Pack {
    /// The short name: the part after the category.
    pub fn short(&self) -> &str {
        self.name.rsplit('@').next().unwrap_or(&self.name)
    }
}

/// The factory library: "All presets" first (every preset of `presets.db`), then its packs in
/// the plugin's order. Without the database every preset file is read and listed under
/// "All presets" only.
pub fn library() -> Vec<Pack> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let root = PathBuf::from(home).join("Library/Cableguys/ShaperBox3");
    let (mut all, mut packs) = from_db(&root).unwrap_or_else(|| (scan(&root), Vec::new()));
    all.sort_by_cached_key(|p| p.name.to_lowercase());
    packs.insert(
        0,
        Pack {
            name: "All presets".into(),
            presets: all,
        },
    );
    packs
}

/// Preset hash `h` is the file `h[0]/h[1]/h.dat`, if it is present.
fn preset_path(root: &std::path::Path, hash: &str) -> Option<PathBuf> {
    let mut c = hash.chars();
    let (a, b) = (c.next()?, c.next()?);
    let path = root
        .join(a.to_string())
        .join(b.to_string())
        .join(format!("{hash}.dat"));
    path.is_file().then_some(path)
}

/// Every preset of `presets.db` and its packs.
fn from_db(root: &std::path::Path) -> Option<(Vec<Preset>, Vec<Pack>)> {
    let db = rusqlite::Connection::open_with_flags(
        root.join("presets.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let rows = |sql: &str| -> Option<Vec<(String, String, String)>> {
        let mut q = db.prepare(sql).ok()?;
        let r = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .ok()?
            .flatten()
            .collect();
        Some(r)
    };
    let all = rows("select '', hash, name from presets")?
        .into_iter()
        .filter_map(|(_, hash, name)| {
            Some(Preset {
                name,
                path: preset_path(root, &hash)?,
            })
        })
        .collect();
    let mut packs: Vec<Pack> = Vec::new();
    let positions = rows(
        "select pp.pack, p.hash, p.name from pack_positions pp join presets p \
         on p.hash = pp.preset order by pp.pack, pp.position",
    )?;
    for (pack, hash, name) in positions {
        let Some(path) = preset_path(root, &hash) else {
            continue;
        };
        let preset = Preset { name, path };
        match packs.last_mut() {
            Some(p) if p.name == pack => p.presets.push(preset),
            _ => packs.push(Pack {
                name: pack,
                presets: vec![preset],
            }),
        }
    }
    Some((all, packs))
}

/// Every plain preset file under the library folder, with its program name.
fn scan(root: &std::path::Path) -> Vec<Preset> {
    let mut out = Vec::new();
    for a in std::fs::read_dir(root).into_iter().flatten().flatten() {
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
    out
}

/// Sets the `<type id=.. value=..>` leaf `id` of the shaper `k` in a state tree, adding it after
/// the shaper's other leaves when missing.
pub fn set_shaper_leaf(tree: &mut Tree, k: usize, id: &str, v: Var) {
    let name = backspin_shaperbox::state::SHAPER_IDS[k];
    let is = |t: &Tree, s: &str| matches!(t.prop("id"), Some(Var::String(x)) if x == s);
    let Some(sh) = tree.children.iter_mut().find(|c| is(c, name)) else {
        return;
    };
    match sh.children.iter_mut().find(|c| is(c, id)) {
        Some(leaf) => match leaf.props.iter_mut().find(|(k, _)| k == "value") {
            Some((_, old)) => *old = v,
            None => leaf.props.push(("value".into(), v)),
        },
        None => sh.children.insert(
            0,
            Tree {
                kind: "bool".into(),
                props: vec![("id".into(), Var::String(id.into())), ("value".into(), v)],
                children: Vec::new(),
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local library groups the presets into the database's packs (skips without an install).
    #[test]
    fn library_reads_packs() {
        let lib = library();
        let Some(all) = lib.first() else { return };
        if all.presets.is_empty() {
            return;
        }
        assert_eq!(all.name, "All presets");
        assert!(lib.len() > 1, "no packs read from presets.db");
        assert!(all.presets.iter().any(|p| p.name == "Init"));
        eprintln!("{} presets in {} packs", all.presets.len(), lib.len() - 1);
    }
}
