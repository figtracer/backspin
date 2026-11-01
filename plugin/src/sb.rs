//! The ShaperBox section of the plugin: its persisted state, the processors handed to the audio
//! thread, and the shape preset library.

use backspin_shaperbox::graph::Processor;
use backspin_shaperbox::state::State;
use backspin_shaperbox::wave::Point;
use backspin_shaperbox::{Tree, Var, read_preset, write_preset};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

/// The section's model: on or off, and the state tree.
#[derive(Clone, Default)]
pub struct Model {
    pub enabled: bool,
    pub tree: Option<Tree>,
    /// Changes whenever `enabled` or `tree` changes (unique in the process), so the editor can
    /// keep what it derives from the tree until then.
    pub revision: u64,
}

/// A revision number no model has had yet.
pub fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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
            revision: next_revision(),
        }
    }

    pub fn state(&self) -> State {
        self.tree.as_ref().map(State::from_tree).unwrap_or_default()
    }
}

/// A processor on its way to the audio thread (`None` turns the section off), in a box that
/// comes back through the garbage queue with the old processor inside.
type Message = Box<Option<Box<Processor>>>;

/// The message the audio thread has not taken yet. A push replaces a pending message, so rapid
/// edits coalesce and the newest state always wins: nothing is lost to a full queue, and the
/// audio thread takes at most one message per block and frees nothing.
struct Mailbox(AtomicPtr<Option<Box<Processor>>>);

impl Mailbox {
    /// Leaves `p` for the audio thread and drops the pending message it did not take.
    fn put(&self, p: Option<Box<Processor>>) {
        let old = self.0.swap(Box::into_raw(Box::new(p)), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: a non-null pointer came from `Box::into_raw` in `put`, and the swap took
            // it out, so this is the only `Box::from_raw` of it.
            drop(unsafe { Box::from_raw(old) });
        }
    }

    /// Takes the pending message.
    fn take(&self) -> Option<Message> {
        let p = self.0.swap(null_mut(), Ordering::AcqRel);
        // SAFETY: as in `put`.
        (!p.is_null()).then(|| unsafe { Box::from_raw(p) })
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        self.take();
    }
}

/// The shared side: the model, the sample rate the processors are built for, the mailbox that
/// carries new processors to the audio thread and the queue that brings old ones back.
pub struct Shared {
    pub model: Mutex<Model>,
    pub sample_rate: AtomicU32,
    /// Set when the latency the host knows may be out of date.
    pub latency_dirty: AtomicBool,
    mailbox: Arc<Mailbox>,
    garbage: Mutex<Consumer<Message>>,
}

/// The audio thread's ends.
pub struct Audio {
    mailbox: Arc<Mailbox>,
    garbage: Producer<Message>,
    pub processor: Option<Box<Processor>>,
}

/// The garbage queue's length. It never holds more than two messages: the editor drains it
/// before each push, and each push gives the audio thread at most one message.
const GARBAGE: usize = 16;

