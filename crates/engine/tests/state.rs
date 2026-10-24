//! Gate 1: the state loader against the nine decoded factory presets
//! (`.amp/in/re/data/presets/*.json`), and the save/load round trip.
//!
//! The tests read the presets from the FL Studio install and skip when it is missing.

use backspin_engine::{Engine, fst_state};
use serde_json::Value;
use std::path::{Path, PathBuf};

const PRESET_DIR: &str = "/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat";
const PRESETS: [&str; 9] = [
    "Default",
    "Flanging",
    "Juggling Science",
    "Momentary",
    "Patterns",
    "Pitch shifter",
    "Repeater",
    "Stutter",
    "Turntablist",
];

fn json_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.amp/in/re/data/presets")
}

fn load(name: &str) -> Option<(Engine, Vec<u8>, Value)> {
    let fst = std::fs::read(Path::new(PRESET_DIR).join(format!("{name}.fst"))).ok()?;
    let json = std::fs::read(json_dir().join(format!("{name}.json"))).ok()?;
    let blob = fst_state(&fst).unwrap().to_vec();
    let mut e = Engine::new();
    let used = e.load_state(&blob);
    assert_eq!(used, blob.len(), "{name}: consumed bytes");
    Some((e, blob, serde_json::from_slice(&json).unwrap()))
}

