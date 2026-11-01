//! The ShaperBox processor (core.md sections 2 and 4): the shapers in rack order, each one split
//! into bands, each band driven by its section LFOs and their envelope followers, mixed per band,
//! summed; then the master mix and the 16-sample base delay. The Oscilloscope and Shift shapers
//! pass audio through, as in ShaperBox (`shapers::scope`, `shapers::shift`); the Delay and Limiter
//! run the DSP that ShaperBox 3.6.3 ships but never calls (shapers_c.md).
//!
//! Latency ([`Processor::latency`]): with `useConstantBaseLatency` (the default) the output runs
//! [`BASE_DELAY`] samples late, and Drive's oversampling filters run inside that delay (the dry
//! path is delayed by their latency and the base delay shortened by it). Without it the output
//! runs late by Drive's latency only (0 when Drive is off), as in ShaperBox (core.md section 1).
//! The Pitch shifter's latency (WSOLA 1687, PSOLA 2381 / 2861 samples at 48 kHz) is not reported
//! and not compensated: ShaperBox reports 0 for it too (shapers_b.md 2.3) and only delays the
//! band's dry path, so the pitched signal lags the input in both. The Limiter's lookahead (up to
//! 288 + 12 samples at 48 kHz) is treated the same way. ShaperBox itself reports 0 to
//! the host; Backspin reports this value so the host aligns the fixed delay. All buffers are
//! allocated when the state is set; `process` does not allocate.
//!
//! Triggers (core.md 5.5): [`Processor::process_with_events`] takes the block's MIDI notes.
//! Every shaper receives every trigger and processes its block in sub-blocks split at the
//! trigger times, so a reset is sample-accurate. A trigger reaches shaper `i` late by its
//! lookahead `A + sum(L_j, j < i)` where `A` is the audio trigger's lookahead (while some section
//! listens to it; MIDI events only) and `L_j` the latencies of the shapers before it (Pitch's
//! shifter, Volume's 6 ms smoothing lookahead); the audio is delayed by the same amount, so the
//! trigger lands on its own audio sample. Free-running LFOs run on the undelayed clock from the
//! start; synced ones read the transport position `lookahead` samples back. The audio trigger
//! analyses the undelayed input (or sidechain) and emits its triggers in the same block.

use crate::bandsplit::{Bands, Bandsplit, multiband_sum};
use crate::ftz::FlushToZero;
use crate::shapers::{
    Wave as Port,
    crush::{CrushBand, CrushParams},
    delay::{DelayBand, DelayParams},
    drive::{DriveBand, DriveParams, internal_style},
    dry_wet,
    dynamics::DynamicsBand,
    envfollow::{EnvParams, EnvShaper, EnvSources, SectionFollower},
    filter::FilterBand,
    limiter::{LimiterBand, LimiterParams},
    liquid::LiquidBand,
    noise::NoiseBand,
    pan::PanBand,
    pitch::{PitchBand, PitchParams},
    reverb::{ReverbBand, ReverbParams, hall_ir, send_mix},
    time::{Steps, TimeBand},
    volume, width,
};
use crate::smoother::Smoother;
use crate::state::{Band, Lfo, Shaper, State};
use crate::trigger::{
    AudioSmoother, CubicSmoother, Detector, ModSmoothing, PlainSmoother, audio_lookahead,
    audio_smoothed, cubic_lookahead, mod_smoothing,
};
use crate::wave::{Smoothing, TABLE, Wave, length_beats, rate_hz, smoothing_coef};

/// Sub-block size of processBlock.
pub const BLOCK: usize = 512;
/// The fixed base delay (`useConstantBaseLatency`).
pub const BASE_DELAY: usize = 16;
/// The most sections a band has (every shaper has at most two).
const MAX_SECTIONS: usize = 4;
/// The Delay shaper.
const DELAY: usize = 6;
/// The Dynamics shaper.
const DYNAMICS: usize = 10;
/// The Limiter shaper.
const LIMITER: usize = 14;
/// The Pitch shaper.
const PITCH: usize = 15;
/// The Volume shaper.
const VOLUME: usize = 5;
/// The most pending triggers a shaper keeps (one block of events plus the carry-over).
const MAX_TRIGGERS: usize = 1024;

/// The host transport at the start of a block.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub ppq: f64,
    pub bpm: f64,
    pub playing: bool,
}

/// A MIDI note of the block passed to [`Processor::process_with_events`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    /// The sample offset inside the block.
    pub offset: usize,
    /// The MIDI channel, 1..16 (0 when unknown: it then passes every channel filter).
    pub channel: u8,
    pub note: u8,
    /// Note-on with a velocity above 0. A note-on with velocity 0 is a note-off.
    pub on: bool,
}

/// A trigger pending for a shaper (0x460f54): `time` in samples from the start of the current
/// block, already shifted by the shaper's lookahead.
#[derive(Clone, Copy, Debug)]
struct Trigger {
    time: i64,
    /// From the audio trigger (ShaperBox event flag 1) rather than a MIDI note.
    audio: bool,
    /// The cubic smoother's advance notice (ShaperBox event type 4), not a reset.
    arm: bool,
    note: u8,
}

/// `OscControllerNote` (0x2de5f0): the frequency of a MIDI note in float32.
fn note_hz(note: i32) -> f64 {
    (440.0 * ((note - 69) as f32 / 12.0).exp2()) as f64
}

/// Whether section `index` of a shaper reads its wave with the step fix (`FUN_0031399c`). The
/// Reverb's send section (index 0) reads without it (shapers_b.md 5.8).
fn step_fix(shaper: usize, index: usize) -> bool {
    matches!(shaper, 0 | 5 | 13) || (shaper == 4 && index != 0)
}

