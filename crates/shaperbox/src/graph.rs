//! The ShaperBox processor (core.md sections 2 and 4): the shapers in rack order, each one split
//! into bands, each band driven by its section LFOs, mixed per band, summed; then the master mix
//! and the 16-sample base delay. Shapers without a port yet pass audio through.

use crate::bandsplit::{Bands, Bandsplit, multiband_sum};
use crate::ftz::FlushToZero;
use crate::shapers::{
    Wave as Port,
    crush::{CrushBand, CrushParams},
    drive::{DriveBand, DriveParams, internal_style},
    dry_wet,
    liquid::LiquidBand,
    noise::NoiseBand,
    pan::PanBand,
    time::{Steps, TimeBand},
    volume, width,
};
use crate::smoother::Smoother;
use crate::state::{Band, Lfo, State};
use crate::wave::{Smoothing, Wave, length_beats, rate_hz, smoothing_coef};

/// Sub-block size of processBlock.
pub const BLOCK: usize = 512;
/// The fixed base delay (`useConstantBaseLatency`).
pub const BASE_DELAY: usize = 16;

/// The host transport at the start of a block.
#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub ppq: f64,
    pub bpm: f64,
    pub playing: bool,
}

/// Shaper ids whose waves use the step fix (`FUN_0031399c`).
fn step_fix(shaper: usize) -> bool {
    matches!(shaper, 0 | 4 | 5 | 13)
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
    fn new(lfo: &Lfo, shaper: usize, sr: f32) -> Self {
        Self {
            wave: Wave::new(&lfo.points, step_fix(shaper)),
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

/// The wave port of the section `id` of a band, `default` when the band has no such section.
fn section_port<'a>(band: &Band, lfos: &'a [LfoRt], id: &str, default: f32) -> Port<'a> {
    band.sections
        .iter()
        .position(|s| s.id == id)
        .map(|k| lfos[k].port())
        .unwrap_or(Port::Constant(default))
}

/// The DSP state of one band.
struct BandRt {
    lfos: Vec<LfoRt>,
    crush: Option<Box<CrushBand>>,
    drive: Option<Box<DriveBand>>,
    gain: Smoother,
    haas_mix: Smoother,
    pan: Option<PanBand>,
    liquid: Option<LiquidBand>,
    noise: Option<NoiseBand>,
    stereo_offset: Smoother,
    time: Option<Box<TimeBand>>,
    dry: Vec<[f32; 2]>,
}

impl BandRt {
    fn new(shaper: usize, band: &Band, sr: f32) -> Self {
        Self {
            lfos: band
                .sections
                .iter()
                .map(|s| LfoRt::new(&s.lfo, shaper, sr))
                .collect(),
            crush: (shaper == 1).then(|| Box::new(CrushBand::new(sr, crush_params(band)))),
            drive: (shaper == 9).then(|| Box::new(DriveBand::new(sr, drive_params(band)))),
            gain: Smoother::new(band.float("gain", 0.5), sr as f64, 0.0),
            haas_mix: Smoother::new(band.float("haasMix", 0.0), sr as f64, 0.0),
            pan: (shaper == 3).then(|| PanBand::new(sr)),
            liquid: (shaper == 11).then(|| LiquidBand::new(sr)),
            noise: (shaper == 8).then(|| NoiseBand::new(sr, band as *const Band as usize as i32)),
            stereo_offset: Smoother::new(band.float("stereoOffset", 0.5), sr as f64, 0.0),
            time: (shaper == 0).then(|| Box::new(TimeBand::new(sr))),
            dry: Vec::with_capacity(BLOCK),
        }
    }
}

struct ShaperRt {
    split: Bandsplit,
    bands: Bands,
    rt: [BandRt; 3],
}

/// The ShaperBox section.
pub struct Processor {
    sr: f32,
    state: State,
    shapers: Vec<Option<ShaperRt>>,
    delay: [[f32; 2]; BASE_DELAY],
    dry: Vec<[f32; 2]>,
    /// The latency of the shapers (Drive's oversampling), taken out of the base delay.
    latency: usize,
    /// The dry input delayed by `latency`.
    dry_delay: Vec<[f32; 2]>,
}

impl Processor {
    pub fn new(state: State, sr: f32) -> Self {
        let mut p = Self {
            sr,
            state: State::default(),
            shapers: Vec::new(),
            delay: [[0.0; 2]; BASE_DELAY],
            dry: Vec::new(),
            latency: 0,
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
                (sh.visible && sh.on).then(|| ShaperRt {
                    split: Bandsplit::new(),
                    bands: Bands::default(),
                    rt: [0, 1, 2].map(|b| BandRt::new(k, &sh.bands[b], self.sr)),
                })
            })
            .collect();
        // Drive's oversampling filters run inside the plugin's 16-sample base latency.
        let latency = self.shapers[9]
            .as_ref()
            .and_then(|rt| rt.rt[0].drive.as_ref())
            .map_or(0, |d| d.latency())
            .min(BASE_DELAY);
        if latency != self.latency {
            self.latency = latency;
            self.dry_delay = vec![[0.0; 2]; latency];
        }
        self.state = state;
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Processes a host block in place.
    pub fn process(&mut self, buf: &mut [[f32; 2]], t: Transport) {
        let _ftz = FlushToZero::new();
        let sr = self.sr as f64;
        let mut start = 0;
        while start < buf.len() {
            let n = BLOCK.min(buf.len() - start);
            let sub = Transport {
                ppq: t.ppq + start as f64 * t.bpm / 60.0 / sr,
                ..t
            };
            self.process_block(&mut buf[start..start + n], sub);
            start += n;
        }
    }

    fn process_block(&mut self, buf: &mut [[f32; 2]], t: Transport) {
        let sr = self.sr as f64;
        self.dry.clear();
        self.dry.extend_from_slice(buf);
        if self.latency > 0 {
            let l = self.latency;
            for f in self.dry.iter_mut() {
                let out = self.dry_delay[0];
                self.dry_delay.copy_within(1.., 0);
                self.dry_delay[l - 1] = *f;
                *f = out;
            }
        }
        if !self.state.bypassed {
            for k in self.state.order() {
                let Some(rt) = self.shapers[k].as_mut() else {
                    continue;
                };
                let sh = &self.state.shapers[k];
                let sp = sh.split;
                rt.split.process(
                    buf,
                    sp.low,
                    sp.high,
                    sp.low_slope,
                    sp.high_slope,
                    self.sr,
                    &mut rt.bands,
                );
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
                    let port = brt
                        .lfos
                        .first()
                        .map(|l| l.port())
                        .unwrap_or(Port::Constant(1.0));
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
                            let bits = section_port(band, &brt.lfos, "bits", 1.0);
                            let dec = section_port(band, &brt.lfos, "decimate", 1.0);
                            if let Some(cb) = brt.crush.as_mut() {
                                cb.process(audio, bits, dec, crush_params(band));
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
                            let w = section_port(band, &brt.lfos, "drive", 0.0);
                            if let Some(db) = brt.drive.as_mut() {
                                db.process(audio, w, drive_params(band));
                            }
                        }
                        11 => {
                            brt.stereo_offset
                                .process(band.float("stereoOffset", 0.5), n);
                            let sec = |id: &str| {
                                band.sections
                                    .iter()
                                    .position(|s| s.id == id)
                                    .map(|k| brt.lfos[k].port())
                                    .unwrap_or(Port::Constant(0.5))
                            };
                            let (center, feedback) = (sec("center"), sec("feedback"));
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
                        _ => {}
                    }
                    if let Some(db) = brt.drive.as_mut() {
                        db.delay_dry(&mut brt.dry);
                    }
                    let m = band.mix;
                    dry_wet(audio, &brt.dry, |_| m);
                }
                multiband_sum(
                    &rt.bands,
                    sp.low,
                    sp.high,
                    sp.solo,
                    sp.selected.clamp(0, 2) as usize,
                    buf,
                );
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
        let d = BASE_DELAY - self.latency;
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
