//! State shared between the editor, the host and the audio thread.
//!
//! The editor and the host state work on a model [`Engine`] that never renders. Every change to
//! the model goes to the audio engine as a [`Command`] through a wait-free queue; the audio thread
//! swaps the new data in and sends the old data back to be dropped outside the audio thread. The
//! audio thread publishes what the editor shows through [`Status`] atomics.

use backspin_engine::{Engine, NUM_PARAMS, Slot};
use parking_lot::Mutex;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

/// Queue sizes. Commands come from user actions, so these are never close to full in practice.
const COMMANDS: usize = 512;

/// A change from the editor to the audio engine.
pub enum Command {
    /// Replace one slot.
    Slot {
        bank: usize,
        index: usize,
        slot: Box<Slot>,
    },
    /// The CenterMenuClick options: Remove DC, attack compensation, HQ, click reduction.
    Options {
        remove_dc: u8,
        attack_comp: u8,
        hq: u8,
        click: u8,
    },
    /// "Trigger slots from MIDI notes".
    MidiTrigger(u8),
    /// A whole state (preset load, paste): parameters, options and slots.
    State(Box<Engine>),
}

/// Data the audio thread hands back for dropping.
pub enum Garbage {
    Slot(Box<Slot>),
    State(Box<Engine>),
}

/// What the editor shows of the playing engine.
#[derive(Default)]
pub struct Status {
    pub params: [AtomicI32; NUM_PARAMS],
    pub active: [AtomicI32; 2],
    pub selected: [AtomicI32; 2],
    /// Envelope positions in beats (f64 bits).
    pub position: [AtomicU64; 2],
    /// Last time envelope value (f32 bits).
    pub time_value: AtomicU32,
    pub ticks: AtomicI32,
    pub playing: AtomicBool,
    pub holding: AtomicBool,
    /// Set when the audio thread changed a parameter (MIDI slot selection), so the editor can tell
    /// the host.
    pub params_changed: AtomicBool,
}

impl Status {
    /// Publishes the engine state after a block.
    pub fn publish(&self, e: &Engine) {
        let mut changed = false;
        for (a, &v) in self.params.iter().zip(&e.params) {
            changed |= a.swap(v, Ordering::Relaxed) != v;
        }
        if changed {
            self.params_changed.store(true, Ordering::Relaxed);
        }
        for b in 0..2 {
            self.active[b].store(e.banks[b].active, Ordering::Relaxed);
            self.selected[b].store(e.banks[b].selected, Ordering::Relaxed);
            self.position[b].store(e.banks[b].position.to_bits(), Ordering::Relaxed);
        }
        self.time_value
            .store(e.last_time_value.to_bits(), Ordering::Relaxed);
        self.ticks.store(e.ui_ticks, Ordering::Relaxed);
        self.playing.store(e.playing(), Ordering::Relaxed);
        self.holding.store(e.holding(), Ordering::Relaxed);
    }

    /// The position of a bank's envelope in beats.
    pub fn position(&self, bank: usize) -> f64 {
        f64::from_bits(self.position[bank].load(Ordering::Relaxed))
    }
}

/// The shared part of the plugin.
pub struct Shared {
    /// The model engine: the state the host saves and the editor edits.
    pub model: Mutex<Engine>,
    commands: Mutex<Producer<Command>>,
    garbage: Mutex<Consumer<Garbage>>,
    pub status: Status,
}

/// The audio thread's ends of the queues.
pub struct AudioQueues {
    pub commands: Consumer<Command>,
    pub garbage: Producer<Garbage>,
}

impl Shared {
    /// Creates the shared state with a fresh model, and the audio thread's queue ends.
    pub fn new() -> (Self, AudioQueues) {
        let (cp, cc) = RingBuffer::new(COMMANDS);
        let (gp, gc) = RingBuffer::new(COMMANDS);
        let shared = Self {
            model: Mutex::new(Engine::new()),
            commands: Mutex::new(cp),
            garbage: Mutex::new(gc),
            status: Status::default(),
        };
        shared.status.publish(&shared.model.lock());
        (
            shared,
            AudioQueues {
                commands: cc,
                garbage: gp,
            },
        )
    }

    /// Sends a command to the audio thread. If the queue is full the change waits for the next
    /// state load; this does not happen with user-paced edits.
    pub fn send(&self, cmd: Command) {
        self.collect_garbage();
        let _ = self.commands.lock().push(cmd);
    }

    /// Drops the data the audio thread handed back.
    pub fn collect_garbage(&self) {
        let mut g = self.garbage.lock();
        while g.pop().is_ok() {}
    }

    /// Sends the model's slot to the audio thread.
    pub fn send_slot(&self, model: &Engine, bank: usize, index: usize) {
        let slot = Box::new(model.banks[bank].slots[index].clone());
        self.send(Command::Slot { bank, index, slot });
    }

    /// Sends the whole model to the audio thread.
    pub fn send_state(&self, model: &Engine) {
        self.send(Command::State(Box::new(model.clone())));
    }

    /// The saved state of the model, with the parameters the audio engine plays.
    pub fn save_state(&self) -> Vec<u8> {
        let mut model = self.model.lock();
        for (p, a) in model.params.iter_mut().zip(&self.status.params) {
            *p = a.load(Ordering::Relaxed);
        }
        model.save_state()
    }
}

impl AudioQueues {
    /// Applies the pending editor commands to the audio engine (at a block start).
    pub fn apply(&mut self, engine: &mut Engine) {
        while let Ok(cmd) = self.commands.pop() {
            let old = match cmd {
                Command::Slot {
                    bank,
                    index,
                    mut slot,
                } => {
                    engine.swap_slot(bank, index, &mut slot);
                    Some(Garbage::Slot(slot))
                }
                Command::Options {
                    remove_dc,
                    attack_comp,
                    hq,
                    click,
                } => {
                    engine.set_options(remove_dc, attack_comp, hq, click);
                    None
                }
                Command::MidiTrigger(on) => {
                    engine.midi_trigger = on;
                    None
                }
                Command::State(mut state) => {
                    engine.swap_state(&mut state);
                    Some(Garbage::State(state))
                }
            };
            if let Some(g) = old {
                // A full garbage queue would drop here; it holds as many entries as the commands.
                let _ = self.garbage.push(g);
            }
        }
    }
}
