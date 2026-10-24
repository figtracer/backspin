//! Behavior tests for the engine against SPEC.md.

use backspin_engine::{
    envelope::{Envelope, Point, Shape},
    factory_bank, Bank, ClickReduction, Engine, ForceHold, LaneId, NoteEvent, Settings, Sync,
    Transport, GUARD,
};

const SR: f32 = 48000.0;
const TEMPO: f64 = 120.0;
const SPB: usize = 24000;
const BLOCK: usize = 512;

struct Rig {
    engine: Engine,
    bank: Bank,
    settings: Settings,
    beat: f64,
    playing: bool,
}

impl Rig {
    fn new() -> Self {
        let mut bank = factory_bank();
        // Default tests use plain, immediate slots.
        for lane in LaneId::ALL {
            for slot in bank.lane_mut(lane) {
                slot.settings.trigger_sync = Sync::Off;
                slot.settings.position_sync = Sync::FourBeats;
            }
        }
        let settings = Settings {
            click_reduction: ClickReduction::Off,
            hq: false,
            ..Settings::default()
        };
        let mut engine = Engine::new(SR);
        engine.reset(&settings);
        Self { engine, bank, settings, beat: 0.0, playing: true }
    }

    fn set_time(&mut self, slot: usize, env: Envelope) {
        self.bank.time[slot].envelope = env;
    }

    fn set_volume(&mut self, slot: usize, env: Envelope) {
        self.bank.volume[slot].envelope = env;
    }

    /// Runs `input` through the engine and returns the left channel.
    fn run(&mut self, input: &[f32], events: &[NoteEvent]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len());
        for (b, chunk) in input.chunks(BLOCK).enumerate() {
            let mut l = chunk.to_vec();
            let mut r = chunk.to_vec();
            let start = b * BLOCK;
            let evs: Vec<NoteEvent> = events
                .iter()
                .filter(|e| e.timing >= start && e.timing < start + chunk.len())
                .map(|e| NoteEvent { timing: e.timing - start, ..*e })
                .collect();
            let transport = Transport {
                playing: self.playing,
                tempo: Some(TEMPO),
                pos_beats: Some(self.beat),
            };
            self.engine.process(&self.bank, &self.settings, &transport, &evs, &mut l, &mut r);
            assert_eq!(l, r, "channels diverged");
            out.extend_from_slice(&l);
            if self.playing {
                self.beat += chunk.len() as f64 / SPB as f64;
            }
        }
        out
    }
}

fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 / n as f32).collect()
}

