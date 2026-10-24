//! The saved state (SaveRestoreState 0x67fa0, state.md section 3) and `.fst` presets.

use crate::bank::SLOTS;
use crate::engine::{Engine, REC_GET_VALUE, REC_UPDATE_CONTROL, REC_UPDATE_VALUE};
use crate::params::NUM_PARAMS;
use backspin_envelope::Reader;
use std::fmt;

/// The state version word the original writes.
pub const STATE_VERSION: u32 = 0x0009_0003;

/// The FL event that holds the plugin state (FLP_PluginParams).
const FLP_PLUGIN_PARAMS: u8 = 213;

/// An error from [`fst_state`].
#[derive(Debug, PartialEq, Eq)]
pub enum StateError {
    /// The file is not an FL `.fst`/`.flp` file.
    NotFl,
    /// The event stream ends in the middle of an event.
    Truncated,
    /// The file has no plugin state event.
    NoState,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFl => write!(f, "not an FL file (no FLhd/FLdt)"),
            Self::Truncated => write!(f, "truncated FL event"),
            Self::NoState => write!(f, "no plugin state event (213)"),
        }
    }
}

impl std::error::Error for StateError {}

/// Returns the plugin state of an FL `.fst` or `.flp` file: the payload of the last event 213.
pub fn fst_state(data: &[u8]) -> Result<&[u8], StateError> {
    if !data.starts_with(b"FLhd") || data.len() < 8 {
        return Err(StateError::NotFl);
    }
    let hlen = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let mut p = 8 + hlen;
    if data.get(p..p + 4) != Some(b"FLdt") {
        return Err(StateError::NotFl);
    }
    p += 8;
    let mut state = None;
    while p < data.len() {
        let id = data[p];
        p += 1;
        let len = match id {
            0..=63 => 1,
            64..=127 => 2,
            128..=191 => 4,
            _ => {
                let (mut len, mut shift) = (0usize, 0);
                loop {
                    let b = *data.get(p).ok_or(StateError::Truncated)?;
                    p += 1;
                    len |= ((b & 0x7f) as usize) << shift;
                    shift += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                len
            }
        };
        let body = data.get(p..p + len).ok_or(StateError::Truncated)?;
        if id == FLP_PLUGIN_PARAMS {
            state = Some(body);
        }
        p += len;
    }
    state.ok_or(StateError::NoState)
}

impl Engine {
    /// Loads a state blob (SaveRestoreState with `save = false`). A version word of `0x90004` or
    /// more makes the whole state ignored. Returns the number of bytes consumed.
    pub fn load_state(&mut self, data: &[u8]) -> usize {
        let mut r = Reader::new(data);
        let w = r.i32(0);
        if w >= 0x90004 {
            return r.pos();
        }
        let v = if w > 0xffff { w >> 16 } else { w };
        self.params[8] = 0x10000;
        self.view_scratch = 1;
        self.click = 1;
        let nparams = if v < 2 { 8 } else { 9 };
        for p in &mut self.params[..nparams] {
            *p = r.i32(*p);
        }
        let mut nopt = if v < 3 { 8 } else { 9 };
        if v < 5 {
            nopt -= 1;
        }
        let mut opt = [0u8; 9];
        opt[..4].copy_from_slice(&self.view_bank.to_le_bytes());
        opt[4..].copy_from_slice(&[
            self.remove_dc,
            self.attack_comp,
            self.hq,
            self.view_scratch,
            self.click,
        ]);
        r.read(&mut opt[..nopt]);
        self.view_bank = u32::from_le_bytes(opt[..4].try_into().unwrap());
        [
            self.remove_dc,
            self.attack_comp,
            self.hq,
            self.view_scratch,
            self.click,
        ] = [opt[4], opt[5], opt[6], opt[7], opt[8]];
        for bank in &mut self.banks {
            for (i, slot) in bank.slots.iter_mut().enumerate() {
                slot.name = read_name(&mut r);
                if v < 4 && slot.name == format!("Slot {}", i + 1) {
                    slot.name.clear();
                }
                slot.trigger_sync = r.i32(slot.trigger_sync);
                slot.position_sync = r.i32(slot.position_sync);
                slot.force_hold = r.i32(slot.force_hold);
                slot.link = r.u8(slot.link);
                slot.key_held = r.u8(slot.key_held);
                slot.one_shot = r.u8(slot.one_shot);
                slot.env.load(&mut r);
                slot.update_empty();
            }
        }
        self.midi_trigger = 1;
        if v > 8 {
            self.midi_trigger = (r.u8(0) != 0) as u8;
        }
        for i in 0..NUM_PARAMS {
            let value = self.process_param(i, 0, REC_GET_VALUE);
            self.process_param(i, value, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
        }
        for b in &mut self.banks {
            b.edited = b.selected;
        }
        self.update_taps();
        self.update_click();
        if self.playing {
            self.reactivate();
        }
        r.pos()
    }

    /// Saves the state blob (SaveRestoreState with `save = true`), version `0x90003`.
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 * 1024);
        out.extend_from_slice(&STATE_VERSION.to_le_bytes());
        for p in &self.params {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out.extend_from_slice(&self.view_bank.to_le_bytes());
        out.extend_from_slice(&[
            self.remove_dc,
            self.attack_comp,
            self.hq,
            self.view_scratch,
            self.click,
        ]);
        for bank in &self.banks {
            for slot in &bank.slots[..SLOTS] {
                write_name(&mut out, &slot.name);
                out.extend_from_slice(&slot.trigger_sync.to_le_bytes());
                out.extend_from_slice(&slot.position_sync.to_le_bytes());
                out.extend_from_slice(&slot.force_hold.to_le_bytes());
                out.extend_from_slice(&[slot.link, slot.key_held, slot.one_shot]);
                slot.env.save(&mut out);
            }
        }
        out.push(self.midi_trigger);
        out
    }
}

/// Reads a slot name (0x70e80): a u8 length (0xff: a u32 length follows), the bytes, trailing
/// bytes below 0x20 stripped, decoded as UTF-8.
fn read_name(r: &mut Reader<'_>) -> String {
    let n = r.u8(0);
    let n = if n == 0xff {
        r.u32(0) as usize
    } else {
        n as usize
    };
    let mut bytes = vec![0u8; n];
    r.read(&mut bytes);
    while bytes.last().is_some_and(|&b| b < 0x20) {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Writes a slot name (0x71020) as UTF-8.
fn write_name(out: &mut Vec<u8>, name: &str) {
    let b = name.as_bytes();
    if b.len() < 0xff {
        out.push(b.len() as u8);
    } else {
        out.push(0xff);
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    }
    out.extend_from_slice(b);
}