/// The one-shot state of a `WaveOsc` (+0x90).
#[derive(Clone, Copy, Debug, PartialEq)]
enum OneShot {
    Running,
    /// The wave reached its end phase: ramp to the end value with 64 increments per sample.
    Ramp,
    /// Hold the end value until the next trigger.
    Done,
}

/// One section LFO of a band.
struct LfoRt {
    lfo: Lfo,
    wave: Wave,
    smoothing: Smoothing,
    phase: f64,
    /// The phase at the start of the last rendered block.
    phase0: f64,
    /// The rate in Hz of the last rendered block.
    rate: f64,
    buf: Vec<f32>,
    ramping: bool,
    /// One-shot: the end phase `max(oneShotEndPhase, 1/32)` and the table position where the
    /// wave stops, 128 table units early in the cubic quality mode (0x2def1c).
    end: f64,
    end_pos: f64,
    state: OneShot,
    /// The last table value before the end, the ramp progress (0x2df304, 0x2df100).
    last: f32,
    ramp_t: f64,
    /// The last raw wave value rendered (+0x88).
    last_out: f32,
    /// The frequency of the last MIDI note (Note timing).
    note_hz: f64,
    cubic: Option<CubicSmoother>,
    plain: Option<PlainSmoother>,
    /// A reset marked the plain smoother for the next render (+0x31).
    plain_triggered: bool,
}

impl LfoRt {
    fn new(lfo: &Lfo, step_fix: bool, sr: f32, smoothing: ModSmoothing) -> Self {
        let end = (lfo.one_shot_end_phase.max(0.03125)) as f64;
        Self {
            wave: Wave::new(&lfo.points, step_fix),
            smoothing: Smoothing::new(smoothing_coef(lfo.smoothing, sr)),
            phase: 0.0,
            phase0: 0.0,
            rate: 0.0,
            buf: Vec::with_capacity(BLOCK),
            ramping: false,
            end,
            end_pos: (end + 32.0) * TABLE as f64 - 128.0,
            state: if lfo.one_shot() {
                OneShot::Done
            } else {
                OneShot::Running
            },
            last: 0.0,
            ramp_t: 0.0,
            last_out: 0.0,
            note_hz: note_hz(60),
            cubic: (smoothing == ModSmoothing::Cubic).then(|| CubicSmoother::new(sr)),
            plain: (smoothing == ModSmoothing::Plain).then(|| PlainSmoother::new(sr)),
            plain_triggered: false,
            lfo: lfo.clone(),
        }
    }

    /// The raw wave value at phase 0, which a reset jumps to.
    fn start_value(&self) -> f32 {
        self.wave.constant.unwrap_or_else(|| self.wave.table[0])
    }

    /// 0x461794: a trigger restarts the wave (0x2df388, 0x2df4e0) and sets the Note frequency.
    fn trigger(&mut self, note: u8) {
        self.phase = 0.0;
        self.state = OneShot::Running;
        let offset = self.lfo.pitch_offset.round() as i32;
        self.note_hz = note_hz((note as i32 + offset).clamp(0, 127));
    }

    /// The rate in Hz for the transport.
    fn set_rate(&mut self, t: Transport) {
        self.rate = match self.lfo.timing_mode {
            1 => rate_hz(self.lfo.speed),
            2 => self.note_hz,
            _ => t.bpm / (60.0 * length_beats(self.lfo.length)),
        };
    }

    /// The value held after a one-shot ends (0x2df100): the wave at the end phase, snapped to
    /// +-1 beyond +-0.9.
    fn target(&self) -> f32 {
        let v = self.wave.read((self.end + 32.0) * TABLE as f64);
        if v > 0.9 {
            1.0
        } else if v < -0.9 {
            -1.0
        } else {
            v
        }
    }

    /// 0x2df100: the `remaining` samples of a block of `n` after the one-shot end.
    fn tail(&mut self, n: usize, inc: f64, remaining: usize) {
        let tgt = self.target();
        if self.state == OneShot::Ramp {
            for _ in 0..remaining {
                self.ramp_t = (self.ramp_t + inc * 64.0).min(1.0);
                self.buf
                    .push((self.ramp_t as f32).mul_add(tgt - self.last, self.last));
            }
            if self.ramp_t >= 1.0 {
                self.state = OneShot::Done;
            }
        } else {
            self.buf.resize(self.buf.len() + remaining, tgt);
        }
        self.phase += inc * n as f64;
    }

    /// Renders `n` samples of `w` (the wave mapped to [0, 1]) into `buf`. A synced Beat LFO reads
    /// the transport position `lookahead` samples before the block (0x2ddc1c).
    fn render(&mut self, n: usize, t: Transport, sr: f64, lookahead: usize) {
        self.set_rate(t);
        let inc = self.rate / sr;
        let running = self.state != OneShot::Done;
        if running && self.lfo.trigger_mode == 0 && self.lfo.timing_mode == 0 {
            let beats = length_beats(self.lfo.length);
            let p = (t.ppq - lookahead as f64 * t.bpm / 60.0 / sr) / beats;
            self.phase = p - p.floor();
        }
        if running {
            self.phase -= self.phase.floor();
        }
        self.phase0 = self.phase;
        self.buf.clear();
        let one_shot = self.lfo.one_shot();
        if one_shot && self.state != OneShot::Running {
            self.ramping = self.state == OneShot::Ramp;
            self.tail(n, inc, n);
        } else if let Some(c) = self.wave.constant
            && !one_shot
        {
            self.buf.resize(n, c);
            self.ramping = false;
            self.phase += n as f64 * inc;
        } else {
            let mut pos = (self.phase + 32.0) * TABLE as f64;
            let mut k = 0;
            while k < n {
                self.buf.push(self.wave.read(pos));
                pos += inc * TABLE as f64;
                k += 1;
                if one_shot && self.end_pos <= pos {
                    // 0x2df304: the shipped quality mode always ramps to the end value.
                    if self.state == OneShot::Running {
                        self.last = self.buf[k - 1];
                        self.state = OneShot::Ramp;
                        self.ramp_t = 0.0;
                    }
                    self.tail(n, inc, n - k);
                    break;
                }
            }
            if k == n {
                // 0x2df6b0 keeps the phase from the position it reached, not `phase + n inc`.
                self.phase = pos / TABLE as f64 - 32.0;
            }
            self.ramping = true;
        }
        self.last_out = self.buf[n - 1];
        for v in &mut self.buf {
            *v = *v * 0.5 + 0.5;
        }
        if let Some(c) = self.cubic.as_mut() {
            self.ramping |= c.process(&mut self.buf);
        }
        if let Some(p) = self.plain.as_mut() {
            self.ramping |= p.process(&mut self.buf, self.plain_triggered);
            self.plain_triggered = false;
        }
        if self.lfo.smoothing != 0.0 && self.smoothing.coef < 1.0 {
            self.ramping = self.smoothing.process(&mut self.buf, self.ramping);
        }
    }

