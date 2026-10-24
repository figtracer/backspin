//! The Backspin time and volume mapping engine.
//!
//! Audio goes into a 2-bar ring buffer. The time lane's envelope sets how far back the read head
//! is, and the volume lane's envelope sets the gain. Each lane has 36 slots that can be selected
//! by parameter or by MIDI note. See `SPEC.md` for the full behavior.
//!
//! The engine is modular in two ways:
//!
//! - [`LaneState`] is generic slot selection (trigger sync, position sync, key-held, one-shot).
//!   A new lane needs a slot list in [`Bank`] and a state in [`Engine`].
//! - [`Stage`] lets new effects run after the built-in stages, with per-sample access to the lane
//!   values (see [`StageContext`]).

mod bank;
mod engine;
mod lane;
mod presets;

pub use bank::{Bank, ForceHold, LaneId, Slot, SlotSettings, Sync, SLOTS};
pub use engine::{
    apply_tension, ClickReduction, Engine, NoteEvent, Settings, Stage, StageContext, Status,
    Transport, GUARD, MIN_TEMPO, TIME_RANGE_BEATS,
};
pub use lane::{next_grid, Activation, LaneState, Source};
pub use presets::factory_bank;

pub use backspin_envelope as envelope;
