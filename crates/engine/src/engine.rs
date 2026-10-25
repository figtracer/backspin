//! The plugin object (`TTestPlug`): host calls, parameters, slot switching and MIDI.

use crate::bank::{Bank, SLOTS, Slot};
use crate::params::{NUM_PARAMS, PARAMS, smoothing_ms};
use backspin_dsp::{DcFilter, DryDelay, OnePole, Resampler, Ring, coef};
use backspin_envelope::{Cursor, curve_params, rne};

/// The latency in samples (FHD_SetLatency(15) in the constructor).
pub const LATENCY: usize = 15;

/// `ProcessParam` flags.
pub const REC_UPDATE_VALUE: i32 = 1;
pub const REC_GET_VALUE: i32 = 2;
pub const REC_UPDATE_CONTROL: i32 = 0x10;
pub const REC_FROM_MIDI: i32 = 0x20;

/// `FPD_ProcessMode` flags.
pub const PM_HQ_NON_REALTIME: i32 = 2;
pub const PM_IS_RENDERING: i32 = 16;

/// "Now" for force hold Last: the hold starts at the next block (0x80000001).
pub(crate) const HOLD_NOW: i32 = i32::MIN + 1;

/// Click reduction time constants in ms, indexed by the option (0x34caa4, 0x34ca98).
const DECLICK_MS: [f32; 3] = [25.0, 25.0, 35.0];
const SPEED_MS: [f32; 3] = [1.0, 1.0, 5.0];

/// The engine. Field comments name the original offsets in `TTestPlug`.
#[derive(Clone, Debug)]
pub struct Engine {
    /// Host sample rate (host info +8).
    pub(crate) sr: i32,
    /// Block size (+0x194).
    pub(crate) block_size: i32,
    /// Tempo in BPM (+0x190).
    pub(crate) tempo: f32,
    /// PPQ (+0x198).
    pub(crate) ppq: i32,
    /// The song position in ticks, as `FHD_GetMixingTime(GT_Ticks)` would return it.
    song_ticks: f64,
    /// Raw parameter values (+0x158).
    pub params: [i32; NUM_PARAMS],
    /// Editor view bank, 0 time, 1 volume (+0x17c).
    pub view_bank: u32,
    /// Option bytes: Remove DC (+0x180), attack compensation (+0x181), HQ (+0x182), scratch clock
    /// view (+0x183), click reduction 0..2 (+0x184).
    pub remove_dc: u8,
    pub attack_comp: u8,
    pub hq: u8,
    pub view_scratch: u8,
    pub click: u8,
    /// "Trigger slots from MIDI notes" (+0x769).
    pub midi_trigger: u8,
    /// `PM_IsRendering` (+0x186) and `PM_HQ_NonRealtime` (+0x18a).
    pub(crate) is_rendering: bool,
    pub(crate) hq_nonrealtime: bool,
    /// Transport playing (+0x187), first block after play (+0x188), hold running (+0x189).
    pub(crate) playing: bool,
    pub(crate) flush_flag: bool,
    pub(crate) holding: bool,
    /// Hold bits: bit 0 HOLD button, bits 1/2 force hold of the time/volume slot (+0x18c).
    pub(crate) hold_bits: i32,
    /// The time and volume banks.
    pub banks: [Bank; 2],
    /// Sync table in ticks (0x3b1cb0, a global in the original).
    pub(crate) sync: [i32; 6],
    /// Ring length setting `L` (+0x130).
    pub(crate) ring_frames: i32,
    pub(crate) ring: Ring,
    pub(crate) dry: DryDelay,
    /// Read head (+0x5d0) and the tempo crossfade read head (+0x5d8).
    pub(crate) r: Resampler,
    pub(crate) r2: Resampler,
    /// Two bars (+0x5c0) and samples per bar (+0x5c8).
    pub(crate) two_bars: f64,
    pub(crate) spb: f64,
    /// Kernel taps setting, 8 or 16 (+0x5e0).
    pub(crate) taps_setting: i32,
    /// Previous envelope offset (+0x5e8).
    pub(crate) prev_env: f64,
    /// Hold time in ticks (+0x5f0), loop read position (+0x5f4), saved read position (+0x5f8),
    /// length index into the sync table (+0x5fc).
    pub(crate) hold_time: i32,
    pub(crate) hold_loop: u32,
    pub(crate) hold_saved: u32,
    pub(crate) hold_index: usize,
    /// Scratch smoothers (+0x600, three stages).
    pub(crate) scratch_sm: [OnePole; 3],
    /// Last time envelope value (+0x678) and ticks (+0x67c), for the editor.
    pub last_time_value: f32,
    pub ui_ticks: i32,
    /// Scratch target (+0x680).
    pub(crate) scratch: f32,
    /// Click threshold (+0x684) and declick decay (+0x688).
    pub(crate) click_thr: f32,
    pub(crate) declick_decay: f32,
    /// Previous raw wet (+0x68c) and declick state (+0x694), per channel.
    pub(crate) prev_wet: [f32; 2],
    pub(crate) declick: [f32; 2],
    /// Last output frame (+0x69c).
    pub(crate) last_out: [f32; 2],
    /// Volume tension curve coefficients (+0x6a4).
    pub(crate) vol_curve: [f32; 3],
    /// Volume smoothers (+0x6b0, two stages).
    pub(crate) vol_sm: [OnePole; 2],
    pub(crate) dc: DcFilter,
    /// Tempo crossfade length (+0x140), pending frames (+0x144), old two bars (+0x148).
    pub(crate) xfade_len: i32,
    pub(crate) xfade_pending: i32,
    pub(crate) xfade_two_bars: f64,
    /// Held MIDI keys, `note | channel << 8` (+0x150).
    pub(crate) held_keys: Vec<i32>,
    pub(crate) work: Work,
}

