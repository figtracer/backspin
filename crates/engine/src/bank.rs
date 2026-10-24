//! Slot banks: the per-lane envelopes and their settings.

use backspin_envelope::Envelope;
use serde::{Deserialize, Serialize};

/// Number of slots per lane.
pub const SLOTS: usize = 36;

/// A grid size used for trigger and position sync.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sync {
    /// No sync.
    #[default]
    Off,
    /// 1/4 beat.
    Quarter,
    /// 1/2 beat.
    Half,
    /// 1 beat.
    Beat,
    /// 2 beats.
    TwoBeats,
    /// 4 beats.
    FourBeats,
}

impl Sync {
    /// All values, in menu order.
    pub const ALL: [Self; 6] =
        [Self::Off, Self::Quarter, Self::Half, Self::Beat, Self::TwoBeats, Self::FourBeats];

    /// Grid size in beats, or `None` for [`Sync::Off`].
    pub fn beats(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Quarter => Some(0.25),
            Self::Half => Some(0.5),
            Self::Beat => Some(1.0),
            Self::TwoBeats => Some(2.0),
            Self::FourBeats => Some(4.0),
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Quarter => "1/4 beat",
            Self::Half => "1/2 beat",
            Self::Beat => "1 beat",
            Self::TwoBeats => "2 beats",
            Self::FourBeats => "4 beats",
        }
    }
}

/// Per-slot hold mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ForceHold {
    /// No hold.
    #[default]
    Off,
    /// Loop the last bar at once.
    Last,
    /// Record one more bar, then loop it.
    Next,
}

impl ForceHold {
    /// All values, in menu order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Last, Self::Next];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Last => "Last",
            Self::Next => "Next",
        }
    }
}

/// Settings stored with each slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotSettings {
    /// When a selected slot starts.
    pub trigger_sync: Sync,
    /// Where in the envelope a slot starts.
    pub position_sync: Sync,
    /// The slot plays only while its MIDI key is held.
    pub key_held: bool,
    /// The slot plays its envelope once, then the lane returns to slot 1.
    pub one_shot: bool,
    /// Time lane only: selecting this slot also selects the volume slot with the same number.
    pub link: bool,
    /// Hold mode while this slot is active.
    pub force_hold: ForceHold,
}

impl Default for SlotSettings {
    fn default() -> Self {
        Self {
            trigger_sync: Sync::Off,
            position_sync: Sync::FourBeats,
            key_held: false,
            one_shot: false,
            link: false,
            force_hold: ForceHold::Off,
        }
    }
}

/// One slot: a named envelope with settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// Display name.
    pub name: String,
    /// The envelope.
    pub envelope: Envelope,
    /// Trigger and hold settings.
    #[serde(default)]
    pub settings: SlotSettings,
}

impl Slot {
    /// Creates a slot with default settings.
    pub fn new(name: impl Into<String>, envelope: Envelope) -> Self {
        Self { name: name.into(), envelope, settings: SlotSettings::default() }
    }
}

/// Identifies a lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LaneId {
    /// Playback position.
    Time,
    /// Gain.
    Volume,
}

impl LaneId {
    /// All lanes.
    pub const ALL: [Self; 2] = [Self::Time, Self::Volume];

    /// The first MIDI note that selects slot 1 of this lane.
    pub fn base_note(self) -> u8 {
        match self {
            Self::Time => 60,
            Self::Volume => 24,
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Time => "Time",
            Self::Volume => "Volume",
        }
    }

    /// The default value of a new envelope in this lane.
    pub fn default_level(self) -> f64 {
        1.0
    }
}

/// All slots of all lanes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bank {
    /// Time lane slots.
    pub time: Vec<Slot>,
    /// Volume lane slots.
    pub volume: Vec<Slot>,
}

impl Default for Bank {
    fn default() -> Self {
        crate::presets::factory_bank()
    }
}

impl Bank {
    /// Returns the slots of a lane.
    pub fn lane(&self, lane: LaneId) -> &[Slot] {
        match lane {
            LaneId::Time => &self.time,
            LaneId::Volume => &self.volume,
        }
    }

    /// Returns the slots of a lane, mutably.
    pub fn lane_mut(&mut self, lane: LaneId) -> &mut Vec<Slot> {
        match lane {
            LaneId::Time => &mut self.time,
            LaneId::Volume => &mut self.volume,
        }
    }

    /// Returns a slot, clamping the index.
    pub fn slot(&self, lane: LaneId, index: usize) -> &Slot {
        let slots = self.lane(lane);
        &slots[index.min(slots.len() - 1)]
    }

    /// Makes sure every lane has exactly [`SLOTS`] slots and valid envelopes.
    pub fn sanitize(&mut self) {
        for lane in LaneId::ALL {
            let slots = self.lane_mut(lane);
            slots.truncate(SLOTS);
            while slots.len() < SLOTS {
                let n = slots.len() + 1;
                slots.push(Slot::new(format!("Slot {n}"), Envelope::flat(lane.default_level())));
            }
            for slot in slots.iter_mut() {
                slot.envelope.normalize();
            }
        }
    }

    /// Swaps two slots of a lane.
    pub fn swap(&mut self, lane: LaneId, a: usize, b: usize) {
        let slots = self.lane_mut(lane);
        if a < slots.len() && b < slots.len() {
            slots.swap(a, b);
        }
    }
}
