//! Gate 9: the editing tools against the reference models.
//!
//! `data/tools.json` comes from `scripts/envtools_vectors.py`, which extends the frozen
//! `.amp/in/re/data/envtools_model.py` with the tools it lacks. Every point must match bit for
//! bit: position, value, tension, shape and selection.

// The tool table wraps the infallible tools in `Ok`.
#![allow(clippy::unit_arg)]

use backspin_envelope::tools::{
    FilterSettings, ScaleSettings, SeqSettings, SeqStep, SmoothSettings, parse_value,
};
use backspin_envelope::{Envelope, Point};
use serde_json::Value;

fn point(v: &Value) -> Point {
    let a = v.as_array().unwrap();
    let mut p = Point::new(
        f64::from_bits(a[0].as_u64().unwrap()),
        f32::from_bits(a[1].as_u64().unwrap() as u32),
        f32::from_bits(a[2].as_u64().unwrap() as u32),
        a[3].as_u64().unwrap() as u8,
    );
    p.set_selected(a[4].as_u64().unwrap() != 0);
    p
}

fn dump(env: &Envelope) -> Vec<(u64, u32, u32, u8, bool)> {
    env.points
        .iter()
        .map(|p| {
            (
                p.dx.to_bits(),
                p.value.to_bits(),
                p.tension.to_bits(),
                p.shape,
                p.selected(),
            )
        })
        .collect()
}

fn kernel_env() -> Envelope {
    Envelope::with_points(
        1.0,
        vec![
            Point::new(0.0, 0.0, 0.0, 0),
            Point::new(0.5, 1.0, 0.0, 0),
            Point::new(0.5, 0.0, 0.0, 0),
        ],
    )
}

fn i(v: &Value) -> i32 {
    v.as_i64().unwrap() as i32
}

#[test]
fn tools_match_the_reference_models() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("data/tools.json")).unwrap();
    let mut checked = 0;
    for c in &cases {
        let name = c["name"].as_str().unwrap();
        let tool = c["tool"].as_str().unwrap();
        let args = c["args"].as_array().unwrap();
        if tool == "parse" {
            let got = parse_value(args[0].as_str().unwrap(), args[1].as_bool().unwrap());
            assert_eq!(got, c["want"].as_f64(), "{name}");
            checked += 1;
            continue;
        }
        let pts: Vec<Point> = c["points"].as_array().unwrap().iter().map(point).collect();
        let mut env = Envelope::with_points(4.0, pts);
        if let Some(clip) = c["clip"].as_array() {
            env.clip = clip.iter().map(point).collect();
        }
        let result = match tool {
            "flip" => Ok(env.flip()),
            "normalize" => Ok(env.normalize()),
            "scale" => Ok(env.scale_levels(&ScaleSettings {
                center: i(&args[0]),
                multiply: i(&args[1]),
                offset: i(&args[2]),
                tension: i(&args[3]),
            })),
            "decimate" => env.decimate_tool(i(&args[0])),
            "smooth_all" => env.turn_smooth(),
            "semitone" => Ok(env.semitone(
                args[0].as_u64().unwrap() as usize,
                args[1].as_bool().unwrap(),
            )),
            "smooth_up" => env.smooth_up(&SmoothSettings {
                attack: i(&args[0]),
                release: i(&args[1]),
                attack_slope: i(&args[2]),
                release_slope: i(&args[3]),
                offset: i(&args[4]),
                decimate: i(&args[5]),
                link: false,
            }),
            "filter" => env.filter(
                &FilterSettings {
                    width: i(&args[0]),
                    offset: i(&args[1]),
                    decimate: i(&args[2]),
                },
                &kernel_env(),
            ),
            "copy" => Ok(env.copy_points(args[0].as_bool().unwrap())),
            "paste" => env.paste_points(args[0].as_bool().unwrap()),
            "duplicate" => env.duplicate(),
            "delete" => env.delete_selected(),
            "sequence" => {
                let steps: Vec<SeqStep> = args[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| {
                        let s = s.as_array().unwrap();
                        SeqStep {
                            kind: i(&s[0]),
                            brk: i(&s[1]),
                            level: i(&s[2]),
                            decay_slope: i(&s[3]),
                            sustain: i(&s[4]),
                            release_slope: i(&s[5]),
                        }
                    })
                    .collect();
                let st = args[1].as_array().unwrap();
                env.create_sequence(
                    &steps.try_into().unwrap(),
                    &SeqSettings {
                        ping_pong: i(&st[0]) > 0,
                        time_mul: i(&st[1]),
                        swing: i(&st[2]),
                        attack: i(&st[3]),
                        decay: i(&st[4]),
                        sustain: i(&st[5]),
                        gate: i(&st[6]),
                    },
                );
                Ok(())
            }
            "analyze" => {
                let frames: Vec<[f32; 2]> = args[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| {
                        let f = f.as_array().unwrap();
                        [
                            f32::from_bits(f[0].as_u64().unwrap() as u32),
                            f32::from_bits(f[1].as_u64().unwrap() as u32),
                        ]
                    })
                    .collect();
                env.analyze(
                    &frames,
                    args[1].as_u64().unwrap() as usize,
                    args[2].as_f64().unwrap(),
                    args[3].as_f64().unwrap(),
                );
                Ok(())
            }
            other => panic!("unknown tool {other}"),
        };
        let want_refused = c["result"].as_str() == Some("refused");
        assert_eq!(result.is_err(), want_refused, "{name}: refusal");
        let want: Vec<(u64, u32, u32, u8, bool)> = c["want"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let p = point(v);
                (
                    p.dx.to_bits(),
                    p.value.to_bits(),
                    p.tension.to_bits(),
                    p.shape,
                    p.selected(),
                )
            })
            .collect();
        let got = dump(&env);
        if got != want {
            for (k, (g, w)) in got.iter().zip(&want).enumerate() {
                if g != w {
                    panic!(
                        "{name}: point {k} differs: got {:?} ({} {}) want {:?} ({} {}); {} vs {} points",
                        g,
                        f64::from_bits(g.0),
                        f32::from_bits(g.1),
                        w,
                        f64::from_bits(w.0),
                        f32::from_bits(w.1),
                        got.len(),
                        want.len()
                    );
                }
            }
            panic!("{name}: {} points, want {}", got.len(), want.len());
        }
        checked += 1;
    }
    assert_eq!(checked, cases.len());
}
