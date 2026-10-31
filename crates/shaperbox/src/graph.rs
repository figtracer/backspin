//! The ShaperBox processor (core.md sections 2 and 4): the shapers in rack order, each one split
//! into bands, each band driven by its section LFOs and their envelope followers, mixed per band,
//! summed; then the master mix and the 16-sample base delay. Shapers without a port yet pass audio
//! through.
//!
//! Latency ([`Processor::latency`]): with `useConstantBaseLatency` (the default) the output runs
//! [`BASE_DELAY`] samples late, and Drive's oversampling filters run inside that delay (the dry
//! path is delayed by their latency and the base delay shortened by it). Without it the output
//! runs late by Drive's latency only (0 when Drive is off), as in ShaperBox (core.md section 1).
//! The Pitch shifter's latency (WSOLA 1687, PSOLA 2381 / 2861 samples at 48 kHz) is not reported
//! and not compensated: ShaperBox reports 0 for it too (shapers_b.md 2.3) and only delays the
//! band's dry path, so the pitched signal lags the input in both. ShaperBox itself reports 0 to
//! the host; Backspin reports this value so the host aligns the fixed delay. All buffers are
//! allocated when the state is set; `process` does not allocate.

use crate::bandsplit::{Bands, Bandsplit, multiband_sum};
use crate::ftz::FlushToZero;
use crate::shapers::{
    Wave as Port,
    crush::{CrushBand, CrushParams},
    drive::{DriveBand, DriveParams, internal_style},
    dry_wet,
    dynamics::DynamicsBand,
    envfollow::{EnvParams, EnvShaper, EnvSources, SectionFollower},
    filter::FilterBand,
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
use crate::wave::{Smoothing, Wave, length_beats, rate_hz, smoothing_coef};

/// Sub-block size of processBlock.
pub const BLOCK: usize = 512;
/// The fixed base delay (`useConstantBaseLatency`).
pub const BASE_DELAY: usize = 16;
/// The most sections a band has (every shaper has at most two).
const MAX_SECTIONS: usize = 4;
/// The Dynamics shaper.
const DYNAMICS: usize = 10;
/// The Pitch shaper.
const PITCH: usize = 15;

/// The host transport at the start of a block.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub ppq: f64,
    pub bpm: f64,
    pub playing: bool,
}

/// Whether section `index` of a shaper reads its wave with the step fix (`FUN_0031399c`). The
/// Reverb's send section (index 0) reads without it (shapers_b.md 5.8).
fn step_fix(shaper: usize, index: usize) -> bool {
    matches!(shaper, 0 | 5 | 13) || (shaper == 4 && index != 0)
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
}

impl LfoRt {
    fn new(lfo: &Lfo, step_fix: bool, sr: f32) -> Self {
        Self {
            wave: Wave::new(&lfo.points, step_fix),
            smoothing: Smoothing::new(smoothing_coef(lfo.smoothing, sr)),
            lfo: lfo.clone(),
            phase: 0.0,
            phase0: 0.0,
            rate: 0.0,
            buf: Vec::with_capacity(BLOCK),
            ramping: false,
        }
    }