    /// 0x461b6c: the cubic smoother's target at a trigger, the wave's start value and its slope
    /// per sample.
    fn arm(&mut self, t: Transport, sr: f64) {
        self.set_rate(t);
        if let Some(c) = self.cubic.as_mut() {
            let (w0, w1) = match self.wave.constant {
                Some(c) => (c, c),
                None => (self.wave.table[0], self.wave.table[1]),
            };
            let (p0, p1) = (w0 * 0.5 + 0.5, w1 * 0.5 + 0.5);
            let spp = ((sr / self.rate) as f32).max(1e-12);
            c.arm(p0, (p1 - p0) * (TABLE as f32 / spp));
        }
    }

    /// The step-crossfade input of the Time shaper (shapers_a.md 5.3): on for a moving wave with
    /// steps unless the LFO is one-shot outside Note timing.
    fn steps(&self) -> Option<Steps<'_>> {
        let on = self.lfo.timing_mode == 2 || self.lfo.trigger_mode != 2;
        (on && self.wave.constant.is_none() && !self.wave.steps.is_empty()).then(|| Steps {
            table: &self.wave.table,
            steps: &self.wave.steps,
            phase: self.phase0,
            rate: self.rate,
        })
    }

    fn port(&self) -> Port<'_> {
        if self.ramping {
            Port::PerSample(&self.buf)
        } else {
            Port::Constant(self.buf[0])
        }
    }
}

/// The Crush parameters of a band.
fn crush_params(band: &Band) -> CrushParams {
    CrushParams {
        dc_offset: band.float("bitsDcOffset", 0.0),
        jitter: band.float("jitter", 0.0),
        treble: band.float("treble", 1.0),
        damp: band.boolean("damp", false),
        dither: band.boolean("dither", false),
    }
}

/// The Drive parameters of a band.
fn drive_params(band: &Band) -> DriveParams {
    DriveParams {
        style: internal_style(band.int("style", 0)),
        input_gain: band.float("inputGain", 0.5),
        output_gain: band.float("outputGain", 0.5),
        push: band.float("push", 0.0),
        grip: band.float("grip", 0.0),
        tone: band.float("tone", 0.5),
        accent: band.float("accent", 0.5),
    }
}

/// The Reverb parameters of a band (the IR loader inputs and the pre-delay).
fn reverb_params(band: &Band) -> ReverbParams {
    let d = ReverbParams::default();
    ReverbParams {
        stretch: band.float("stretchFactor", d.stretch),
        decay: band.float("decayTime", d.decay),
        gated: band.boolean("gatedMode", d.gated),
        delay_time: band.float("delayTime", d.delay_time),
        delay_sync: band.int("delaySync", d.delay_sync),
    }
}

/// The Reverb band of `band`: "Standard Hall 1" when the band names it; the "Off" IR and the
/// library IRs, which are encrypted downloads, give a silent wet path as in the plugin.
fn reverb_band(band: &Band, sr: f32) -> ReverbBand {
    let file = band.string("impulseResponseFile").unwrap_or("");
    let ir = file.contains("Standard Hall 1").then(hall_ir);
    ReverbBand::new(sr, ir.as_deref(), reverb_params(band))
}

/// The Pitch parameters of a band; `mode` is the shaper's.
fn pitch_params(band: &Band, mode: i32) -> PitchParams {
    let d = PitchParams::default();
    PitchParams {
        mode,
        grain_size_wsola: band.int("grainSizeWsola", d.grain_size_wsola),
        grain_size_cyclic: band.float("grainSizeCyclic", d.grain_size_cyclic),
        link_formant: band.boolean("linkFormant", d.link_formant),
        fill_gaps: band.float("fillGaps", d.fill_gaps),
    }
}

/// The Delay parameters of a band.
fn delay_params(band: &Band) -> DelayParams {
    let d = DelayParams::default();
    DelayParams {
        time_center: band.int("delayTimeCenter", d.time_center),
        modulation: band.float("modulationIntensity", d.modulation),
        feedback: band.float("feedback", d.feedback),
        ping_pong: band.boolean("pingPong", d.ping_pong),
        stereo_offset: band.float("stereoOffset", d.stereo_offset),
    }
}

/// The Limiter parameters of a band.
fn limiter_params(band: &Band) -> LimiterParams {
    let d = LimiterParams::default();
    LimiterParams {
        lookahead: band.float("lookahead", d.lookahead),
        release: band.float("release", d.release),
        auto_release: band.boolean("autoRelease", d.auto_release),
        transient_boost: band.int("transientBoost", d.transient_boost),
        transient_attack: band.float("transientAttack", d.transient_attack),
    }
}

/// The envelope follower parameters of the section `id` of a band (defaults: inactive).
fn section_env(band: &Band, id: &str) -> EnvParams {
    band.section(id).map_or(EnvParams::default(), |s| s.env)
}

