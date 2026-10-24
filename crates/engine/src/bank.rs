//! Slots and banks (state.md section 4).

use backspin_envelope::{Cursor, Envelope};

/// Slots per bank.
pub const SLOTS: usize = 36;

/// One slot (`TBRSlot`): a 4-beat envelope plus its switching settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// Name; empty shows as "Slot N".
    pub name: String,
    /// Trigger sync index into the sync table (+0xd0), 5 = 4 beats.
    pub trigger_sync: i32,
    /// Position sync index (+0xd4).
    pub position_sync: i32,
    /// Force hold: 0 off, 1 last, 2 next (+0xd8).
    pub force_hold: i32,
    /// "Link volume slot" (+0xdc).
    pub link: u8,
    /// "Key-held" (+0xdd).
    pub key_held: u8,
    /// "One-shot" (+0xde).
    pub one_shot: u8,
    /// The envelope.
    pub env: Envelope,
    /// Cached "empty" flag (+0xe4): every point at 1.0.
    pub empty: bool,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            name: String::new(),
            trigger_sync: 5,
            position_sync: 5,
            force_hold: 0,
            link: 0,
            key_held: 0,
            one_shot: 0,
            env: Envelope::slot(),
            empty: true,
        }
    }
}

impl Slot {
    /// The name the editor and the host show: the name, or "Slot N".
    pub fn display_name(&self, index: usize) -> String {
        if self.name.is_empty() {
            format!("Slot {}", index + 1)
        } else {
            self.name.clone()
        }
    }

    /// Recomputes the cached "empty" flag after an edit.
    pub fn update_empty(&mut self) {
        self.empty = self.env.is_empty();
    }
}

/// A bank of 36 slots (time or volume) and its playback state (`self+0x1a0 + bank * 0x208`).
#[derive(Clone, Debug)]
pub struct Bank {
    /// Active slot (playing in the engine), -1 when stopped (+0x00).
    pub active: i32,
    /// Selected slot = parameter 0/1 (+0x04).
    pub selected: i32,
    /// Slot shown in the editor (+0x08).
    pub edited: i32,
    /// Slot that waits for the trigger time (+0x0c).
    pub pending: i32,
    /// Trigger time in ticks, `i32::MAX` = none (+0x10).
    pub trigger_time: i32,
    /// MIDI note of slot 0 (+0x14).
    pub base_note: i32,
    /// The slots.
    pub slots: Vec<Slot>,
    /// Key-held stack of slot indexes (+0x150).
    pub key_stack: Vec<i32>,
    /// The slot whose envelope plays (+0x158). It survives a stop.
    pub active_slot: usize,
    /// Envelope playback state (+0x160).
    pub cursor: Cursor,
    /// Beats per sample (+0x1e0).
    pub step: f64,
    /// Attack compensation in samples (+0x1f0).
    pub attack_comp: f64,
    /// Envelope position for the editor locator, in beats (+0x188 / +0x198).
    pub position: f64,
}

impl Bank {
    /// A bank with constructor slots.
    pub fn new(base_note: i32) -> Self {
        Self {
            active: -1,
            selected: 0,
            edited: 0,
            pending: 0,
            trigger_time: i32::MAX,
            base_note,
            slots: vec![Slot::default(); SLOTS],
            key_stack: Vec::with_capacity(SLOTS + 1),
            active_slot: 0,
            cursor: Cursor::default(),
            step: 0.0,
            attack_comp: 0.0,
            position: 0.0,
        }
    }

    /// The slot whose envelope plays.
    pub fn active_slot(&self) -> &Slot {
        &self.slots[self.active_slot]
    }
}
