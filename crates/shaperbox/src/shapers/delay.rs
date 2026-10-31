//! Delay (`DelayShaperModulation::process` 0x534ef0, `Delay::process` 0x2e54cc,
//! `FeedbackClient::process` 0x534790; shapers_c.md section 1): a tempo-synced stereo delay with
//! a modulated time, filtered feedback and ping-pong.
//!
//! Per band and block: the time buffer `T (1 - m) + 2 T m w_time` from the centre time `T` (a
//! fraction of a 4/4 bar) and `modulationIntensity`; the band times the send wave goes into the
//! ring (ping-pong: L silent, R the mono sum); the taps are read with the sinc head at the time,
//! L and R stretched by `1 + A` / `1 + B` from `stereoOffset`, or at whole samples when the time
//! is constant; the taps go through the one-pole low-pass then high-pass of the feedback filter;
//! `feedback` times the filtered taps is added back into the ring at this block's input
//! positions (ping-pong: crossed); the output is the filtered taps times the volume wave, mixed
//! with the send-style law (`super::reverb::send_mix`). All arithmetic is f32 with the binary's
//! fused multiply-adds, the delays in f64.
//!
//! ShaperBox 3.6.3 never runs this code: `DelayShaperProcessor::processBlock` is empty in that
//! build (shapers_c.md section 0), so the plugin passes the band through. This port is
//! code-derived and cannot be checked against the plugin.

use super::Wave;
use super::ring::SincRing;
use crate::smoother::Smoother;

/// `DAT_01328cc0`, 20.6 Hz: the cutoff mapping base of the feedback filter.
const E0: f64 = 20.601_722_307_054_37;
/// `DAT_0130dc00`, ln 1024.
const LN1024: f64 = 6.931_471_805_599_453;
const PI_F: f32 = std::f32::consts::PI;

/// The band state of the Delay shaper (`DelayBandState`, shapers_c.md 1.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelayParams {
    /// `delayTimeCenter`, `Defs::DelayTimeCenter` 0..19: the centre time as a bar fraction.
    pub time_center: i32,
    /// `modulationIntensity`: the time wave spans `T (1 - m) .. T (1 + m)`.
    pub modulation: f32,
    /// `feedback`, linear.
    pub feedback: f32,
    /// `pingPong`.
    pub ping_pong: bool,
    /// `stereoOffset`: 0.5 none, below lengthens L, above lengthens R.
    pub stereo_offset: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self {
            time_center: 7,
            modulation: 0.5,
            feedback: 0.5,
            ping_pong: false,
            stereo_offset: 0.5,
        }
    }
}

/// The centre time in seconds (0x534ef0 switch): `f32(240 / bpm)` times the bar fraction of
/// `time_center`; values outside 0..19 give one bar.
pub fn center_seconds(time_center: i32, bpm: f64) -> f32 {
    let bar = (240.0 / bpm) as f32;
    match time_center {
        0 => bar * 0.007_812_5,
        1 => bar * 0.015_625,
        2 => bar * 0.031_25,
        3 => bar * 0.041_666_668,
        4 => bar * 0.0625,
        5 => bar * 0.083_333_336,
        6 => bar * 0.125,
        7 => bar * 0.1875,
        8 => bar * 0.166_666_67,
        9 => bar * 0.25,
        10 => bar * 0.3125,
        11 => bar * 0.375,
        12 => bar * 0.333_333_34,
        13 => bar * 0.5,
        14 => bar * 0.666_666_7,
        16 => bar * 1.5,
        17 => bar * 1.333_333_4,
        18 => bar + bar,
        19 => bar * 4.0,
        _ => bar,
    }
}

/// The L and R stretch factors `A`, `B` of `stereoOffset` (0x534ef0): `s <= 0.5` gives
/// `A = clamp01(1 - 2 s)`, else `B = clamp01(2 s - 1)`.
pub fn stereo_gains(s: f32) -> (f32, f32) {
    if s <= 0.5 {
        (s.mul_add(-2.0, 1.0).clamp(0.0, 1.0), 0.0)
    } else {
        (0.0, s.mul_add(2.0, -1.0).clamp(0.0, 1.0))
    }
}

/// The TPT one-pole coefficient of the feedback filter at `f = E0 1024^p` (0x534790):
/// `fs = min(f / sr, 0.4999)`, `t = max(tan(pi fs), 0)`, `g = t / (t + 1)`, with `f` in f64 and
/// `1 / sr` in f32.
fn filter_g(p: f32, inv_sr: f32) -> f32 {
    let fs = inv_sr * (E0 * (LN1024 * p as f64).exp()) as f32;
    let w = if fs >= 0.0 {
        fs.min(0.4999) * PI_F
    } else {
        0.0
    };
    let t = w.tan().max(0.0);
    t / (t + 1.0)
}

/// One Delay band.
pub struct DelayBand {
    sr: f64,
    inv_sr: f32,
    ring: SincRing,
    /// The previous L and R delays in samples (the sinc rate is `1 + prev - d`).
    prev: [f64; 2],
    /// The feedback filter states per channel: low-pass, high-pass.
    lp: [f32; 2],
    hp: [f32; 2],
    /// The sent block (ping-pong) and the taps.
    sent: Vec<[f32; 2]>,
    taps: Vec<[f32; 2]>,
}