/// Whether shaper `k` has a band processor (the Oscilloscope and Shift pass audio through).
fn ported(k: usize) -> bool {
    matches!(k, 0..=11 | LIMITER | PITCH)
}

/// Which merge slots a section follower of shaper `k` uses.
fn env_shaper(k: usize) -> EnvShaper {
    match k {
        4 => EnvShaper::Reverb,
        8 => EnvShaper::Noise,
        PITCH => EnvShaper::Pitch,
        _ => EnvShaper::Default,
    }
}

/// The port of the section `id` of a band, `default` when the band has no such section.
fn section_port<'a>(band: &Band, ports: &[Port<'a>], id: &str, default: f32) -> Port<'a> {
    band.sections
        .iter()
        .position(|s| s.id == id)
        .and_then(|k| ports.get(k).copied())
        .unwrap_or(Port::Constant(default))
}

/// A smoothed port whose buffer holds a block without reallocating.
fn smoother(v: f32, sr: f32) -> Smoother {
    let mut s = Smoother::new(v, sr as f64, 0.0);
    s.buf.reserve(BLOCK);
    s
}

/// The DSP state of one band.
struct BandRt {
    lfos: Vec<LfoRt>,
    /// The envelope follower of each section whose follower is active (not for Dynamics).
    followers: Vec<Option<SectionFollower>>,
    crush: Option<Box<CrushBand>>,
    delay: Option<Box<DelayBand>>,
    drive: Option<Box<DriveBand>>,
    dynamics: Option<Box<DynamicsBand>>,
    feedback_hp: Smoother,
    feedback_lp: Smoother,
    filter: Option<Box<FilterBand>>,
    gain: Smoother,
    haas_mix: Smoother,
    limiter: Option<Box<LimiterBand>>,
    pan: Option<PanBand>,
    liquid: Option<LiquidBand>,
    noise: Option<NoiseBand>,
    pitch: Option<Box<PitchBand>>,
    reverb: Option<Box<ReverbBand>>,
    stereo_offset: Smoother,
    time: Option<Box<TimeBand>>,
    width: Smoother,
    dry: Vec<[f32; 2]>,
}

impl BandRt {
    fn new(shaper: usize, sh: &Shaper, band: &Band, sr: f32, smoothing: ModSmoothing) -> Self {
        let follow = ported(shaper) && shaper != DYNAMICS;
        Self {
            lfos: band
                .sections
                .iter()
                .enumerate()
                .map(|(k, s)| LfoRt::new(&s.lfo, step_fix(shaper, k), sr, smoothing))
                .collect(),
            followers: band
                .sections
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    (follow && s.env.active && k < MAX_SECTIONS)
                        .then(|| SectionFollower::new(sr, BLOCK, env_shaper(shaper), &s.env))
                })
                .collect(),
            crush: (shaper == 1).then(|| Box::new(CrushBand::new(sr, crush_params(band)))),
            delay: (shaper == DELAY).then(|| Box::new(DelayBand::new(sr, BLOCK))),
            drive: (shaper == 9).then(|| Box::new(DriveBand::new(sr, drive_params(band)))),
            dynamics: (shaper == DYNAMICS)
                .then(|| Box::new(DynamicsBand::new(sr, BLOCK, &section_env(band, "dynamics")))),
            feedback_hp: smoother(band.float("feedbackHp", 0.0), sr),
            feedback_lp: smoother(band.float("feedbackLp", 1.0), sr),
            filter: (shaper == 2).then(|| {
                Box::new(FilterBand::new(
                    sr as f64,
                    band.int("typeWithoutBypass", 2),
                    band.float("drive", 0.182_575),
                    band.boolean("safeResonance", true),
                    band.int("clippingCurve", 0),
                ))
            }),
            gain: smoother(band.float("gain", 0.5), sr),
            haas_mix: smoother(band.float("haasMix", 0.0), sr),
            limiter: (shaper == LIMITER)
                .then(|| Box::new(LimiterBand::new(sr, limiter_params(band)))),
            pan: (shaper == 3).then(|| PanBand::new(sr)),
            liquid: (shaper == 11).then(|| LiquidBand::new(sr)),
            noise: (shaper == 8).then(|| NoiseBand::new(sr, band as *const Band as usize as i32)),
            pitch: (shaper == PITCH)
                .then(|| Box::new(PitchBand::new(sr, BLOCK, &pitch_params(band, sh.mode)))),
            reverb: (shaper == 4).then(|| Box::new(reverb_band(band, sr))),
            stereo_offset: smoother(band.float("stereoOffset", 0.5), sr),
            time: (shaper == 0).then(|| Box::new(TimeBand::new(sr))),
            width: smoother(band.float("width", 0.5), sr),
            dry: Vec::with_capacity(BLOCK),
        }
    }
}

struct ShaperRt {
    split: Bandsplit,
    bands: Bands,
    rt: [BandRt; 3],
    /// A copy of the unsplit shaper input: the free detector filter's source and Pitch's
    /// transient source. Filled only when `needs_full`.
    full: Vec<[f32; 2]>,
    needs_full: bool,
    /// The split of the sidechain bus, run only when a follower listens to it.
    sc_split: Bandsplit,
    sc_bands: Bands,
    needs_sc: bool,
    /// The pending triggers, in samples from the start of the current block.
    events: Vec<Trigger>,
    /// The latency of the shapers before this one in the chain (node offset +0x30).
    chain: usize,
    /// The Volume's own smoothing lookahead: its input runs this late (vtable +0x40).
    lookahead: usize,
    input_delay: Vec<[f32; 2]>,
    input_pos: usize,
    /// The audio click suppressor of the shapers that have one, with `triggerSmoothing`.
    audio_smoother: Option<AudioSmoother>,
}