#[test]
fn loader_matches_decoded_presets() {
    let mut checked = 0;
    for name in PRESETS {
        let Some((e, _, j)) = load(name) else {
            continue;
        };
        let st = &j["state"];
        for (i, p) in st["params"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                e.params[i] as i64,
                p["raw"].as_i64().unwrap(),
                "{name}: param {i}"
            );
        }
        let o = &st["options"];
        assert_eq!(
            e.view_bank as u64,
            o["edit_view_bank"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            e.remove_dc != 0,
            o["remove_dc"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            e.attack_comp != 0,
            o["attack_compensation"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(e.hq != 0, o["hq_resampling"].as_bool().unwrap(), "{name}");
        assert_eq!(
            e.view_scratch != 0,
            o["view_scratch_clock"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            e.click as u64,
            o["click_reduction"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            e.midi_trigger != 0,
            st["midi_trigger"].as_bool().unwrap(),
            "{name}"
        );
        for (b, bank) in ["time", "volume"].iter().enumerate() {
            for (i, js) in st["banks"][bank].as_array().unwrap().iter().enumerate() {
                let s = &e.banks[b].slots[i];
                let tag = format!("{name} {bank}[{i}]");
                assert_eq!(s.name, js["name"].as_str().unwrap(), "{tag}");
                assert_eq!(
                    s.trigger_sync as i64,
                    js["trigger_sync"].as_i64().unwrap(),
                    "{tag}"
                );
                assert_eq!(
                    s.position_sync as i64,
                    js["position_sync"].as_i64().unwrap(),
                    "{tag}"
                );
                assert_eq!(
                    s.force_hold as i64,
                    js["force_hold"].as_i64().unwrap(),
                    "{tag}"
                );
                assert_eq!(s.link as i64, js["link"].as_i64().unwrap(), "{tag}");
                assert_eq!(s.key_held as i64, js["key_held"].as_i64().unwrap(), "{tag}");
                assert_eq!(s.one_shot as i64, js["one_shot"].as_i64().unwrap(), "{tag}");
                let env = &js["envelope"];
                assert_eq!(s.empty, env["is_empty"].as_bool().unwrap(), "{tag}");
                let h = env["header_raw"].as_array().unwrap();
                assert_eq!(s.env.flags as i64, h[0].as_i64().unwrap(), "{tag}");
                assert_eq!(s.env.marker as i64, h[1].as_i64().unwrap(), "{tag}");
                check_points(&tag, &s.env, env["points"].as_array().unwrap());
            }
        }
        checked += 1;
    }
    eprintln!("checked {checked} presets");
}

/// Compares loaded points with the decoded record, after the length fix of the original: a
/// first point past 0 gets a copy at index 1 with dx 0, and every dx is scaled to 4 beats.
fn check_points(tag: &str, env: &backspin_engine::envelope::Envelope, pts: &[Value]) {
    let mut want: Vec<(f64, f32, f32, u8, u8)> = pts
        .iter()
        .map(|p| {
            (
                p["x"].as_f64().unwrap(),
                p["y"].as_f64().unwrap() as f32,
                p["tension"].as_f64().unwrap() as f32,
                p["shape"].as_u64().unwrap() as u8,
                p["b12"].as_u64().unwrap() as u8,
            )
        })
        .collect();
    if want[0].0 > 0.0 {
        want.insert(1, (0.0, want[0].1, 0.0, 0, 0));
    }
    let total: f64 = want.iter().map(|p| p.0).sum();
    if total != 4.0 {
        let k = 4.0 / total;
        for p in &mut want {
            p.0 *= k;
        }
    }
    assert_eq!(env.points.len(), want.len(), "{tag}: point count");
    for (i, (p, w)) in env.points.iter().zip(&want).enumerate() {
        assert_eq!(p.dx.to_bits(), w.0.to_bits(), "{tag} point {i}: dx");
        assert_eq!(p.value.to_bits(), w.1.to_bits(), "{tag} point {i}: value");
        assert_eq!(
            p.tension.to_bits(),
            w.2.to_bits(),
            "{tag} point {i}: tension"
        );
        assert_eq!(p.shape, w.3, "{tag} point {i}: shape");
        assert_eq!(p.flags, w.4, "{tag} point {i}: flags");
        assert_eq!(p.arp, 0, "{tag} point {i}: arp");
    }
}

/// Save then load gives the same state, up to the length fix that every load runs: an envelope
/// whose first point is past 0 gets one more copy of that point, and a total that is not exactly
/// 4 beats is scaled again (Momentary and Patterns volume slot 29).
#[test]
fn save_then_load_round_trips() {
    for name in PRESETS {
        let Some((e, _, _)) = load(name) else {
            continue;
        };
        let blob = e.save_state();
        let mut f = Engine::new();
        assert_eq!(f.load_state(&blob), blob.len(), "{name}");
        let mut want = e.clone();
        for b in &mut want.banks {
            for s in &mut b.slots {
                s.env.fix_length();
                s.env.prepare();
            }
        }
        assert_eq!(f.params, want.params, "{name}");
        for b in 0..2 {
            assert_eq!(f.banks[b].slots, want.banks[b].slots, "{name}: bank {b}");
        }
        assert_eq!(
            f.save_state(),
            want.save_state(),
            "{name}: second save differs"
        );
    }
}

#[test]
fn fresh_instance_defaults() {
    let e = Engine::new();
    assert_eq!(e.params, [0, 0, 256, 256, 1765, 5000, 0, 0, 65536]);
    assert_eq!(
        (
            e.view_bank,
            e.remove_dc,
            e.attack_comp,
            e.hq,
            e.view_scratch,
            e.click
        ),
        (0, 0, 0, 1, 1, 1)
    );
    assert_eq!(e.midi_trigger, 0);
    for b in &e.banks {
        for s in &b.slots {
            assert!(s.name.is_empty() && s.empty);
            assert_eq!((s.trigger_sync, s.position_sync, s.force_hold), (5, 5, 0));
            assert_eq!(s.env.points.len(), 2);
        }
    }
}

#[test]
fn newer_state_is_ignored() {
    let mut e = Engine::new();
    let mut blob = Engine::new().save_state();
    blob[..4].copy_from_slice(&0x0009_0004u32.to_le_bytes());
    blob[4..8].copy_from_slice(&7i32.to_le_bytes());
    e.load_state(&blob);
    assert_eq!(e.params[0], 0);
}