pub fn new() -> (Shared, Audio) {
    let mailbox = Arc::new(Mailbox(AtomicPtr::new(null_mut())));
    let (gp, gc) = RingBuffer::new(GARBAGE);
    (
        Shared {
            model: Mutex::new(Model::default()),
            sample_rate: AtomicU32::new(44100f32.to_bits()),
            latency_dirty: AtomicBool::new(false),
            mailbox: mailbox.clone(),
            garbage: Mutex::new(gc),
        },
        Audio {
            mailbox,
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
        self.processor_for(&self.model.lock())
    }

    fn processor_for(&self, m: &Model) -> Option<Box<Processor>> {
        m.enabled
            .then(|| Box::new(Processor::new(m.state(), self.sample_rate())))
    }

    /// Sends the model to the audio thread (from the editor, or from the background thread
    /// when the output guard asks for a rebuild). Drops the processors the audio thread handed
    /// back.
    pub fn push(&self) {
        while self.garbage.lock().pop().is_ok() {}
        // The model stays locked until the message is in the mailbox, so two pushers cannot
        // leave a processor of an older model behind one of a newer model.
        let m = self.model.lock();
        self.mailbox.put(self.processor_for(&m));
        drop(m);
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
            m.revision = next_revision();
            m.tree = Some(tree);
            m.enabled = true;
        }
        self.push();
        Ok(())
    }

    pub fn set_enabled(&self, on: bool) {
        {
            let mut m = self.model.lock();
            m.enabled = on;
            m.revision = next_revision();
        }
        self.push();
    }

    /// Sets the master mix; `push` rebuilds the processors (at the end of a knob drag).
    pub fn set_mix(&self, v: f32, push: bool) {
        {
            let mut m = self.model.lock();
            m.revision = next_revision();
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
            m.revision = next_revision();
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
            m.revision = next_revision();
            let Some(band) = m.tree.as_mut().and_then(|t| band_tree(t, k, b)) else {
                return;
            };
            let Some(lfo) = first_lfo(band) else { return };
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

    /// Replaces the points of band `b`'s first LFO wave of shaper `k`; `push` rebuilds the
    /// processors (at the end of a drag).
    pub fn set_lfo_points(&self, k: usize, b: usize, points: &[Point], push: bool) {
        {
            let mut m = self.model.lock();
            m.revision = next_revision();
            let Some(list) = m
                .tree
                .as_mut()
                .and_then(|t| band_tree(t, k, b))
                .and_then(first_lfo)
                .and_then(|lfo| lfo.children.iter_mut().find(|c| has_id(c, "wave")))
                .and_then(|wave| wave.children.iter_mut().find(|c| has_id(c, "points")))
            else {
                return;
            };
            // New nodes copy an existing point, keeping the preset's node and leaf kinds.
            let template = list.children.first().cloned().unwrap_or_else(|| Tree {
                kind: "Point".into(),
                props: Vec::new(),
                children: Vec::new(),
            });
            list.children = points
                .iter()
                .map(|p| {
                    let mut node = template.clone();
                    set_leaf(&mut node, "int", "x", Var::Int(p.x));
                    set_leaf(&mut node, "float", "y", Var::Double(p.y as f64));
                    set_leaf(&mut node, "int", "factor", Var::Int(p.factor));
                    node
                })
                .collect();
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
            m.revision = next_revision();
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
    /// Takes the newest processor from the editor (at a block start). Returns true when it took
    /// one. The old processor leaves in the message's box, so nothing is freed here.
    pub fn apply(&mut self) -> bool {
        let Some(mut msg) = self.mailbox.take() else {
            return false;
        };
        *msg = std::mem::replace(&mut self.processor, msg.take());
        if let Err(PushError::Full(msg)) = self.garbage.push(msg) {
            // Cannot happen (see `GARBAGE`); a leak is the fallback, never a free here.
            std::mem::forget(msg);
        }
        true
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

/// The first LFO of a band tree.
fn first_lfo(band: &mut Tree) -> Option<&mut Tree> {
    band.children
        .iter_mut()
        .find(|c| c.kind.starts_with("Section"))?
        .children
        .iter_mut()
        .find(|c| has_id(c, "lfo"))
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

    /// Pushes the audio thread does not take coalesce into the newest one, the audio thread
    /// takes one message per block and sends the old processor back, and the garbage queue
    /// never fills: nothing is lost and nothing is freed on the audio side.
    #[test]
    fn mailbox_keeps_the_newest_processor() {
        let (shared, mut audio) = new();
        // An empty state tree, so the mix setter has a tree to edit.
        shared.model.lock().tree = Some(Tree {
            kind: "ShaperBox".into(),
            props: Vec::new(),
            children: Vec::new(),
        });
        let mut rng = 11u32;
        let mut next = |m: u32| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (rng >> 8) % m
        };
        for round in 0..200 {
            // A burst of edits while the audio thread sleeps: on, off and mix changes.
            let mut want = None;
            for _ in 0..1 + next(40) {
                let (on, mix) = (next(4) != 0, next(100) as f32 / 100.0);
                shared.set_mix(mix, false);
                shared.set_enabled(on);
                want = Some((on, mix));
            }
            let (on, mix) = want.unwrap();
            assert!(audio.apply(), "round {round}: nothing to take");
            assert!(!audio.apply(), "round {round}: a second message");
            assert_eq!(audio.processor.is_some(), on, "round {round}");
            if let Some(p) = &audio.processor {
                assert_eq!(p.state().master_mix, mix, "round {round}");
            }
            assert!(
                audio.garbage.slots() >= GARBAGE - 1,
                "round {round}: {} messages in the garbage",
                GARBAGE - audio.garbage.slots()
            );
        }
        assert!(!audio.apply());
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

    /// Throughput of every library shape preset: `cargo test --release -p backspin
    /// shape_throughput -- --ignored --nocapture` prints the average and the slowest presets as
    /// a share of real time (4 s of noise at 48 kHz in 512-frame blocks, 120 BPM, playing).
    #[test]
    #[ignore = "benchmark; run on demand"]
    fn shape_throughput() {
        use backspin_shaperbox::graph::Transport;
        let lib = library();
        let Some(all) = lib.first() else { return };
        // BACKSPIN_BENCH_PRESET runs one preset for a minute (for a profiler).
        let only = std::env::var("BACKSPIN_BENCH_PRESET").ok();
        let secs = if only.is_some() { 60.0 } else { 4.0f32 };
        let (sr, block) = (48_000.0f32, 512usize);
        let frames = (sr * secs) as usize;
        let mut seed = 1u32;
        let noise: Vec<[f32; 2]> = (0..frames)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let v = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                [v, -v]
            })
            .collect();
        let mut rows = Vec::new();
        for p in &all.presets {
            if only.as_ref().is_some_and(|n| *n != p.name) {
                continue;
            }
            let Some(tree) = p.bytes().and_then(|b| read_preset(&b).ok()) else {
                continue;
            };
            let mut proc = Processor::new(State::from_tree(&tree), sr);
            let mut buf = noise.clone();
            let t0 = std::time::Instant::now();
            let mut pos = 0.0f64;
            for chunk in buf.chunks_mut(block) {
                let t = Transport {
                    ppq: pos,
                    bpm: 120.0,
                    playing: true,
                };
                proc.process(chunk, t);
                pos += chunk.len() as f64 / sr as f64 * 2.0;
            }
            rows.push((t0.elapsed().as_secs_f64() / secs as f64, p.name.clone()));
        }
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let avg = rows.iter().map(|r| r.0).sum::<f64>() / rows.len().max(1) as f64;
        eprintln!(
            "{} presets, average {:.2}% of real time",
            rows.len(),
            avg * 100.0
        );
        for (share, name) in rows.iter().take(12) {
            eprintln!("{:6.2}%  {name}", share * 100.0);
        }
    }

    /// The point setter edits the wave the processors are built from, and the audio follows.
    #[test]
    fn point_edits_reach_the_state() {
        let lib = library();
        let Some(all) = lib.first() else { return };
        let (shared, _audio) = new();
        let found = all.presets.iter().take(200).find_map(|p| {
            let tree = read_preset(&p.bytes()?).ok()?;
            let s = State::from_tree(&tree);
            let k = (0..16).find(|&k| {
                s.shapers[k].visible
                    && s.shapers[k].bands[1]
                        .sections
                        .first()
                        .is_some_and(|sec| sec.lfo.points.len() >= 2)
            })?;
            Some((
                tree,
                k,
                s.shapers[k].bands[1].sections[0].lfo.points.clone(),
            ))
        });
        let Some((tree, k, mut points)) = found else {
            return;
        };
        shared.model.lock().tree = Some(tree);
        points[0].y = -0.5;
        points[1].factor = if points[1].factor == 1 { 3 } else { 1 };
        points.push(Point {
            x: 8191,
            y: 0.25,
            factor: 3,
        });
        points.retain(|p| p.x <= 8191);
        points.sort_by_key(|p| p.x);
        points.dedup_by_key(|p| p.x);
        shared.set_lfo_points(k, 1, &points, true);
        let s = shared.model.lock().state();
        assert_eq!(s.shapers[k].bands[1].sections[0].lfo.points, points);
        // The edited tree still writes and reads back.
        let bytes = shared.model.lock().to_bytes();
        let back = Model::from_bytes(&bytes).state();
        assert_eq!(back.shapers[k].bands[1].sections[0].lfo.points, points);
    }
}