impl ShaperRt {
    fn new(k: usize, sh: &Shaper, sr: f32, trigger_smoothing: bool) -> Self {
        let envs = || {
            sh.bands
                .iter()
                .flat_map(|b| b.sections.iter().map(|s| s.env))
                .filter(|e| e.active)
        };
        let follows = ported(k) && envs().next().is_some();
        let smoothing = if trigger_smoothing {
            mod_smoothing(k)
        } else {
            ModSmoothing::None
        };
        let free = sh
            .bands
            .iter()
            .flat_map(|b| b.sections.iter())
            .any(|s| s.lfo.free());
        let lookahead = if smoothing == ModSmoothing::Cubic && free {
            cubic_lookahead(sr)
        } else {
            0
        };
        Self {
            split: Bandsplit::with_capacity(BLOCK),
            bands: Bands::with_capacity(BLOCK),
            rt: [0, 1, 2].map(|b| BandRt::new(k, sh, &sh.bands[b], sr, smoothing)),
            full: Vec::with_capacity(BLOCK),
            needs_full: k == PITCH || follows,
            sc_split: Bandsplit::with_capacity(BLOCK),
            sc_bands: Bands::with_capacity(BLOCK),
            needs_sc: follows && envs().any(|e| e.use_sidechain),
            events: Vec::with_capacity(MAX_TRIGGERS),
            chain: 0,
            lookahead,
            input_delay: vec![[0.0; 2]; lookahead],
            input_pos: 0,
            audio_smoother: (trigger_smoothing && audio_smoothed(k))
                .then(|| AudioSmoother::new(sr)),
        }
    }

    /// The latency this shaper adds to the chain after it (vtable +0x40): the Pitch shifter's,
    /// the Volume's smoothing lookahead. Drive's oversampling adds nothing (0x60c2ec).
    fn latency(&self, k: usize, sh: &Shaper, sr: f32) -> usize {
        match k {
            PITCH => crate::shapers::pitch::latency(sr, sh.mode),
            _ => self.lookahead,
        }
    }

    /// Queues a trigger at `time` (block-relative, shifted by this shaper's lookahead) with its
    /// `arm` notice for the cubic smoother. A full queue drops the event: no allocation.
    fn push(&mut self, time: i64, audio: bool, note: u8) {
        if self.lookahead > 0 && self.events.len() < self.events.capacity() {
            self.events.push(Trigger {
                time,
                audio,
                arm: true,
                note,
            });
        }
        if self.events.len() < self.events.capacity() {
            self.events.push(Trigger {
                time: time + self.lookahead as i64,
                audio,
                arm: false,
                note,
            });
        }
    }

    /// 0x461794: applies a trigger to the sections that listen to its source.
    fn dispatch(&mut self, ev: Trigger, sh: &Shaper, t: Transport, sr: f64) {
        let mut any = false;
        let (low_on, high_on) = crate::bandsplit::active(sh.split.low, sh.split.high);
        for (b, brt) in self.rt.iter_mut().enumerate() {
            let produced = match b {
                0 => low_on,
                2 => high_on,
                _ => true,
            };
            let active = produced && (!sh.split.solo || sh.split.selected == b as i32);
            for l in &mut brt.lfos {
                let listens = if ev.audio {
                    l.lfo.audio_triggered()
                } else {
                    l.lfo.free() && (l.lfo.trigger_source == 0 || l.lfo.timing_mode == 2)
                };
                if !listens {
                    continue;
                }
                if ev.arm {
                    l.arm(t, sr);
                    continue;
                }
                l.trigger(ev.note);
                if active && (l.last_out - l.start_value()).abs() >= 1e-10 {
                    l.plain_triggered = true;
                    any = true;
                }
            }
        }
        if any && let Some(a) = self.audio_smoother.as_mut() {
            a.trigger();
        }
    }
}