fn sine(n: usize, freq: f32) -> Vec<f32> {
    (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
}

fn zero_crossings(x: &[f32]) -> usize {
    x.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count()
}

#[test]
fn live_slot_is_a_pure_delay_of_guard() {
    let mut rig = Rig::new();
    let input = ramp(SPB * 2);
    let out = rig.run(&input, &[]);
    for i in GUARD..input.len() {
        assert!((out[i] - input[i - GUARD]).abs() < 1e-5, "at {i}");
    }
    assert_eq!(rig.engine.latency() as usize, GUARD);
}

#[test]
fn flat_offset_delays_by_beats() {
    let mut rig = Rig::new();
    // One beat back: y = 1 - 1/8.
    rig.set_time(0, Envelope::flat(1.0 - 1.0 / 8.0));
    let input = ramp(SPB * 3);
    let out = rig.run(&input, &[]);
    let d = GUARD + SPB;
    for i in d..input.len() {
        assert!((out[i] - input[i - d]).abs() < 1e-4, "at {i}: {} vs {}", out[i], input[i - d]);
    }
    // Before a beat of audio exists, the output is silent.
    assert!(out[..SPB].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn half_speed_halves_the_frequency() {
    let mut rig = Rig::new();
    rig.set_time(0, Envelope::from_points(vec![Point::new(0.0, 1.0), Point::new(4.0, 0.75)]));
    let input = sine(SPB * 4, 440.0);
    let out = rig.run(&input, &[]);
    // Compare zero crossings over beats 1..3 (inside the envelope, past the start).
    let a = zero_crossings(&input[SPB..SPB * 3]) as f64;
    let b = zero_crossings(&out[SPB..SPB * 3]) as f64;
    assert!((b / a - 0.5).abs() < 0.02, "ratio {}", b / a);
}

#[test]
fn safety_line_slope_freezes_playback() {
    let mut rig = Rig::new();
    // dy/dx = -1/8: speed 0.
    rig.set_time(0, Envelope::from_points(vec![Point::new(0.0, 1.0), Point::new(4.0, 0.5)]));
    let input = ramp(SPB * 4);
    let out = rig.run(&input, &[]);
    let probe = &out[SPB..SPB * 3];
    let spread = probe.iter().fold(f32::MIN, |a, &b| a.max(b)) - probe.iter().fold(f32::MAX, |a, &b| a.min(b));
    assert!(spread < 1e-3, "spread {spread}");
}

#[test]
fn reverse_reads_backwards() {
    let mut rig = Rig::new();
    rig.set_time(0, Envelope::from_points(vec![Point::new(0.0, 1.0), Point::new(4.0, 0.0)]));
    let input = ramp(SPB * 8);
    let out = rig.run(&input, &[]);
    // In bar 1 the read head is past the recorded audio: silence.
    assert!(out[SPB..SPB * 2].iter().all(|v| v.abs() < 1e-6));
    // In bar 2, speed -1: the ramp plays downward.
    let at = SPB * 5;
    let slope = out[at + 100] - out[at];
    assert!(slope < 0.0, "slope {slope}");
    let per_sample = (out[at + 1000] - out[at]) / 1000.0;
    let ramp_step = 1.0 / input.len() as f32;
    assert!((per_sample + ramp_step).abs() < ramp_step * 0.01, "{per_sample} vs -{ramp_step}");
}

#[test]
fn volume_gate_silences_off_steps() {
    let mut rig = Rig::new();
    let mut gate = Envelope::flat(1.0);
    gate.create_sequence(&[true, false], 0.5, 1.0, 0.0);
    rig.set_volume(0, gate);
    let input = vec![1.0; SPB * 2];
    let out = rig.run(&input, &[]);
    let q = SPB / 4;
    // Middle of an "on" step and of an "off" step.
    assert!((out[q] - 1.0).abs() < 1e-3, "{}", out[q]);
    assert!(out[SPB / 2 + q].abs() < 1e-3, "{}", out[SPB / 2 + q]);
}

#[test]
fn volume_mix_and_tension() {
    let mut rig = Rig::new();
    rig.set_volume(0, Envelope::flat(0.5));
    rig.settings.volume_mix = 0.5;
    let out = rig.run(&vec![1.0; SPB], &[]);
    assert!((out[SPB - 1] - 0.75).abs() < 1e-3, "{}", out[SPB - 1]);

    let mut rig = Rig::new();
    rig.set_volume(0, Envelope::flat(0.5));
    rig.settings.tension = 0.25;
    let out = rig.run(&vec![1.0; SPB], &[]);
    assert!((out[SPB - 1] - 0.25).abs() < 1e-3, "{}", out[SPB - 1]);
}

#[test]
fn time_mix_zero_is_dry() {
    let mut rig = Rig::new();
    rig.set_time(0, Envelope::flat(0.5));
    rig.settings.time_mix = 0.0;
    let input = ramp(SPB * 2);
    let out = rig.run(&input, &[]);
    for i in GUARD..input.len() {
        assert!((out[i] - input[i - GUARD]).abs() < 1e-5);
    }
}

#[test]
fn midi_note_selects_time_slot_and_key_held_returns() {
    let mut rig = Rig::new();
    rig.set_time(4, Envelope::flat(0.5));
    rig.bank.time[4].settings.key_held = true;
    let input = vec![1.0; SPB * 6];
    let on = SPB * 5;
    let off = on + SPB / 2;
    let events = [
        NoteEvent { timing: on, note: 64, on: true },
        NoteEvent { timing: off, note: 64, on: false },
    ];
    let out = rig.run(&input, &events);
    // Slot 5 reads 4 beats back, still inside the constant input: output stays 1.
    assert!((out[on + 100] - 1.0).abs() < 1e-4);
    let status = rig.engine.status();
    assert_eq!(status.time_slot, 0, "key-held slot did not return");
}

#[test]
fn midi_note_selects_volume_slot() {
    let mut rig = Rig::new();
    rig.set_volume(2, Envelope::flat(0.0));
    let input = vec![1.0; SPB];
    let events = [NoteEvent { timing: 1000, note: 26, on: true }];
    let out = rig.run(&input, &events);
    assert!(out[900].abs() > 0.99);
    assert!(out[5000].abs() < 1e-3);
    assert_eq!(rig.engine.status().volume_slot, 2);
}

#[test]
fn trigger_sync_waits_for_beat() {
    let mut rig = Rig::new();
    rig.bank.volume[3].envelope = Envelope::flat(0.0);
    rig.bank.volume[3].settings.trigger_sync = Sync::Beat;
    let input = vec![1.0; SPB * 2];
    let events = [NoteEvent { timing: SPB / 3, note: 27, on: true }];
    let out = rig.run(&input, &events);
    assert!(out[SPB - 200] > 0.99, "switched before the beat");
    assert!(out[SPB + 2000] < 1e-3, "did not switch on the beat");
}

#[test]
fn link_selects_matching_volume_slot() {
    let mut rig = Rig::new();
    rig.bank.time[6].settings.link = true;
    rig.bank.volume[6].envelope = Envelope::flat(0.0);
    rig.settings.time_slot = 6;
    let out = rig.run(&vec![1.0; SPB], &[]);
    assert!(out[SPB - 1] < 1e-3);
    assert_eq!(rig.engine.status().volume_slot, 6);
}

#[test]
fn one_shot_returns_to_slot_one() {
    let mut rig = Rig::new();
    rig.bank.volume[1].envelope = Envelope::flat(0.0);
    rig.bank.volume[1].settings.one_shot = true;
    rig.bank.volume[1].settings.position_sync = Sync::Off;
    let input = vec![1.0; SPB * 6];
    let events = [NoteEvent { timing: 0, note: 25, on: true }];
    let out = rig.run(&input, &events);
    assert!(out[SPB * 2] < 1e-3, "one-shot slot not playing");
    assert!(out[SPB * 5] > 0.99, "did not return to slot 1");
}

#[test]
fn hold_loops_the_last_bar() {
    let mut rig = Rig::new();
    let bar = SPB * 4;
    let input = ramp(bar * 3);
    // Two bars normal, then hold for the third.
    let first = rig.run(&input[..bar * 2], &[]);
    rig.settings.hold = true;
    let held = rig.run(&input[bar * 2..], &[]);
    assert!(rig.engine.status().holding);
    // While holding, the output repeats the previous bar (shifted by the latency).
    for i in 100..bar - 100 {
        let want = first[bar + i];
        assert!((held[i] - want).abs() < 1e-4, "at {i}: {} vs {want}", held[i]);
    }
}

#[test]
fn force_hold_next_records_one_more_bar() {
    let mut rig = Rig::new();
    let bar = SPB * 4;
    rig.bank.time[2].settings.force_hold = ForceHold::Next;
    let input = ramp(bar * 4);
    rig.run(&input[..bar], &[]);
    rig.settings.time_slot = 2;
    rig.bank.time[2].envelope = Envelope::flat(1.0);
    let next = rig.run(&input[bar..bar * 2], &[]);
    let looped = rig.run(&input[bar * 2..bar * 3], &[]);
    for i in 100..bar - 100 {
        assert!((looped[i] - next[i]).abs() < 1e-4, "at {i}");
    }
}

#[test]
fn click_reduction_crossfades_jumps() {
    let jump = Envelope::from_points(vec![
        Point::new(0.0, 1.0),
        Point::new(2.0, 0.75).with_shape(Shape::Hold),
        Point::new(4.0, 0.75),
    ]);
    let input = sine(SPB * 8, 211.0);
    let mut max_step = [0.0f32; 2];
    for (k, mode) in [ClickReduction::Off, ClickReduction::High].into_iter().enumerate() {
        let mut rig = Rig::new();
        rig.settings.click_reduction = mode;
        rig.set_time(0, jump.clone());
        let out = rig.run(&input, &[]);
        let at = SPB * 4 + SPB * 2;
        max_step[k] = out[at - 10..at + 10].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    }
    assert!(max_step[1] < max_step[0], "{max_step:?}");
}

#[test]
fn stopped_transport_freezes_envelope_position() {
    let mut rig = Rig::new();
    rig.playing = false;
    rig.set_time(0, Envelope::from_points(vec![Point::new(0.0, 1.0), Point::new(4.0, 0.0)]));
    let input = ramp(SPB * 2);
    let out = rig.run(&input, &[]);
    // Envelope stays at x = 0 (live): pure latency delay.
    for i in GUARD..input.len() {
        assert!((out[i] - input[i - GUARD]).abs() < 1e-5);
    }
}

#[test]
fn hq_matches_cubic_on_plain_delay() {
    let mut rig = Rig::new();
    rig.settings.hq = true;
    rig.set_time(0, Envelope::flat(1.0 - 0.37 / 8.0));
    let input = sine(SPB * 2, 300.0);
    let out = rig.run(&input, &[]);
    let d = GUARD as f64 + 0.37 * SPB as f64;
    for i in (SPB..SPB * 2).step_by(97) {
        let t = (i as f64 - d) / SR as f64;
        let want = (2.0 * std::f64::consts::PI * 300.0 * t).sin() as f32;
        assert!((out[i] - want).abs() < 2e-3, "at {i}: {} vs {want}", out[i]);
    }
}
