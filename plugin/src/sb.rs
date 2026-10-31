//! The ShaperBox section of the plugin: its persisted state, the processors handed to the audio
//! thread, and the shape preset library.

use backspin_shaperbox::graph::Processor;
use backspin_shaperbox::state::State;
use backspin_shaperbox::{Tree, Var, read_preset, write_preset};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};
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
        self.load(&std::fs::read(path).map_err(|e| e.to_string())?)
    }

    /// Loads preset bytes into the model and turns the section on.
    pub fn load(&self, data: &[u8]) -> Result<(), String> {
        let tree = read_preset(data)?;
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

    /// Sets band `b`'s mix of shaper `k`; `push` rebuilds the processors (at the end of a drag).
    pub fn set_band_mix(&self, k: usize, b: usize, v: f32, push: bool) {
        {
            let mut m = self.model.lock();
            let Some(band) = m.tree.as_mut().and_then(|t| band_tree(t, k, b)) else {
                return;
            };
            set_leaf(band, "float", "mix", Var::Double(v as f64));
        }
        if push {
            self.push();
        }
    }

    /// Steps the length of band `b`'s first LFO of shaper `k` through the sync divisions.
    pub fn step_lfo_length(&self, k: usize, b: usize, step: i32) {
        {
            let mut m = self.model.lock();
            let Some(band) = m.tree.as_mut().and_then(|t| band_tree(t, k, b)) else {
                return;
            };
            let Some(lfo) = band
                .children
                .iter_mut()
                .find(|c| c.kind.starts_with("Section"))
                .and_then(|sec| sec.children.iter_mut().find(|c| has_id(c, "lfo")))
            else {
                return;
            };
            let now = lfo
                .children
                .iter()
                .find(|c| has_id(c, "length"))
                .and_then(|c| match c.prop("value") {
                    Some(Var::Int(v)) => Some(*v),
                    _ => None,
                })
                .unwrap_or(backspin_shaperbox::state::Lfo::default().length);
            let max = backspin_shaperbox::wave::LENGTHS.len() as i32 - 1;
            set_leaf(lfo, "int", "length", Var::Int((now + step).clamp(0, max)));
        }
        self.push();
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

/// The presets embedded in release builds (empty in development builds).
static PACK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/presets.pack"));

/// The embedded preset pack, if this build has one.
pub fn embedded() -> Option<&'static backspin_presets::Embedded> {
    static DECODED: std::sync::OnceLock<Option<backspin_presets::Embedded>> =
        std::sync::OnceLock::new();
    DECODED
        .get_or_init(|| backspin_presets::Embedded::decode(PACK))
        .as_ref()
}

/// The shape preset library: the local ShaperBox 3 library, else the embedded one.
pub fn library() -> Vec<backspin_presets::Group> {
    backspin_presets::shape_library(embedded())
}

fn has_id(t: &Tree, id: &str) -> bool {
    matches!(t.prop("id"), Some(Var::String(x)) if x == id)
}

/// Sets the `<kind id=.. value=..>` leaf `id` among `t`'s children, adding it when missing.
fn set_leaf(t: &mut Tree, kind: &str, id: &str, v: Var) {
    match t.children.iter_mut().find(|c| has_id(c, id)) {
        Some(leaf) => match leaf.props.iter_mut().find(|(k, _)| k == "value") {
            Some((_, old)) => *old = v,
            None => leaf.props.push(("value".into(), v)),
        },
        None => t.children.push(Tree {
            kind: kind.into(),
            props: vec![("id".into(), Var::String(id.into())), ("value".into(), v)],
            children: Vec::new(),
        }),
    }
}

/// Band `b` of shaper `k` in a state tree.
fn band_tree(tree: &mut Tree, k: usize, b: usize) -> Option<&mut Tree> {
    let name = backspin_shaperbox::state::SHAPER_IDS[k];
    let sh = tree.children.iter_mut().find(|c| has_id(c, name))?;
    sh.children
        .iter_mut()
        .find(|c| has_id(c, "bands"))?
        .children
        .get_mut(b)
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

    /// The embedded pack (in builds with one) holds readable presets and valid groups.
    #[test]
    fn embedded_pack_reads() {
        let Some(p) = embedded() else { return };
        assert!(!p.gross_beat.is_empty() && !p.maps.is_empty());
        for (name, data) in &p.shapes {
            assert!(read_preset(data).is_ok(), "{name}");
        }
        for (name, items) in &p.groups {
            assert!(
                items.iter().all(|&i| (i as usize) < p.shapes.len()),
                "{name}"
            );
        }
        eprintln!("{} shape presets embedded", p.shapes.len());
    }

    /// The band mix and LFO length setters edit the state the processors are built from.
    #[test]
    fn band_edits_reach_the_state() {
        let lib = library();
        let Some(all) = lib.first() else { return };
        let (shared, _audio) = new();
        // A preset with a visible shaper whose middle band has an LFO section.
        let found = all.presets.iter().take(200).find_map(|p| {
            let tree = read_preset(&p.bytes()?).ok()?;
            let s = State::from_tree(&tree);
            let k = (0..16)
                .find(|&k| s.shapers[k].visible && !s.shapers[k].bands[1].sections.is_empty())?;
            Some((tree, k, s.shapers[k].bands[1].sections[0].lfo.length))
        });
        let Some((tree, k, length)) = found else {
            return;
        };
        shared.model.lock().tree = Some(tree);
        shared.set_band_mix(k, 1, 0.25, false);
        shared.step_lfo_length(k, 1, if length > 0 { -1 } else { 1 });
        let s = shared.model.lock().state();
        assert_eq!(s.shapers[k].bands[1].mix, 0.25);
        assert_eq!(
            s.shapers[k].bands[1].sections[0].lfo.length,
            if length > 0 { length - 1 } else { 1 }
        );
    }
}