/// One sub-block of shaper `k`: the band split, the section LFOs with their lookahead, the band
/// processors, the band mixes and the sum; then the audio click suppressor at a trigger.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn process_shaper(
    sr_f32: f32,
    sh: &Shaper,
    k: usize,
    rt: &mut ShaperRt,
    buf: &mut [[f32; 2]],
    sidechain: Option<&[[f32; 2]]>,
    t: Transport,
    lookahead: usize,
) {
    let sr = sr_f32 as f64;
    let sp = sh.split;
    let selected = sp.selected.clamp(0, 2) as usize;
    if rt.needs_full {
        rt.full.clear();
        rt.full.extend_from_slice(buf);
    }
    // Every shaper, Pitch included, splits before its band processors (Bandsplit
    // mode 0). core.md 3.5 reads a mode 1 for Delay and Pitch from the code, but the
    // harness shows Pitch with the mode 0 split, bit for bit.
    rt.split.process(
        buf,
        sp.low,
        sp.high,
        sp.low_slope,
        sp.high_slope,
        sr_f32,
        &mut rt.bands,
    );
    // The sidechain bus split with the shaper's own crossover.
    let sc = sidechain.filter(|_| rt.needs_sc);
    if let Some(sc) = sc {
        rt.sc_split.process(
            sc,
            sp.low,
            sp.high,
            sp.low_slope,
            sp.high_slope,
            sr_f32,
            &mut rt.sc_bands,
        );
    }
    for b in 0..3 {
        if !rt.bands.produced[b] {
            continue;
        }
        let band = &sh.bands[b];
        let brt = &mut rt.rt[b];
        let n = buf.len();
        for l in &mut brt.lfos {
            l.render(n, t, sr, lookahead);
        }
        let audio = &mut rt.bands.b[b];
        brt.dry.clear();
        brt.dry.extend_from_slice(audio);
        let src = EnvSources {
            full: if rt.needs_full { &rt.full } else { &brt.dry },
            sidechain_band: sc
                .and_then(|_| rt.sc_bands.produced[b].then_some(&rt.sc_bands.b[b][..])),
            sidechain_full: sc,
        };
        // The section ports: the LFO, through its envelope follower when active.
        let mut ports = [Port::Constant(1.0); MAX_SECTIONS];
        let sections = ports.iter_mut().zip(&brt.lfos).zip(&mut brt.followers);
        for (((p, l), f), s) in sections.zip(&band.sections) {
            *p = match f {
                Some(f) => f.process(l.port(), &brt.dry, &src, &s.env),
                None => l.port(),
            };
        }
        let port = if brt.lfos.is_empty() {
            Port::Constant(1.0)
        } else {
            ports[0]
        };
        match k {
            0 => {
                let lfo = brt.lfos.first();
                let r = crate::shapers::time::range_seconds(
                    band.int("delayTimeMax", 0),
                    t.bpm as f32,
                    lfo.map_or(1.0, |l| l.rate as f32),
                );
                let steps = lfo.and_then(|l| l.steps());
                if let Some(tb) = brt.time.as_mut() {
                    tb.process(audio, port, r, steps);
                }
            }
            1 => {
                let bits = section_port(band, &ports, "bits", 1.0);
                let dec = section_port(band, &ports, "decimate", 1.0);
                if let Some(cb) = brt.crush.as_mut() {
                    cb.process(audio, bits, dec, crush_params(band));
                }
            }
            2 => {
                brt.stereo_offset
                    .process(band.float("stereoOffset", 0.5), n);
                let cutoff = section_port(band, &ports, "cutoff", 1.0);
                let resonance = section_port(band, &ports, "resonance", 0.0);
                if let Some(fb) = brt.filter.as_mut() {
                    fb.process(audio, cutoff, resonance, &brt.stereo_offset);
                }
            }
            3 => {
                brt.haas_mix.process(band.float("haasMix", 0.0), n);
                if let Some(pb) = brt.pan.as_mut() {
                    pb.process(
                        audio,
                        port,
                        band.int("panMode", 0),
                        band.float("haasTime", 0.3553),
                        &brt.haas_mix,
                    );
                }
            }
            4 => {
                brt.width.process(band.float("width", 0.5), n);
                // Section 0 is the send wave, section 1 the volume wave.
                let nsec = brt.lfos.len();
                let wave = |k: usize| {
                    if k < nsec {
                        ports[k]
                    } else {
                        Port::Constant(1.0)
                    }
                };
                if let Some(rb) = brt.reverb.as_mut() {
                    rb.process(audio, wave(0), wave(1), &brt.width, t.bpm);
                }
            }
            5 => {
                brt.gain.process(band.float("gain", 0.5), n);
                volume::process(audio, port, &brt.gain);
            }
            DELAY => {
                brt.feedback_lp.process(band.float("feedbackLp", 1.0), n);
                brt.feedback_hp.process(band.float("feedbackHp", 0.0), n);
                let send = section_port(band, &ports, "send", 1.0);
                let volume = section_port(band, &ports, "volume", 1.0);
                let time = section_port(band, &ports, "time", 0.5);
                if let Some(db) = brt.delay.as_mut() {
                    db.process(
                        audio,
                        send,
                        volume,
                        time,
                        &delay_params(band),
                        &brt.feedback_lp,
                        &brt.feedback_hp,
                        t.bpm,
                    );
                }
            }
            7 => width::process(audio, port, band.float("gain", 0.5)),
            8 => {
                let file = band.string("file").unwrap_or("");
                let generated = file.ends_with(".generated") && !file.contains("/0 Off");
                if let Some(nb) = brt.noise.as_mut() {
                    nb.process(
                        audio,
                        port,
                        band.float("gain", 0.8),
                        band.float("lpCutoff", 1.0),
                        band.float("hpCutoff", 0.254),
                        band.boolean("isMono", false),
                        band.boolean("solo", false),
                        generated,
                    );
                }
            }
            9 => {
                let w = section_port(band, &ports, "drive", 0.0);
                if let Some(db) = brt.drive.as_mut() {
                    db.process(audio, w, drive_params(band));
                }
            }
            DYNAMICS => {
                if let Some(db) = brt.dynamics.as_mut() {
                    db.process(audio, &section_env(band, "dynamics"), &src);
                }
            }
            11 => {
                brt.stereo_offset
                    .process(band.float("stereoOffset", 0.5), n);
                let center = section_port(band, &ports, "center", 0.5);
                let feedback = section_port(band, &ports, "feedback", 0.5);
                if let Some(lq) = brt.liquid.as_mut() {
                    lq.process(
                        audio,
                        center,
                        feedback,
                        band.int("typeWithBypass", 1),
                        band.int("mode", 0),
                        band.int("phaserStages", 2),
                        &brt.stereo_offset,
                    );
                }
            }
            LIMITER => {
                if let Some(lb) = brt.limiter.as_mut() {
                    lb.process(audio, section_port(band, &ports, "limiter", 1.0));
                }
            }
            PITCH => {
                let pitch = section_port(band, &ports, "pitch", 0.5);
                let formant = section_port(band, &ports, "formant", 0.5);
                if let Some(pb) = brt.pitch.as_mut() {
                    let p = pitch_params(band, sh.mode);
                    pb.process(audio, pitch, formant, &p, Some(src.full));
                }
            }
            _ => {}
        }
        if let Some(db) = brt.drive.as_mut() {
            db.delay_dry(&mut brt.dry);
        }
        if let Some(pb) = brt.pitch.as_mut() {
            pb.delay_dry(&mut brt.dry);
        }
        if let Some(lb) = brt.limiter.as_mut() {
            lb.delay_dry(&mut brt.dry);
        }
        if matches!(k, 4 | DELAY | PITCH) {
            send_mix(audio, &brt.dry, band.mix);
        } else {
            dry_wet(audio, &brt.dry, band.mix);
        }
    }
    multiband_sum(&rt.bands, sp.low, sp.high, sp.solo, selected, buf);
    if let Some(a) = rt.audio_smoother.as_mut() {
        a.process(buf);
    }
}