    /// Renders `n` samples of `w` (the wave mapped to [0, 1]) into `buf`.
    fn render(&mut self, n: usize, t: Transport, sr: f64) {
        let beats = length_beats(self.lfo.length);
        let hz = self.lfo.timing_mode == 1;
        let rate = if hz {
            rate_hz(self.lfo.speed)
        } else {
            t.bpm / (60.0 * beats)
        };
        let inc = rate / sr;
        if !hz && self.lfo.trigger_mode == 0 {
            let p = t.ppq / beats;
            self.phase = p - p.floor();
        }
        self.phase0 = self.phase;
        self.rate = rate;
        self.buf.clear();
        if let Some(c) = self.wave.constant {
            self.buf.resize(n, c * 0.5 + 0.5);
            self.ramping = false;
        } else {
            let mut pos = (self.phase + 32.0) * 8192.0;
            for _ in 0..n {
                let y = self.wave.read(pos);
                pos = inc.mul_add(8192.0, pos);
                self.buf.push(y * 0.5 + 0.5);
            }
            self.ramping = true;
        }
        self.phase += n as f64 * inc;
        if self.lfo.smoothing != 0.0 && self.smoothing.coef < 1.0 {
            self.ramping = self.smoothing.process(&mut self.buf, self.ramping);
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

/// The envelope follower parameters of the section `id` of a band (defaults: inactive).
fn section_env(band: &Band, id: &str) -> EnvParams {
    band.section(id).map_or(EnvParams::default(), |s| s.env)
}

/// Whether shaper `k` has a band processor (the others pass audio through).
fn ported(k: usize) -> bool {
    matches!(k, 0..=5 | 7..=11 | PITCH)
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
    drive: Option<Box<DriveBand>>,
    dynamics: Option<Box<DynamicsBand>>,
    filter: Option<Box<FilterBand>>,
    gain: Smoother,
    haas_mix: Smoother,
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
    fn new(shaper: usize, sh: &Shaper, band: &Band, sr: f32) -> Self {
        let follow = ported(shaper) && shaper != DYNAMICS;
        Self {
            lfos: band
                .sections
                .iter()
                .enumerate()
                .map(|(k, s)| LfoRt::new(&s.lfo, step_fix(shaper, k), sr))
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
            drive: (shaper == 9).then(|| Box::new(DriveBand::new(sr, drive_params(band)))),
            dynamics: (shaper == DYNAMICS)
                .then(|| Box::new(DynamicsBand::new(sr, BLOCK, &section_env(band, "dynamics")))),
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
}

impl ShaperRt {
    fn new(k: usize, sh: &Shaper, sr: f32) -> Self {
        let envs = || {
            sh.bands
                .iter()
                .flat_map(|b| b.sections.iter().map(|s| s.env))
                .filter(|e| e.active)
        };
        let follows = ported(k) && envs().next().is_some();
        Self {
            split: Bandsplit::with_capacity(BLOCK),
            bands: Bands::with_capacity(BLOCK),
            rt: [0, 1, 2].map(|b| BandRt::new(k, sh, &sh.bands[b], sr)),
            full: Vec::with_capacity(BLOCK),
            needs_full: k == PITCH || follows,
            sc_split: Bandsplit::with_capacity(BLOCK),
            sc_bands: Bands::with_capacity(BLOCK),
            needs_sc: follows && envs().any(|e| e.use_sidechain),
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
        };
        p.set_state(state);
        p
    }

    /// Replaces the state and rebuilds the DSP of every shaper.
    pub fn set_state(&mut self, state: State) {
        self.shapers = (0..16)
            .map(|k| {
                let sh = &state.shapers[k];
                (sh.visible && sh.on).then(|| ShaperRt::new(k, sh, self.sr))
            })
            .collect();
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
        self.order = state.order();
        self.state = state;
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The latency to report to the host in samples: the output delay, [`BASE_DELAY`] unless
    /// the state turns `useConstantBaseLatency` off. Pitch's shifter latency is not included (see
    /// the module documentation).
    pub fn latency(&self) -> usize {
        self.base
    }

    /// Processes a host block in place, without a sidechain bus.
    pub fn process(&mut self, buf: &mut [[f32; 2]], t: Transport) {
        self.process_with_sidechain(buf, t, None);
    }

    /// Processes a host block in place. `sidechain` is the host's sidechain bus (input bus 1),
    /// aligned with `buf`, for the envelope followers with `useSidechain`; `None` when there is
    /// no bus, and those followers then hear silence, as in ShaperBox.
    pub fn process_with_sidechain(
        &mut self,
        buf: &mut [[f32; 2]],
        t: Transport,
        sidechain: Option<&[[f32; 2]]>,
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
            self.process_block(&mut buf[start..start + n], sub, sc);
            start += n;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn process_block(
        &mut self,
        buf: &mut [[f32; 2]],
        t: Transport,
        sidechain: Option<&[[f32; 2]]>,
    ) {
        let sr = self.sr as f64;
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
                    self.sr,
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
                        self.sr,
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
                        l.render(n, t, sr);
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
                        7 => width::process(audio, port, band.float("gain", 0.5)),
                        8 => {
                            let file = band.string("file").unwrap_or("");
                            let generated =
                                file.ends_with(".generated") && !file.contains("/0 Off");
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
                    if k == 4 || k == PITCH {
                        send_mix(audio, &brt.dry, band.mix);
                    } else {
                        dry_wet(audio, &brt.dry, band.mix);
                    }
                }
                multiband_sum(&rt.bands, sp.low, sp.high, sp.solo, selected, buf);
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
