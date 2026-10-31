//! Dynamics (`DynamicsShaperModulation::process` `FUN_004ad49c`; shapers_b.md section 7, model
//! `.amp/in/sb/data/sb_dynamics_exact.py`): a gain stage driven by the section's envelope
//! follower with the Analog core. Per sample `audio *= 2^(-GR/6) * makeup`.
//!
//! Wiring: the band state has `mix` only (generic `dry_wet`). The section `dynamics` holds the
//! LFO and the `ShaperBoxEnvelopeFollowerState` ([`EnvParams`], every id there, init defaults);
//! `mode`, `depth`, `knee` and `offset` have no effect here. The section LFO wave has no effect on
//! the audio either (verified: the port the follower multiplies is 1), so the band takes no wave.
//! No latency; each band has its own follower.

use super::envfollow::{
    AnalogCore, DetectorInput, EnvParams, EnvSources, ratio_from_amount, threshold_cu,
};
use crate::smoother::Smoother;

/// `2^((48 m - 24)/6)`; auto makeup adds `(1 - attack)^2 (-0.75 ratio) thr_cu` to the exponent
/// numerator (`attack` is the raw state value).
pub fn makeup_gain(makeup: f32, auto_makeup: bool, thr_cu: f32, ratio: f32, attack: f32) -> f32 {
    let mk = makeup.mul_add(48.0, -24.0);
    if auto_makeup {
        let x = 1.0 - attack;
        let k = ratio * -0.75;
        ((mk + (x * x) * (k * thr_cu)) * 0.166_666_67).exp2()
    } else {
        (mk * 0.166_666_67).exp2()
    }
}

/// One Dynamics band.
pub struct DynamicsBand {
    core: AnalogCore,
    det: DetectorInput,
    threshold: Smoother,
    amount: Smoother,
    makeup: Smoother,
    gr: Vec<f32>,
    max_block: usize,
}

impl DynamicsBand {
    pub fn new(sr: f32, max_block: usize, p: &EnvParams) -> Self {
        let smoother = |v: f32| {
            let mut s = Smoother::new(v, sr as f64, 0.0);
            s.buf.reserve(max_block);
            s
        };
        Self {
            core: AnalogCore::new(sr, p.attack, p.hold, p.release),
            det: DetectorInput::new(sr, max_block),
            threshold: smoother(p.threshold),
            amount: smoother(p.amount),
            makeup: smoother(p.makeup),
            gr: vec![0.0; max_block],
            max_block,
        }
    }

    /// The latency in samples: none.
    pub fn latency(&self) -> usize {
        0
    }

    /// The gain reduction of the last block in units of 6 log2 (6.0206 dB), for a meter.
    pub fn gain_reduction(&self) -> &[f32] {
        &self.gr
    }

    /// Processes the band audio `buf` in place. `src` carries the other detector inputs, aligned
    /// with `buf` (see [`EnvSources`]).
    pub fn process(&mut self, buf: &mut [[f32; 2]], p: &EnvParams, src: &EnvSources) {
        let mut a = 0;
        while a < buf.len() {
            let b = buf.len().min(a + self.max_block);
            self.process_block(&mut buf[a..b], p, &src.range(a, b));
            a = b;
        }
    }

    fn process_block(&mut self, buf: &mut [[f32; 2]], p: &EnvParams, src: &EnvSources) {
        let n = buf.len();
        // The 50 Hz smoothers of threshold, amount and makeup (constant for constant settings;
        // the plugin's ramping branches are not verified).
        self.threshold.process(p.threshold, n);
        self.amount.process(p.amount, n);
        self.makeup.process(p.makeup, n);
        if !p.active {
            self.gr[..n].fill(0.0);
            return;
        }
        self.core.set_times(p.attack, p.hold, p.release);
        let auto_makeup = p.auto_makeup && !p.use_sidechain;
        let ramping = self.threshold.ramping || self.amount.ramping || self.makeup.ramping;
        let thr_c = threshold_cu(self.threshold.value);
        let ratio_c = ratio_from_amount(self.amount.value);
        let gain_c = makeup_gain(self.makeup.value, auto_makeup, thr_c, ratio_c, p.attack);
        let det = self.det.fill(p, n, src);
        for (i, f) in buf.iter_mut().enumerate() {
            let x = det.map_or(*f, |d| d[i]);
            let (thr, ratio, gain) = if ramping {
                let thr = threshold_cu(self.threshold.at(i));
                let ratio = ratio_from_amount(self.amount.at(i));
                let g = makeup_gain(self.makeup.at(i), auto_makeup, thr, ratio, p.attack);
                (thr, ratio, g)
            } else {
                (thr_c, ratio_c, gain_c)
            };
            let gr = self.core.tick(x, thr, ratio, p.auto_release);
            self.gr[i] = gr;
            let sec = (gr * -0.166_666_67).exp2();
            f[0] = f[0] * sec * gain;
            f[1] = f[1] * sec * gain;
        }
    }
}