/// Orders the pending triggers by time, keeping the order of equal times: a stable insertion
/// sort, since the standard stable sort allocates for longer lists and this runs on the audio
/// thread. The list is short in practice (the notes of one block plus the carry-over).
fn sort_triggers(events: &mut [Trigger]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].time > events[j].time {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// The ShaperBox section.
pub struct Processor {
    sr: f32,
    state: State,
    /// The processing order of the state.
    order: Vec<usize>,
    shapers: Vec<Option<ShaperRt>>,
    delay: [[f32; 2]; BASE_DELAY],
    dry: Vec<[f32; 2]>,
    /// The latency of the shapers (Drive's oversampling), taken out of the base delay.
    shaper_latency: usize,
    /// The output delay: [`BASE_DELAY`] with `useConstantBaseLatency`, else `shaper_latency`.
    base: usize,
    /// The dry input delayed by `shaper_latency`.
    dry_delay: Vec<[f32; 2]>,
    /// The audio trigger detector, while a section listens to the audio trigger.
    detector: Option<Detector>,
    /// The audio trigger's lookahead: the input runs this late behind the detector.
    audio_lookahead: usize,
    in_delay: Vec<[f32; 2]>,
    sc_delay: Vec<[f32; 2]>,
    in_pos: usize,
    /// The delayed sidechain of the block, and a silent block for a missing sidechain bus.
    sc_buf: Vec<[f32; 2]>,
    silence: Vec<[f32; 2]>,
    /// The audio trigger's offsets in the current block.
    audio_events: Vec<usize>,
}

impl Processor {
    pub fn new(state: State, sr: f32) -> Self {
        let mut p = Self {
            sr,
            state: State::default(),
            order: Vec::new(),
            shapers: Vec::new(),
            delay: [[0.0; 2]; BASE_DELAY],
            dry: Vec::with_capacity(BLOCK),
            shaper_latency: 0,
            base: BASE_DELAY,
            dry_delay: Vec::new(),
            detector: None,
            audio_lookahead: 0,
            in_delay: Vec::new(),
            sc_delay: Vec::new(),
            in_pos: 0,
            sc_buf: Vec::with_capacity(BLOCK),
            silence: vec![[0.0; 2]; BLOCK],
            audio_events: Vec::with_capacity(BLOCK),
        };
        p.set_state(state);
        p
    }

    /// Replaces the state and rebuilds the DSP of every shaper.
    pub fn set_state(&mut self, state: State) {
        self.shapers = (0..16)
            .map(|k| {
                let sh = &state.shapers[k];
                (sh.visible && sh.on)
                    .then(|| ShaperRt::new(k, sh, self.sr, state.trigger_smoothing))
            })
            .collect();
        // The audio trigger runs, and delays the input, while a section listens to it (0x60c2ec).
        let listens = self.shapers.iter().enumerate().any(|(k, rt)| {
            rt.is_some()
                && state.shapers[k]
                    .bands
                    .iter()
                    .flat_map(|b| b.sections.iter())
                    .any(|s| s.lfo.audio_triggered())
        });
        self.audio_lookahead = if listens {
            audio_lookahead(self.sr, &state.audio_trigger)
        } else {
            0
        };
        self.detector = listens.then(|| Detector::new(self.sr, &state.audio_trigger));
        self.in_delay = vec![[0.0; 2]; self.audio_lookahead];
        self.sc_delay = vec![[0.0; 2]; self.audio_lookahead];
        self.in_pos = 0;
        // The node offsets: the latency of the shapers before each one in the chain.
        let order = state.order();
        let mut chain = 0;
        for &k in &order {
            if let Some(rt) = self.shapers[k].as_mut() {
                rt.chain = chain;
                chain += rt.latency(k, &state.shapers[k], self.sr);
            }
        }
        // Drive's oversampling filters run inside the plugin's 16-sample base latency.
        let latency = self.shapers[9]
            .as_ref()
            .and_then(|rt| rt.rt[0].drive.as_ref())
            .map_or(0, |d| d.latency())
            .min(BASE_DELAY);
        if latency != self.shaper_latency {
            self.shaper_latency = latency;
            self.dry_delay = vec![[0.0; 2]; latency];
        }
        self.base = if state.use_constant_base_latency {
            BASE_DELAY
        } else {
            latency
        };
        self.order = order;
        self.state = state;
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The latency to report to the host in samples: the output delay, [`BASE_DELAY`] unless
    /// the state turns `useConstantBaseLatency` off, plus the audio trigger's and the Volume's
    /// smoothing lookaheads. Pitch's shifter latency is not included (see the module
    /// documentation).
    pub fn latency(&self) -> usize {
        let volume = self.shapers[VOLUME].as_ref().map_or(0, |rt| rt.lookahead);
        self.base + self.audio_lookahead + volume
    }

    /// Processes a host block in place, without a sidechain bus or MIDI.
    pub fn process(&mut self, buf: &mut [[f32; 2]], t: Transport) {
        self.process_with_events(buf, t, None, &[]);
    }

    /// Processes a host block in place with a sidechain bus and no MIDI.
    pub fn process_with_sidechain(
        &mut self,
        buf: &mut [[f32; 2]],
        t: Transport,
        sidechain: Option<&[[f32; 2]]>,
    ) {
        self.process_with_events(buf, t, sidechain, &[]);
    }

    /// Processes a host block in place. `sidechain` is the host's sidechain bus (input bus 1),
    /// aligned with `buf`, for the envelope followers with `useSidechain` and the audio trigger
    /// with `useSidechain`; `None` when there is no bus, and those then hear silence, as in
    /// ShaperBox. `events` are the block's MIDI notes in any order; only note-ons trigger
    /// (0x609f60), through the `inChannel` and `fireWavesViaMidi` filters.
    pub fn process_with_events(
        &mut self,
        buf: &mut [[f32; 2]],
        t: Transport,
        sidechain: Option<&[[f32; 2]]>,
        events: &[NoteEvent],
    ) {
        let _ftz = FlushToZero::new();
        let sr = self.sr as f64;
        let mut start = 0;
        while start < buf.len() {
            let n = BLOCK.min(buf.len() - start);
            let sub = Transport {
                ppq: t.ppq + start as f64 * t.bpm / 60.0 / sr,
                ..t
            };
            let sc = sidechain.map(|s| &s[start..start + n]);
            self.queue_notes(events, start, n);
            self.process_block(&mut buf[start..start + n], sub, sc);
            start += n;
        }
    }

    /// Queues the note-ons of `events` that fall in `start..start + n` for every shaper.
    fn queue_notes(&mut self, events: &[NoteEvent], start: usize, n: usize) {
        let st = &self.state;
        for e in events {
            let in_block = (start..start + n).contains(&e.offset);
            let channel =
                st.midi_in_channel == 0 || e.channel == 0 || st.midi_in_channel == e.channel as i32;
            // With `fireWavesViaMidi` the pitch classes C#..B select waves; only C triggers.
            let fires = !st.fire_waves_via_midi || e.note % 12 == 0;
            if !(e.on && in_block && channel && fires) {
                continue;
            }
            let time = (e.offset - start + self.audio_lookahead) as i64;
            for rt in self.shapers.iter_mut().flatten() {
                rt.push(time + rt.chain as i64, false, e.note);
            }
        }
    }

    /// Delays `buf` through the ring `line` at `pos` (its length is the delay).
    fn delay_line(line: &mut [[f32; 2]], pos: &mut usize, buf: &mut [[f32; 2]]) {
        let len = line.len();
        if len == 0 {
            return;
        }
        for f in buf.iter_mut() {
            std::mem::swap(&mut line[*pos], f);
            *pos = (*pos + 1) % len;
        }
    }

    fn process_block(
        &mut self,
        buf: &mut [[f32; 2]],
        t: Transport,
        sidechain: Option<&[[f32; 2]]>,
    ) {
        let sr = self.sr as f64;
        let n = buf.len();
        // The audio trigger on the undelayed input or sidechain (0x60a408); its triggers reach the
        // shapers in this block (0x460f54).
        self.audio_events.clear();
        if let Some(d) = self.detector.as_mut() {
            let input = if d.params().use_sidechain {
                sidechain.unwrap_or(&self.silence[..n])
            } else {
                &*buf
            };
            d.process(input, &mut self.audio_events);
            for &off in &self.audio_events {
                for rt in self.shapers.iter_mut().flatten() {
                    rt.push((off + rt.chain) as i64, true, 60);
                }
            }
        }
        // The input runs late by the audio trigger's lookahead.
        let mut sc_pos = self.in_pos;
        Self::delay_line(&mut self.in_delay, &mut self.in_pos, buf);
        let sidechain = match sidechain {
            Some(sc) if self.audio_lookahead > 0 => {
                self.sc_buf.clear();
                self.sc_buf.extend_from_slice(sc);
                Self::delay_line(&mut self.sc_delay, &mut sc_pos, &mut self.sc_buf);
                Some(&self.sc_buf[..])
            }
            other => other,
        };
        self.dry.clear();
        self.dry.extend_from_slice(buf);
        if self.shaper_latency > 0 {
            let l = self.shaper_latency;
            for f in self.dry.iter_mut() {
                let out = self.dry_delay[0];
                self.dry_delay.copy_within(1.., 0);
                self.dry_delay[l - 1] = *f;
                *f = out;
            }
        }
        if !self.state.bypassed {
            for &k in &self.order {
                let Some(rt) = self.shapers[k].as_mut() else {
                    continue;
                };
                let sh = &self.state.shapers[k];
                // The Volume's input runs late by its smoothing lookahead.
                let mut pos = rt.input_pos;
                Self::delay_line(&mut rt.input_delay, &mut pos, buf);
                rt.input_pos = pos;
                // The block in sub-blocks split at the pending triggers (0x460f54).
                sort_triggers(&mut rt.events);
                let lookahead = self.audio_lookahead + rt.chain + rt.lookahead;
                let mut start = 0;
                let mut i = 0;
                while start < n {
                    while i < rt.events.len() && rt.events[i].time <= start as i64 {
                        let ev = rt.events[i];
                        let sub = Transport {
                            ppq: t.ppq + start as f64 * t.bpm / 60.0 / sr,
                            ..t
                        };
                        rt.dispatch(ev, sh, sub, sr);
                        i += 1;
                    }
                    let end = match rt.events.get(i) {
                        Some(e) if (e.time as usize) < n => e.time as usize,
                        _ => n,
                    };
                    let sub = Transport {
                        ppq: t.ppq + start as f64 * t.bpm / 60.0 / sr,
                        ..t
                    };
                    let sc = sidechain.map(|s| &s[start..end]);
                    process_shaper(self.sr, sh, k, rt, &mut buf[start..end], sc, sub, lookahead);
                    start = end;
                }
                rt.events.drain(..i);
                for e in &mut rt.events {
                    e.time -= n as i64;
                }
            }
            let m = self.state.master_mix;
            if m != 1.0 {
                for (o, d) in buf.iter_mut().zip(&self.dry) {
                    for c in 0..2 {
                        o[c] = (1.0 - m) * d[c] + m * o[c];
                    }
                }
            }
        } else {
            buf.copy_from_slice(&self.dry);
        }
        // The base delay, less the latency the shapers already added.
        let d = self.base - self.shaper_latency;
        for f in buf.iter_mut() {
            if d == 0 {
                break;
            }
            let out = self.delay[BASE_DELAY - d];
            self.delay.copy_within(1.., 0);
            self.delay[BASE_DELAY - 1] = *f;
            *f = out;
        }
    }
}
