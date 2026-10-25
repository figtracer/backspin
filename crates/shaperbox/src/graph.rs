//! The ShaperBox processor (core.md sections 2 and 4): the shapers in rack order, each one split
//! into bands, each band driven by its section LFOs, mixed per band, summed; then the master mix
//! and the 16-sample base delay. Shapers without a port yet pass audio through.

use crate::bandsplit::{Bands, Bandsplit, multiband_sum};
use crate::ftz::FlushToZero;
use crate::shapers::{Wave as Port, dry_wet, pan::PanBand, time::TimeBand, volume, width};
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

    fn port(&self) -> Port<'_> {
        if self.ramping {
            Port::PerSample(&self.buf)
        } else {
            Port::Constant(self.buf[0])
        }
    }
}

/// The DSP state of one band.
struct BandRt {
    lfos: Vec<LfoRt>,
    gain: Smoother,
    haas_mix: Smoother,
    pan: Option<PanBand>,
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
            gain: Smoother::new(band.float("gain", 0.5), sr as f64, 0.0),
            haas_mix: Smoother::new(band.float("haasMix", 0.0), sr as f64, 0.0),
            pan: (shaper == 3).then(|| PanBand::new(sr)),
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
}

impl Processor {
    pub fn new(state: State, sr: f32) -> Self {
        let mut p = Self {
            sr,
            state: State::default(),
            shapers: Vec::new(),
            delay: [[0.0; 2]; BASE_DELAY],
            dry: Vec::new(),
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
                            let r = crate::shapers::time::range_seconds(
                                band.int("delayTimeMax", 0),
                                t.bpm as f32,
                                1.0,
                            );
                            if let Some(tb) = brt.time.as_mut() {
                                tb.process(audio, port, r);
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
                        _ => {}
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
        // The base delay.
        for f in buf.iter_mut() {
            let out = self.delay[0];
            self.delay.copy_within(1.., 0);
            self.delay[BASE_DELAY - 1] = *f;
            *f = out;
        }
    }
}
