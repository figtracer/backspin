//! The audio engine.

use crate::{
    bank::{Bank, ForceHold, LaneId, SlotSettings},
    lane::{LaneState, Source},
};
use backspin_dsp::{
    one_pole_coeff, AsymSmoother, DcBlocker, Frame, Inertia, Interpolator, Ring, SINC_HALF_TAPS,
};
use backspin_envelope::{Cursor, DEFAULT_LENGTH};
use serde::{Deserialize, Serialize};

/// Fixed latency in frames. The read head never gets closer to the write head than this, so the
/// interpolators always have future frames.
pub const GUARD: usize = SINC_HALF_TAPS;

/// The time lane's full range, in beats.
pub const TIME_RANGE_BEATS: f64 = 8.0;

/// Lowest tempo the ring buffer is sized for.
pub const MIN_TEMPO: f64 = 20.0;

const DEFAULT_TEMPO: f64 = 120.0;
const JUMP_THRESHOLD: f64 = 48.0;
const SCRATCH_INERTIA_MS: f32 = 40.0;
const MIN_GAIN_SMOOTH_MS: f32 = 0.5;

/// Click reduction mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClickReduction {
    /// No crossfade on jumps.
    Off,
    /// Short crossfade, for drums.
    #[default]
    Low,
    /// Long crossfade, for melodic material.
    High,
}

impl ClickReduction {
    /// Crossfade length in milliseconds.
    pub fn crossfade_ms(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Low => 2.0,
            Self::High => 10.0,
        }
    }
}

/// Global controls. The plugin fills this from its parameters every block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Selected time slot (0-based).
    pub time_slot: usize,
    /// Selected volume slot (0-based).
    pub volume_slot: usize,
    /// Time mix, 0..=1.
    pub time_mix: f32,
    /// Volume mix, 0..=1.
    pub volume_mix: f32,
    /// Volume smoothing attack, ms.
    pub attack_ms: f32,
    /// Volume smoothing release, ms.
    pub release_ms: f32,
    /// Volume envelope tension, -1..=1.
    pub tension: f32,
    /// Bar repeat.
    pub hold: bool,
    /// Extra time offset, 0..=1 of the 8-beat range.
    pub scratch: f32,
    /// MIDI notes select slots.
    pub midi_trigger: bool,
    /// Crossfade on read-position jumps.
    pub click_reduction: ClickReduction,
    /// Delay the wet signal by the attack time.
    pub attack_compensation: bool,
    /// High-pass the output.
    pub remove_dc: bool,
    /// Windowed-sinc interpolation.
    pub hq: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            time_slot: 0,
            volume_slot: 0,
            time_mix: 1.0,
            volume_mix: 1.0,
            attack_ms: 0.0,
            release_ms: 0.0,
            tension: 0.0,
            hold: false,
            scratch: 0.0,
            midi_trigger: true,
            click_reduction: ClickReduction::Low,
            attack_compensation: false,
            remove_dc: false,
            hq: true,
        }
    }
}

/// Host transport for one block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transport {
    /// The host is playing.
    pub playing: bool,
    /// Tempo in BPM.
    pub tempo: Option<f64>,
    /// Song position in quarter-note beats at the first sample of the block.
    pub pos_beats: Option<f64>,
}

/// A MIDI note event inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteEvent {
    /// Sample offset in the block.
    pub timing: usize,
    /// MIDI note number.
    pub note: u8,
    /// Note on (true) or off (false).
    pub on: bool,
}

/// Engine state the editor shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// Active time slot.
    pub time_slot: usize,
    /// Active volume slot.
    pub volume_slot: usize,
    /// Time slot waiting for its trigger sync.
    pub time_pending: Option<usize>,
    /// Volume slot waiting for its trigger sync.
    pub volume_pending: Option<usize>,
    /// Time envelope position, beats.
    pub time_pos: f64,
    /// Volume envelope position, beats.
    pub volume_pos: f64,
    /// The host is playing.
    pub playing: bool,
    /// Bar repeat is looping.
    pub holding: bool,
    /// Current time offset, beats (including scratch).
    pub offset_beats: f64,
    /// Current smoothed scratch value.
    pub scratch: f32,
    /// Beats of audio recorded so far, capped at the time range.
    pub recorded_beats: f64,
}

/// Per-sample values handed to post stages.
#[derive(Clone, Copy, Debug, Default)]
pub struct StageContext {
    /// Sample rate.
    pub sample_rate: f32,
    /// Samples per beat.
    pub samples_per_beat: f64,
    /// Song position in beats.
    pub beat: f64,
    /// Time envelope value, 0..=1.
    pub time_value: f32,
    /// Final gain applied by the volume lane.
    pub gain: f32,
    /// Read speed (1 = real time).
    pub speed: f32,
}