/// Render scratch buffers (the arena at +0xd8).
#[derive(Clone, Debug, Default)]
pub(crate) struct Work {
    pub env: Vec<f64>,
    pub raw: Vec<f64>,
    pub off2: Vec<f64>,
    pub speed: Vec<f32>,
    pub click: Vec<f32>,
    pub tmp: Vec<f32>,
    pub vol: Vec<f32>,
    pub expf: Vec<f32>,
    pub omega: Vec<f64>,
    pub cos: Vec<f64>,
    pub wet: Vec<[f32; 2]>,
    pub old: Vec<[f32; 2]>,
}

impl Work {
    pub(crate) fn reserve(&mut self, n: usize) {
        if self.env.len() < n {
            let n = (n + 3) & !3;
            self.env.resize(n, 0.0);
            self.raw.resize(n, 0.0);
            self.off2.resize(n, 0.0);
            self.speed.resize(n, 0.0);
            self.click.resize(n, 0.0);
            self.tmp.resize(n, 0.0);
            self.vol.resize(n, 0.0);
            self.expf.resize(n, 0.0);
            self.omega.resize(n, 0.0);
            self.cos.resize(n, 0.0);
            self.wet.resize(n, [0.0; 2]);
            self.old.resize(n, [0.0; 2]);
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A fresh instance (constructor 0x68490): flat slots, default parameters, MIDI triggering
    /// off, 44.1 kHz until the host sets the sample rate.
    pub fn new() -> Self {
        let mut e = Self {
            sr: 44100,
            block_size: 0,
            tempo: 120.0,
            ppq: 96,
            song_ticks: 0.0,
            params: PARAMS.map(|p| p.default),
            view_bank: 0,
            remove_dc: 0,
            attack_comp: 0,
            hq: 1,
            view_scratch: 1,
            click: 1,
            midi_trigger: 0,
            is_rendering: false,
            hq_nonrealtime: false,
            playing: false,
            flush_flag: false,
            holding: false,
            hold_bits: 0,
            banks: [Bank::new(60), Bank::new(24)],
            sync: [0; 6],
            ring_frames: 0,
            ring: Ring::default(),
            dry: DryDelay::default(),
            r: Resampler::new(8),
            r2: Resampler::new(16),
            two_bars: 0.0,
            spb: 0.0,
            taps_setting: 8,
            prev_env: 0.0,
            hold_time: i32::MAX,
            hold_loop: 0,
            hold_saved: 0,
            hold_index: 5,
            scratch_sm: [OnePole::default(); 3],
            last_time_value: 0.0,
            ui_ticks: 0,
            scratch: 0.0,
            click_thr: 0.0,
            declick_decay: 0.0,
            prev_wet: [0.0; 2],
            declick: [0.0; 2],
            last_out: [0.0; 2],
            vol_curve: [0.0; 3],
            vol_sm: [OnePole::default(); 2],
            dc: DcFilter::default(),
            xfade_len: 0,
            xfade_pending: 0,
            xfade_two_bars: 0.0,
            held_keys: Vec::with_capacity(256),
            work: Work::default(),
        };
        for i in 0..NUM_PARAMS {
            let v = e.process_param(i, 0, REC_GET_VALUE);
            e.process_param(i, v, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
        }
        e.update_sync();
        e
    }

    /// Sets the song position that the next calls read, in ticks.
    pub fn set_song_ticks(&mut self, ticks: f64) {
        self.song_ticks = ticks;
    }

    /// The song position in whole ticks (0x695c0).
    pub(crate) fn now(&self) -> i32 {
        rne(self.song_ticks) as i32
    }

    /// The sample rate.
    pub fn sample_rate(&self) -> i32 {
        self.sr
    }

    /// The PPQ.
    pub fn ppq(&self) -> i32 {
        self.ppq
    }

    /// The tempo.
    pub fn tempo(&self) -> f32 {
        self.tempo
    }

    /// True while the transport plays.
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// True while the hold replays a bar.
    pub fn holding(&self) -> bool {
        self.holding
    }

    /// FPD_SetSampleRate (Dispatcher case 4).
    pub fn set_sample_rate(&mut self, sr: i32) {
        self.sr = sr;
        self.update_smoothing();
        let sr = sr as f64;
        for s in &mut self.scratch_sm {
            s.set_times(sr * 0.3 * 0.5773502691896258, sr * 0.2 * 0.5773502691896258);
        }
        self.update_click();
        self.dc.set_sample_rate(self.sr);
    }

    /// FPD_SetBlockSize (Dispatcher case 3, 0x69da0): sizes the ring for 72 s plus the block.
    pub fn set_block_size(&mut self, block: i32) {
        self.block_size = block;
        let f = (self.sr as i64 * 240) as f32 / 10.0;
        self.ring_frames = rne(f as f64 * 2.0 + f as f64) as i32 + 16;
        self.ring
            .resize(block.max(0) as usize, self.ring_frames as usize);
        self.r.reserve(block.max(0) as usize);
        self.r2.reserve(block.max(0) as usize);
        self.work.reserve(block.max(0) as usize);
        self.tempo_changed();
    }

    /// FPD_SetTimeSig (Dispatcher case 14): takes the PPQ.
    pub fn set_time_sig(&mut self, ppq: i32) {
        self.ppq = ppq;
        self.update_sync();
        self.tempo_changed();
    }

    /// FPE_Tempo (ProcessEvent 0).
    pub fn set_tempo(&mut self, tempo: f32) {
        self.tempo = tempo;
        self.tempo_changed();
    }

    /// FPD_ProcessMode (Dispatcher case 1).
    pub fn set_process_mode(&mut self, flags: i32) {
        self.is_rendering = flags & PM_IS_RENDERING != 0;
        self.hq_nonrealtime = flags & PM_HQ_NON_REALTIME != 0;
        self.update_taps();
    }

    /// FPD_Flush (Dispatcher case 2, 0x6b920).
    pub fn flush(&mut self) {
        self.dc.reset();
        self.ring.zero();
        self.dry.reset();
        self.flush_flag = true;
        self.prev_env = 0.0;
        self.r.speed.state = 1.0;
        self.prev_wet = [0.0; 2];
        self.declick = [0.0; 2];
        self.last_out = [0.0; 2];
        self.xfade_pending = 0;
    }

    /// FPD_SetPlaying (Dispatcher case 12, 0x6a050).
    pub fn set_playing(&mut self, playing: bool) {
        if self.playing == playing {
            return;
        }
        self.playing = playing;
        if !playing {
            self.ring.zero();
            self.xfade_pending = 0;
            self.hold_bits &= 1;
            self.hold_time = i32::MAX;
            for b in &mut self.banks {
                b.active = -1;
            }
            self.ui_ticks = 0;
            self.last_time_value = 0.0;
        } else {
            self.flush();
            self.flush_flag = true;
            self.reactivate();
        }
    }

    /// FPD_SongPosChanged (Dispatcher case 13).
    pub fn song_pos_changed(&mut self) {
        if self.playing {
            self.reactivate();
        }
    }

    /// The "Remove DC offset", "Smoothing attack compensation", "HQ resampling" and click
    /// reduction menu items (TSynthResModule.CenterMenuClick 0x6d7d0): set the option bytes, then
    /// recompute the taps setting and the smoothing, clear the declick state and recompute the
    /// click constants.
    pub fn set_options(&mut self, remove_dc: u8, attack_comp: u8, hq: u8, click: u8) {
        self.remove_dc = remove_dc;
        self.attack_comp = attack_comp;
        self.hq = hq;
        self.update_taps();
        self.update_smoothing();
        self.click = click;
        self.prev_wet = [0.0; 2];
        self.declick = [0.0; 2];
        self.update_click();
    }

    /// Replaces a slot (the editor edits the slot object in place in the original) and returns
    /// the old one in `slot`.
    pub fn swap_slot(&mut self, bank: usize, index: usize, slot: &mut Slot) {
        std::mem::swap(&mut self.banks[bank].slots[index], slot);
    }

    /// Moves a slot from `from` to `to` by successive swaps of neighbours (0x6c3e0). The active,
    /// selected, edited and pending indexes follow the slots they point at.
    pub fn move_slot(&mut self, bank: usize, from: usize, to: usize) {
        let b = &mut self.banks[bank];
        let mut i = from;
        while i != to {
            let j = if to > i { i + 1 } else { i - 1 };
            b.slots.swap(i, j);
            for v in [&mut b.active, &mut b.selected, &mut b.edited, &mut b.pending] {
                if *v == i as i32 {
                    *v = j as i32;
                } else if *v == j as i32 {
                    *v = i as i32;
                }
            }
            if b.active_slot == i {
                b.active_slot = j;
            } else if b.active_slot == j {
                b.active_slot = i;
            }
            i = j;
        }
    }

    /// ProcessParam (0x68f70). Returns the value.
    pub fn process_param(&mut self, index: usize, value: i32, flags: i32) -> i32 {
        if index >= NUM_PARAMS {
            return value;
        }
        let mut value = value;
        if flags & REC_FROM_MIDI != 0 {
            let p = PARAMS[index];
            value = rne(value as f64 * 9.313225746154785e-10 * (p.max as i64 - p.min as i64) as f64)
                as i32
                + p.min;
        }
        if flags & REC_UPDATE_VALUE == 0 {
            if flags & REC_GET_VALUE != 0 {
                value = self.params[index];
            }
            return value;
        }
        self.params[index] = value;
        match index {
            0 | 1 => self.select_slot(index, value),
            4 | 5 => self.update_smoothing(),
            6 => {
                let e = ((value as f32 * 0.00390625f32).abs() as f64 * 6.90875477931522).exp();
                let s = value.signum() as f32;
                curve_params((e - 1.0) as f32 * s, &mut self.vol_curve);
            }
            7 => {
                if (0 < self.hold_bits) != (0 < value) {
                    self.hold_bits = value | (self.hold_bits & !1);
                    if self.hold_time == i32::MAX {
                        self.hold_time = self.trigger_time(self.hold_index);
                    }
                }
            }
            8 => self.scratch = value as f32 * (1.0 / 65536.0) - 1.0,
            _ => {}
        }
        value
    }

    /// FPD_MIDIIn (Dispatcher case 24) followed by the editor handler 0x62190, which the original
    /// runs on its GUI thread before the next block. `msg` is `status | data1 << 8 | data2 << 16`.
    /// Returns true if the message was consumed.
    pub fn midi_in(&mut self, msg: u32) -> bool {
        let status = msg & 0xf0;
        if self.midi_trigger == 0 || (status != 0x80 && status != 0x90) {
            return false;
        }
        let note = ((msg >> 8) & 0xff) as i32;
        let key = note | ((msg & 0xf) as i32) << 8;
        let on = status == 0x90 && (msg >> 16) & 0xff != 0;
        if on {
            if !self.held_keys.contains(&key) {
                self.held_keys.push(key);
            }
        } else {
            match self.held_keys.iter().position(|&k| k == key) {
                Some(i) => {
                    self.held_keys.remove(i);
                }
                None => return true,
            }
        }
        let Some((bank, slot)) = self.note_slot(note) else {
            return true;
        };
        let key_held = self.banks[bank].slots[slot as usize].key_held != 0;
        if on {
            if key_held {
                let b = &mut self.banks[bank];
                if b.key_stack.is_empty() {
                    b.key_stack.push(b.selected);
                }
                if !b.key_stack.contains(&slot) {
                    b.key_stack.push(slot);
                }
            }
            self.process_param(bank, slot, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
        } else if key_held {
            let b = &mut self.banks[bank];
            if let Some(i) = b.key_stack.iter().position(|&s| s == slot) {
                b.key_stack.remove(i);
            }
            if let Some(&top) = b.key_stack.last() {
                let one = b.key_stack.len() == 1;
                self.process_param(bank, top, REC_UPDATE_VALUE | REC_UPDATE_CONTROL);
                if one {
                    self.banks[bank].key_stack.clear();
                }
            }
        }
        true
    }

    /// The bank and slot of a MIDI note (0x6d110): 60..95 time, 24..59 volume.
    fn note_slot(&self, note: i32) -> Option<(usize, i32)> {
        let mut found = None;
        for (i, b) in self.banks.iter().enumerate() {
            if b.base_note <= note && note <= b.base_note + 35 {
                found = Some((i, note - b.base_note));
            }
        }
        found
    }

    /// Selects a slot through parameter 0/1 (0x6c060). The same value again does nothing.
    fn select_slot(&mut self, bank: usize, slot: i32) {
        if self.banks[bank].selected != slot {
            self.banks[bank].selected = slot;
            if self.playing {
                self.schedule(bank, slot);
            }
        }
    }

    /// Schedules a slot switch at its trigger time (0x6c660).
    fn schedule(&mut self, bank: usize, slot: i32) {
        if bank == 1 && self.banks[0].active_slot().link != 0 {
            return;
        }
        let sync = self.banks[bank].slots[slot.clamp(0, SLOTS as i32 - 1) as usize].trigger_sync;
        let t = self.trigger_time(sync as usize);
        self.banks[bank].trigger_time = t;
        if t == i32::MAX {
            self.activate(bank, slot, true);
        } else {
            self.banks[bank].pending = self.banks[bank].selected;
        }
    }

    /// The next grid tick for a sync index (0x6c5f0), `i32::MAX` for "Off".
    pub(crate) fn trigger_time(&self, sync: usize) -> i32 {
        if sync == 0 {
            return i32::MAX;
        }
        let period = self.sync_ticks(sync);
        if period == 0 {
            return 0;
        }
        let q = ((period as i64 + self.now() as i64 - 1) / period as i64) as i32;
        period.wrapping_mul(q)
    }

    /// A sync table entry; indexes outside the table read 0.
    pub(crate) fn sync_ticks(&self, i: usize) -> i32 {
        self.sync.get(i).copied().unwrap_or(0)
    }

    /// Activates a slot (0x6c710): link, force hold, position sync seek, one-shot and the cascade
    /// to the volume bank.
    pub(crate) fn activate(&mut self, bank: usize, slot: i32, cascade: bool) {
        let old_link = self.banks[bank].active_slot().link;
        let mut slot = slot;
        if bank == 1 && self.banks[0].active_slot().link != 0 {
            slot = self.banks[0].active;
            self.banks[1].trigger_time = i32::MAX;
        }
        let b = &mut self.banks[bank];
        b.active = slot;
        b.active_slot = slot.clamp(0, SLOTS as i32 - 1) as usize;
        let s = &b.slots[b.active_slot];
        b.cursor = Cursor::new(&s.env);
        let (force_hold, position_sync, one_shot, link) =
            (s.force_hold, s.position_sync, s.one_shot, s.link);
        let bit = 2 << bank;
        if force_hold < 1 {
            let old = self.hold_bits;
            self.hold_bits &= !bit;
            if 0 < old && self.hold_bits == 0 {
                self.hold_time = HOLD_NOW;
            }
        } else {
            self.hold_bits |= bit;
            if self.hold_time == i32::MAX {
                self.hold_time = if force_hold == 1 {
                    HOLD_NOW
                } else {
                    self.sync_ticks(self.hold_index).wrapping_add(self.now())
                };
            }
        }
        let mut pos = 0.0;
        if position_sync >= 1 {
            let period = self.sync_ticks(position_sync as u32 as usize);
            let mut now = self.now() as f64;
            if 0 < self.hold_bits && self.hold_time != i32::MAX {
                now = if self.hold_time == HOLD_NOW {
                    0.0
                } else {
                    (now + ((self.ppq as i64) << 8) as f64) - self.hold_time as f64
                };
            }
            pos = if now <= 0.0 {
                0.0
            } else {
                (now - (period as i64 * (now / period as f64) as i64) as f64) / self.ppq as f64
            };
        }
        let now = self.now();
        let one_shot_time = self.sync[5].wrapping_add(now);
        let b = &mut self.banks[bank];
        let env = &b.slots[b.active_slot].env;
        b.cursor.advance(env, pos + b.attack_comp * b.step);
        if one_shot != 0 {
            b.pending = 0;
            b.trigger_time = one_shot_time;
        }
        if cascade && (old_link != 0 || link != 0) {
            let sel = self.banks[1].selected;
            self.activate(1, sel, false);
        }
    }

    /// Re-activates both banks at their selected slots (0x6c9d0), on play and song position
    /// changes.
    pub fn reactivate(&mut self) {
        if self.holding {
            self.r.pos = self.hold_saved;
        }
        self.holding = false;
        self.hold_time = i32::MAX;
        if 0 < self.hold_bits {
            self.hold_time = self.sync_ticks(self.hold_index).wrapping_add(self.now());
        }
        for b in &mut self.banks {
            b.trigger_time = i32::MAX;
        }
        for bank in 0..2 {
            let sel = self.banks[bank].selected;
            self.activate(bank, sel, true);
        }
    }

    /// Rebuilds the sync table (0x6d080).
    fn update_sync(&mut self) {
        let p = self.ppq;
        self.sync = [0, p / 4, p / 2, p, p << 1, p << 2];
    }

    /// Recomputes the volume smoothers and the attack compensation (0x6a330).
    pub(crate) fn update_smoothing(&mut self) {
        let sr = self.sr as f64;
        let att = smoothing_ms(self.params[4]) * 0.001 * sr;
        for b in &mut self.banks {
            b.attack_comp = att * self.attack_comp as f64;
        }
        let rel = smoothing_ms(self.params[5]) * 0.001 * sr;
        for s in &mut self.vol_sm {
            s.set_times(att * 0.7071067811865475, rel * 2.0 * 0.7071067811865475);
        }
    }

    /// Recomputes the click reduction constants (0x69c90).
    pub(crate) fn update_click(&mut self) {
        let sr = self.sr as f64;
        let i = (self.click as usize).min(2);
        self.click_thr = (self.sr as f32 as f64 * 0.002) as f32;
        self.declick_decay =
            (backspin_dsp::LN_001 / (DECLICK_MS[i] as f64 * 0.001 * sr)).exp() as f32;
        let c = coef(SPEED_MS[i] as f64 * 0.001 * sr);
        self.r.speed.rise = c;
        self.r.speed.fall = c;
        let n = rne(sr * 0.001);
        self.xfade_len = if n < 2 { 1 } else { n as i32 };
    }

    /// Picks the kernel taps setting (0x6a400).
    pub(crate) fn update_taps(&mut self) {
        self.taps_setting = if self.hq == 0 && !self.hq_nonrealtime {
            8
        } else {
            16
        };
        self.r.set_ratio(1.0);
    }

    /// Recomputes the tempo-dependent constants and the read position (0x69e40).
    pub(crate) fn tempo_changed(&mut self) {
        let old = self.two_bars;
        self.spb = ((self.sr as i64 * 240) as f32 / self.tempo) as f64;
        self.two_bars = self.spb * 2.0;
        for b in &mut self.banks {
            b.step = 4.0 / self.spb;
        }
        let len = self.ring.len() as i64;
        self.r.len = len as u32;
        let write = self.ring.write_pos() as i64;
        let mut base = rne(self.two_bars + 0.5) + 15;
        if base < 16 {
            base = 15;
        }
        if len - 15 < base {
            base = len - 15;
        }
        let x = len * 2 + (write - base);
        let pos = if len == 0 {
            0
        } else {
            (x as i32)
                .wrapping_sub((x / len) as i32 * len as i32)
                .wrapping_add(len as i32)
        };
        if self.playing && 0.0 < old && 8.0 < (self.two_bars - old).abs() {
            self.r2.len = len as u32;
            self.r2.pos = self.r.len + self.r.pos % self.r.len;
            self.xfade_two_bars = old;
            self.xfade_pending = self.xfade_len;
        }
        self.r.pos = pos as u32;
        self.hold_saved = self.r.pos;
    }
}