impl DelayBand {
    pub fn new(sr: f32, max_block: usize) -> Self {
        Self {
            sr: sr as f64,
            inv_sr: 1.0 / sr,
            ring: SincRing::new(),
            prev: [0.0; 2],
            lp: [0.0; 2],
            hp: [0.0; 2],
            sent: Vec::with_capacity(max_block),
            taps: Vec::with_capacity(max_block),
        }
    }

    /// Processes one band block in place up to the wet signal (before the band mix). `send`,
    /// `volume` and `time` are the section waves; `lp` and `hp` the smoothed `feedbackLp` and
    /// `feedbackHp` ports.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        buf: &mut [[f32; 2]],
        send: Wave,
        volume: Wave,
        time: Wave,
        p: &DelayParams,
        lp: &Smoother,
        hp: &Smoother,
        bpm: f64,
    ) {
        let n = buf.len();
        let t = center_seconds(p.time_center, bpm);
        let m = p.modulation;
        let tmin = (-m).mul_add(t, t);
        let range = (t + t) * m;
        // The time port: constant when the range is 0 or the wave is constant.
        let constant_time = match time {
            Wave::PerSample(_) if range != 0.0 => None,
            Wave::PerSample(w) => Some(tmin + range * w[0]),
            Wave::Constant(w) => {
                let v = range * w;
                Some(if v != 0.0 { v + tmin } else { tmin })
            }
        };
        let (a, b) = stereo_gains(p.stereo_offset);
        let stereo = a > 0.0 || b > 0.0;
        self.sent.clear();
        for (i, f) in buf.iter().enumerate() {
            let s = send.at(i);
            let sent = [f[0] * s, f[1] * s];
            self.sent.push(if p.ping_pong {
                [0.0, (sent[0] + sent[1]) * 0.5]
            } else {
                sent
            });
        }
        self.ring.write(&self.sent);
        self.taps.clear();
        match constant_time {
            Some(tc) => {
                let d = (self.sr * tc as f64).max(0.0);
                let (kl, kr) = if stereo {
                    ((d * (a + 1.0) as f64) as i64, (d * (b + 1.0) as f64) as i64)
                } else {
                    (d as i64, d as i64)
                };
                for i in 0..n {
                    let l = self.ring.read_int(kl, n - i)[0];
                    let r = self.ring.read_int(kr, n - i)[1];
                    self.taps.push([l, r]);
                }
            }
            None => {
                let Wave::PerSample(w) = time else {
                    unreachable!()
                };
                for (i, &wi) in w.iter().enumerate() {
                    let ts = range.mul_add(wi, tmin);
                    let d = (self.sr * ts as f64).max(0.0);
                    if stereo {
                        let dl = d * (a + 1.0) as f64;
                        let dr = d * (b + 1.0) as f64;
                        let l =
                            self.ring
                                .read_sinc_channel(0, dl, (self.prev[0] - dl) + 1.0, n - i);
                        let r =
                            self.ring
                                .read_sinc_channel(1, dr, (self.prev[1] - dr) + 1.0, n - i);
                        self.prev = [dl, dr];
                        self.taps.push([l, r]);
                    } else {
                        let tap = self.ring.read_sinc(d, (self.prev[0] - d) + 1.0, n - i);
                        self.prev = [d, d];
                        self.taps.push(tap);
                    }
                }
            }
        }
        // The feedback filter: one-pole low-pass, then one-pole high-pass of its output.
        for (i, tap) in self.taps.iter_mut().enumerate() {
            let g1 = filter_g(lp.at(i), self.inv_sr);
            let g2 = filter_g(hp.at(i), self.inv_sr);
            for ((x, lp), hp) in tap.iter_mut().zip(&mut self.lp).zip(&mut self.hp) {
                let v = g1 * (*x - *lp);
                let y = *lp + v;
                *lp = v + y;
                let v2 = g2 * (y - *hp);
                let y2 = *hp + v2;
                *hp = v2 + y2;
                *x = y - y2;
            }
        }
        if p.feedback != 0.0 {
            for (i, tap) in self.taps.iter().enumerate() {
                let v = if p.ping_pong { [tap[1], tap[0]] } else { *tap };
                self.ring.add_scaled(n - i, p.feedback, v);
            }
        }
        for (i, (f, tap)) in buf.iter_mut().zip(&self.taps).enumerate() {
            let v = volume.at(i);
            *f = [tap[0] * v, tap[1] * v];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_times_and_stereo_gains() {
        // 3/16 bar at 120 bpm is 0.375 s; one bar is 2 s; unknown indices give one bar.
        assert_eq!(center_seconds(7, 120.0), 0.375);
        assert_eq!(center_seconds(15, 120.0), 2.0);
        assert_eq!(center_seconds(25, 120.0), 2.0);
        assert_eq!(center_seconds(18, 120.0), 4.0);
        assert_eq!(stereo_gains(0.5), (0.0, 0.0));
        assert_eq!(stereo_gains(0.25), (0.5, 0.0));
        assert_eq!(stereo_gains(1.0), (0.0, 1.0));
        assert_eq!(stereo_gains(0.0), (1.0, 0.0));
    }
}
