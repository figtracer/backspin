//! Slot selection state for one lane.

use crate::bank::SlotSettings;

const EPS: f64 = 1e-9;

/// Why a slot was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// From the slot parameter or the editor.
    Param,
    /// From a MIDI note.
    Note(u8),
    /// Return after a key-held release or a one-shot.
    Return,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    slot: usize,
    at: f64,
    source: Source,
}

/// A slot change that took effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Activation {
    /// The new slot.
    pub slot: usize,
    /// What caused it.
    pub source: Source,
}

/// Slot selection and envelope phase for one lane.
#[derive(Clone, Debug)]
pub struct LaneState {
    current: usize,
    previous: usize,
    origin: f64,
    pending: Option<Pending>,
    held_note: Option<u8>,
    one_shot_end: Option<f64>,
}

impl Default for LaneState {
    fn default() -> Self {
        Self {
            current: 0,
            previous: 0,
            origin: 0.0,
            pending: None,
            held_note: None,
            one_shot_end: None,
        }
    }
}

impl LaneState {
    /// The active slot.
    pub fn current(&self) -> usize {
        self.current
    }

    /// The slot waiting for its trigger sync, if any.
    pub fn pending(&self) -> Option<usize> {
        self.pending.map(|p| p.slot)
    }

    /// Returns the envelope position for `beat`, for an envelope `length` beats long.
    pub fn position(&self, beat: f64, length: f64) -> f64 {
        (beat - self.origin).rem_euclid(length)
    }

    /// Resets to slot `slot`, locked to the song position.
    pub fn reset(&mut self, slot: usize) {
        *self = Self { current: slot, previous: slot, ..Self::default() };
    }

    /// Queues a slot change. `settings` are the new slot's settings. With `immediate`, trigger
    /// sync is ignored (used when the host is stopped).
    pub fn select(
        &mut self,
        slot: usize,
        beat: f64,
        settings: &SlotSettings,
        source: Source,
        immediate: bool,
    ) {
        let at = match settings.trigger_sync.beats() {
            Some(grid) if !immediate => next_grid(beat, grid),
            _ => beat,
        };
        self.pending = Some(Pending { slot, at, source });
    }

    /// Handles a MIDI note release. Returns true if the lane should go back to its previous slot.
    pub fn note_off(&mut self, note: u8, settings_of_current: &SlotSettings) -> Option<usize> {
        if self.held_note == Some(note) {
            self.held_note = None;
            if settings_of_current.key_held {
                return Some(self.previous);
            }
        }
        if let Some(p) = self.pending
            && p.source == Source::Note(note)
            && settings_of_current.key_held
        {
            self.pending = None;
        }
        None
    }

    /// Advances to `beat`. `settings` returns the settings of a slot. Returns the activation, if
    /// one happened at this beat.
    pub fn tick(
        &mut self,
        beat: f64,
        length: f64,
        settings: impl Fn(usize) -> SlotSettings,
    ) -> Option<Activation> {
        if let Some(end) = self.one_shot_end
            && beat + EPS >= end
        {
            self.one_shot_end = None;
            let first = settings(0);
            self.select(0, beat, &first, Source::Return, true);
        }
        let p = self.pending?;
        if beat + EPS < p.at {
            return None;
        }
        self.pending = None;
        let s = settings(p.slot);
        if p.source != Source::Return {
            self.previous = self.current;
        }
        self.current = p.slot;
        self.origin = match s.position_sync.beats() {
            None => p.at,
            Some(grid) => p.at - p.at.rem_euclid(grid),
        };
        self.held_note = match p.source {
            Source::Note(n) => Some(n),
            _ => None,
        };
        self.one_shot_end = if s.one_shot && p.slot != 0 { Some(self.origin + length) } else { None };
        Some(Activation { slot: p.slot, source: p.source })
    }

    /// Handles a host position jump backwards.
    pub fn rewind(&mut self, beat: f64) {
        if self.origin > beat + EPS {
            self.origin = 0.0;
        }
        if let Some(end) = self.one_shot_end
            && end - beat > 64.0
        {
            self.one_shot_end = None;
        }
        if let Some(p) = &mut self.pending
            && p.at > beat + 64.0
        {
            p.at = beat;
        }
    }
}

/// Returns the first grid line at or after `beat`.
pub fn next_grid(beat: f64, grid: f64) -> f64 {
    let n = (beat / grid - EPS).ceil();
    (n * grid).max(beat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Sync;

    fn s(trigger: Sync, position: Sync) -> SlotSettings {
        SlotSettings { trigger_sync: trigger, position_sync: position, ..SlotSettings::default() }
    }

    #[test]
    fn grid() {
        assert_eq!(next_grid(0.1, 1.0), 1.0);
        assert_eq!(next_grid(1.0, 1.0), 1.0);
        assert_eq!(next_grid(3.9, 4.0), 4.0);
    }

    #[test]
    fn trigger_sync_waits_for_grid() {
        let mut lane = LaneState::default();
        let set = s(Sync::Beat, Sync::Off);
        lane.select(3, 0.3, &set, Source::Param, false);
        assert!(lane.tick(0.9, 4.0, |_| set).is_none());
        let act = lane.tick(1.0, 4.0, |_| set).unwrap();
        assert_eq!(act.slot, 3);
        // Position sync off: the envelope starts at the switch.
        assert!((lane.position(1.5, 4.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn position_sync_locks_to_song() {
        let mut lane = LaneState::default();
        let set = s(Sync::Off, Sync::FourBeats);
        lane.select(2, 5.5, &set, Source::Param, false);
        lane.tick(5.5, 4.0, |_| set);
        assert!((lane.position(5.5, 4.0) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn key_held_returns_to_previous() {
        let mut lane = LaneState::default();
        let held = SlotSettings { key_held: true, ..s(Sync::Off, Sync::Off) };
        lane.select(5, 1.0, &held, Source::Note(65), false);
        lane.tick(1.0, 4.0, |_| held);
        assert_eq!(lane.current(), 5);
        assert_eq!(lane.note_off(65, &held), Some(0));
    }

    #[test]
    fn one_shot_returns_to_first_slot() {
        let mut lane = LaneState::default();
        let once = SlotSettings { one_shot: true, ..s(Sync::Off, Sync::Off) };
        lane.select(7, 2.0, &once, Source::Note(67), false);
        lane.tick(2.0, 4.0, |_| once);
        assert_eq!(lane.current(), 7);
        assert!(lane.tick(5.9, 4.0, |_| once).is_none());
        let act = lane.tick(6.0, 4.0, |_| once).unwrap();
        assert_eq!(act.slot, 0);
    }
}