/// An extra processing stage that runs after the built-in time and volume stages.
pub trait Stage: Send {
    /// Processes one frame.
    fn process(&mut self, ctx: &StageContext, frame: Frame) -> Frame;

    /// Clears internal state.
    fn reset(&mut self) {}
}

#[derive(Clone, Copy, Debug)]
struct Freeze {
    start: u64,
    len: u64,
}

#[derive(Clone, Copy, Debug)]
struct Crossfade {
    pos: f64,
    speed: f64,
    left: usize,
    len: usize,
}

/// The Backspin engine. Owns all audio state.
pub struct Engine {
    sample_rate: f32,
    ring: Ring,
    dry: Ring,
    interp: Interpolator,

    lanes: [LaneState; 2],
    cursors: [Cursor; 2],
    last_params: [usize; 2],
    beat: f64,
    last_block_end: Option<f64>,

    freeze: Option<Freeze>,
    prev_pos: Option<f64>,
    prev_speed: f64,
    xfade: Option<Crossfade>,

    scratch: Inertia,
    gain: AsymSmoother,
    dc: DcBlocker,
    last_values: [f64; 2],

    stages: Vec<Box<dyn Stage>>,
    status: Status,
}

impl Engine {
    /// Creates an engine for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let spb_max = sample_rate as f64 * 60.0 / MIN_TEMPO;
        let cap = (spb_max * TIME_RANGE_BEATS * 1.5) as usize + 4 * GUARD;
        Self {
            sample_rate,
            ring: Ring::new(cap),
            dry: Ring::new(4 * GUARD),
            interp: Interpolator::default(),
            lanes: Default::default(),
            cursors: Default::default(),
            last_params: [0; 2],
            beat: 0.0,
            last_block_end: None,
            freeze: None,
            prev_pos: None,
            prev_speed: 1.0,
            xfade: None,
            scratch: Inertia::default(),
            gain: AsymSmoother::default(),
            dc: DcBlocker::default(),
            last_values: [1.0; 2],
            stages: Vec::new(),
            status: Status::default(),
        }
    }

    /// Latency in samples.
    pub fn latency(&self) -> u32 {
        GUARD as u32
    }

    /// Adds a post stage.
    pub fn push_stage(&mut self, stage: Box<dyn Stage>) {
        self.stages.push(stage);
    }

    /// Returns what the editor shows.
    pub fn status(&self) -> Status {
        self.status
    }

    /// Clears audio history and returns lanes to the given slots.
    pub fn reset(&mut self, settings: &Settings) {
        self.ring.clear();
        self.dry.clear();
        self.lanes[0].reset(settings.time_slot);
        self.lanes[1].reset(settings.volume_slot);
        self.last_params = [settings.time_slot, settings.volume_slot];
        self.cursors = Default::default();
        self.last_block_end = None;
        self.freeze = None;
        self.prev_pos = None;
        self.prev_speed = 1.0;
        self.xfade = None;
        self.scratch.reset(settings.scratch);
        self.gain = AsymSmoother::default();
        self.dc = DcBlocker::default();
        for s in &mut self.stages {
            s.reset();
        }
    }

    fn select(&mut self, bank: &Bank, lane: LaneId, slot: usize, source: Source, immediate: bool) {
        let idx = lane as usize;
        let settings = bank.slot(lane, slot).settings;
        self.lanes[idx].select(slot, self.beat, &settings, source, immediate);
    }

    /// Processes one block in place. `left` and `right` have the same length. `events` must be
    /// sorted by timing.
    pub fn process(
        &mut self,
        bank: &Bank,
        settings: &Settings,
        transport: &Transport,
        events: &[NoteEvent],
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let n = left.len().min(right.len());
        let sr = self.sample_rate;
        let tempo = transport.tempo.filter(|t| *t > 1.0).unwrap_or(DEFAULT_TEMPO).max(MIN_TEMPO);
        let spb = sr as f64 * 60.0 / tempo;
        let playing = transport.playing && transport.pos_beats.is_some();
        let immediate = !playing;

        if playing {
            let pos = transport.pos_beats.unwrap_or(self.beat);
            if let Some(end) = self.last_block_end
                && pos + 1e-6 < end - 0.01
            {
                for lane in &mut self.lanes {
                    lane.rewind(pos);
                }
            }
            self.beat = pos;
        }

        // Parameter-driven slot changes.
        for lane in LaneId::ALL {
            let idx = lane as usize;
            let param = match lane {
                LaneId::Time => settings.time_slot,
                LaneId::Volume => settings.volume_slot,
            };
            if param != self.last_params[idx] {
                self.last_params[idx] = param;
                self.select(bank, lane, param, Source::Param, immediate);
                if lane == LaneId::Time && bank.slot(lane, param).settings.link {
                    self.select(bank, LaneId::Volume, param, Source::Param, immediate);
                }
            }
        }

        let length = DEFAULT_LENGTH;
        let loop_len = (spb * length).round().max(1.0) as u64;
        let xf_len = (settings.click_reduction.crossfade_ms() * 0.001 * sr) as usize;
        let att = one_pole_coeff(settings.attack_ms.max(MIN_GAIN_SMOOTH_MS), sr);
        let rel = one_pole_coeff(settings.release_ms.max(MIN_GAIN_SMOOTH_MS), sr);
        let scratch_coeff = one_pole_coeff(SCRATCH_INERTIA_MS, sr);
        let dc_r = DcBlocker::radius(5.0, sr);
        let comp_samples =
            if settings.attack_compensation { settings.attack_ms as f64 * 0.001 * sr as f64 } else { 0.0 };
        let max_offset = (self.ring.capacity() - 4 * GUARD) as f64;
        let mut ev = 0;

        for i in 0..n {
            while ev < events.len() && events[ev].timing <= i {
                let e = events[ev];
                ev += 1;
                if settings.midi_trigger {
                    self.note(bank, e, immediate);
                }
            }

            // Lane activations.
            for lane in LaneId::ALL {
                let idx = lane as usize;
                let slots = bank.lane(lane);
                let get = |s: usize| -> SlotSettings { slots[s.min(slots.len() - 1)].settings };
                if let Some(act) = self.lanes[idx].tick(self.beat, length, get) {
                    self.cursors[idx] = Cursor::default();
                    if lane == LaneId::Time
                        && act.source != Source::Return
                        && bank.slot(lane, act.slot).settings.link
                        && self.lanes[1].current() != act.slot
                    {
                        let s = bank.slot(LaneId::Volume, act.slot).settings;
                        self.lanes[1].select(act.slot, self.beat, &s, act.source, true);
                        self.lanes[1].tick(self.beat, length, |s| {
                            bank.lane(LaneId::Volume)[s.min(SLOTS_M1)].settings
                        });
                        self.cursors[1] = Cursor::default();
                    }
                }
            }

            // Hold.
            let time_slot = bank.slot(LaneId::Time, self.lanes[0].current());
            let vol_slot = bank.slot(LaneId::Volume, self.lanes[1].current());
            let hold_mode = if settings.hold {
                ForceHold::Last
            } else if time_slot.settings.force_hold != ForceHold::Off {
                time_slot.settings.force_hold
            } else {
                vol_slot.settings.force_hold
            };
            let now = self.ring.written();
            match (hold_mode, self.freeze) {
                (ForceHold::Off, Some(_)) => self.freeze = None,
                (ForceHold::Last, None) => self.freeze = Some(Freeze { start: now, len: loop_len }),
                (ForceHold::Next, None) => {
                    self.freeze = Some(Freeze { start: now + loop_len, len: loop_len })
                }
                _ => {}
            }

            let input = [left[i], right[i]];
            self.dry.push(input);
            let record = match self.freeze {
                Some(f) if now >= f.start => self.ring.frame(now as i64 - f.len as i64),
                _ => input,
            };
            self.ring.push(record);
            let now = now as f64;

            // Time lane.
            let x_time = self.lanes[0].position(self.beat, length);
            let y = self.cursors[0].value(&time_slot.envelope, x_time);
            self.last_values[0] = y;
            let scratch = self.scratch.next(settings.scratch, scratch_coeff);
            let offset_beats = ((1.0 - y) + scratch as f64).clamp(0.0, 1.0) * TIME_RANGE_BEATS;
            let offset = (offset_beats * spb + comp_samples).min(max_offset);
            let pos = now - GUARD as f64 - offset;

            let raw_speed = self.prev_pos.map_or(1.0, |p| pos - p);
            let jumped = (raw_speed - self.prev_speed).abs() > JUMP_THRESHOLD;
            let speed = if jumped { self.prev_speed } else { raw_speed };
            if jumped && xf_len > 0 {
                let from = self.prev_pos.unwrap_or(pos) + self.prev_speed;
                self.xfade = Some(Crossfade { pos: from, speed: self.prev_speed, left: xf_len, len: xf_len });
            }
            self.prev_pos = Some(pos);
            self.prev_speed = speed;

            let mut wet = self.interp.read(&self.ring, pos, speed, settings.hq);
            if let Some(xf) = &mut self.xfade {
                let old = self.interp.read(&self.ring, xf.pos, xf.speed, settings.hq);
                let t = 1.0 - xf.left as f32 / xf.len as f32;
                let (a, b) = ((t * std::f32::consts::FRAC_PI_2).cos(), (t * std::f32::consts::FRAC_PI_2).sin());
                wet = [old[0] * a + wet[0] * b, old[1] * a + wet[1] * b];
                xf.pos += xf.speed;
                xf.left -= 1;
                if xf.left == 0 {
                    self.xfade = None;
                }
            }

            let dry = self.dry.frame(self.dry.written() as i64 - 1 - GUARD as i64);
            let tm = settings.time_mix;
            let mixed = [dry[0] * (1.0 - tm) + wet[0] * tm, dry[1] * (1.0 - tm) + wet[1] * tm];

            // Volume lane.
            let x_vol = self.lanes[1].position(self.beat, length);
            let v = self.cursors[1].value(&vol_slot.envelope, x_vol);
            self.last_values[1] = v;
            let g = apply_tension(v as f32, settings.tension);
            let g = 1.0 - settings.volume_mix + settings.volume_mix * g;
            let g = self.gain.next(g, att, rel);
            let mut out = [mixed[0] * g, mixed[1] * g];

            if settings.remove_dc {
                out = self.dc.next(out, dc_r);
            }

            if !self.stages.is_empty() {
                let ctx = StageContext {
                    sample_rate: sr,
                    samples_per_beat: spb,
                    beat: self.beat,
                    time_value: y as f32,
                    gain: g,
                    speed: speed as f32,
                };
                for stage in &mut self.stages {
                    out = stage.process(&ctx, out);
                }
            }

            left[i] = out[0];
            right[i] = out[1];

            if playing {
                self.beat += 1.0 / spb;
            }

            if i + 1 == n {
                self.status = Status {
                    time_slot: self.lanes[0].current(),
                    volume_slot: self.lanes[1].current(),
                    time_pending: self.lanes[0].pending(),
                    volume_pending: self.lanes[1].pending(),
                    time_pos: x_time,
                    volume_pos: x_vol,
                    playing,
                    holding: self.freeze.is_some_and(|f| self.ring.written() > f.start),
                    offset_beats,
                    scratch,
                    recorded_beats: (self.ring.written() as f64 / spb).min(TIME_RANGE_BEATS),
                };
            }
        }

        // Late events (timing past the block) still count.
        while ev < events.len() {
            let e = events[ev];
            ev += 1;
            if settings.midi_trigger {
                self.note(bank, e, immediate);
            }
        }
        if playing {
            self.last_block_end = Some(self.beat);
        }
    }

    fn note(&mut self, bank: &Bank, e: NoteEvent, immediate: bool) {
        for lane in LaneId::ALL {
            let base = lane.base_note();
            if e.note < base || e.note >= base + crate::SLOTS as u8 {
                continue;
            }
            let slot = (e.note - base) as usize;
            let idx = lane as usize;
            if e.on {
                self.select(bank, lane, slot, Source::Note(e.note), immediate);
                if lane == LaneId::Time && bank.slot(lane, slot).settings.link {
                    self.select(bank, LaneId::Volume, slot, Source::Note(e.note), immediate);
                }
            } else {
                let current = self.lanes[idx].current();
                let s = bank.slot(lane, current).settings;
                if let Some(prev) = self.lanes[idx].note_off(e.note, &s) {
                    self.select(bank, lane, prev, Source::Return, true);
                    if lane == LaneId::Time && s.link {
                        let vcur = self.lanes[1].current();
                        let vs = bank.slot(LaneId::Volume, vcur).settings;
                        if let Some(vprev) = self.lanes[1].note_off(e.note, &SlotSettings { key_held: true, ..vs }) {
                            self.select(bank, LaneId::Volume, vprev, Source::Return, true);
                        }
                    }
                }
            }
        }
    }
}

const SLOTS_M1: usize = crate::SLOTS - 1;

/// Applies the volume tension curve to a gain value.
pub fn apply_tension(g: f32, tension: f32) -> f32 {
    let g = g.clamp(0.0, 1.0);
    if tension >= 0.0 {
        g.powf(1.0 + 4.0 * tension)
    } else {
        1.0 - (1.0 - g).powf(1.0 - 4.0 * tension)
    }
}
