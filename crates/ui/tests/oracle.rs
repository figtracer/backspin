//! The whole frame checked against the real Gross Beat: harness captures of three Repeater
//! states, kept outside the repository in `.amp/in/gfx/data/oracle/`, compared pixel for pixel
//! without masks. The model is the Repeater preset as `render_frame.py` builds it. The test
//! skips when the captures or the fonts (FL Studio's and Arial) are absent; the exact match needs
//! the macOS libm.

use backspin_envelope::{Point, Shape};
use backspin_gfx::Dib;
use backspin_ui::{HEIGHT, Model, PANEL, QfcText, WIDTH, paint};
use std::path::PathBuf;

/// The plugin view starts at this row of the capture.
const PLUGIN_Y: usize = 25;

const TIME_SLOTS: [&str; 36] = [
    "Empty",
    "Repeat a",
    "Repeat b",
    "Repeat c",
    "Repeat d",
    "Repeat e",
    "Repeat f",
    "Repeat g",
    "3 Kick a",
    "3 Kick b",
    "3 Snare a",
    "3 Snare b",
    "3 step a",
    "3 step b",
    "3 step c",
    "3 step d",
    "Break a",
    "Break b",
    "Break c",
    "break d",
    "Half time a",
    "Half time b",
    "Half time c",
    "Half time d",
    "Triple kick",
    "Triple snare",
    "Skip ahead",
    "Skip fast",
    "Snare roll a",
    "Snare roll b",
    "Snare roll c",
    "Snare roll d",
    "Grain kick",
    "Grain snare",
    "Grain hat",
    "Grain all",
];
const VOL_SLOTS: [&str; 36] = [
    "Empty",
    "Gate a",
    "Gate b",
    "Gate c",
    "Gate d",
    "Gate e",
    "Gate f",
    "Gate g",
    "Gate h",
    "Gate i",
    "Gate j",
    "Gate k",
    "Fast gate a",
    "Fast gate b",
    "Fast gate c",
    "Fast gate d",
    "End gate a",
    "End gate b",
    "End gate c",
    "End gate d",
    "Saw gate a",
    "Saw gate b",
    "Saw gate c",
    "Saw gate d",
    "Break a",
    "Break b",
    "Break saw a",
    "Break saw b",
    "Kick gate",
    "Kick off",
    "Stop start",
    "Crabbing",
    "Sidechain a",
    "Sidechain b",
    "Sine",
    "Triple gate",
];

struct Oracle {
    w: usize,
    rgb: Vec<u8>,
}

impl Oracle {
    fn load(name: &str) -> Option<Self> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.amp/in/gfx/data/oracle")
            .join(name);
        let f = std::fs::File::open(p).ok()?;
        let mut d = png::Decoder::new(f);
        d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut r = d.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        let ch = info.color_type.samples();
        let rgb = buf.chunks(ch).flat_map(|p| [p[0], p[1], p[2]]).collect();
        Some(Self {
            w: info.width as usize,
            rgb,
        })
    }

    fn at(&self, x: i32, y: i32) -> u32 {
        let i = ((y as usize + PLUGIN_Y) * self.w + x as usize) * 3;
        (self.rgb[i] as u32) << 16 | (self.rgb[i + 1] as u32) << 8 | self.rgb[i + 2] as u32
    }
}

/// The Repeater state with the volume group edited and parameter overrides.
fn repeater(overrides: &[(usize, i32)]) -> Model {
    let mut params = [0, 0, 256, 256, 657, 653, 0, 0, 65536];
    for &(i, v) in overrides {
        params[i] = v;
    }
    Model {
        names: [
            TIME_SLOTS.map(String::from).to_vec(),
            VOL_SLOTS.map(String::from).to_vec(),
        ],
        empty: [vec![false; 36], vec![false; 36]],
        group: 1,
        snap: 3,
        params,
        env_points: vec![
            Point::new(0.0, 1.0, 0.0, Shape::HOLD),
            Point::new(4.0, 1.0, 0.0, Shape::HOLD),
        ],
        watermark: "Gross Beat".into(),
        ..Default::default()
    }
}

/// Renders a model and returns the differing pixels against a capture (`None` if absent).
fn check(name: &str, m: &Model, text: &mut QfcText) -> Option<Vec<String>> {
    let o = Oracle::load(name)?;
    let mut dib = Dib::new(WIDTH, HEIGHT, PANEL);
    paint(&mut dib, m, text);
    let mut bad = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (want, got) = (o.at(x, y), dib.pixel(x, y) & 0xffffff);
            if want != got {
                bad.push(format!("({x},{y}) oracle {want:06x} port {got:06x}"));
            }
        }
    }
    Some(bad)
}

#[test]
fn frames_match() {
    let mut text = QfcText::new();
    if !text.complete() {
        eprintln!("FL Studio fonts or Arial not present; skipped");
        return;
    }
    let frames = [
        ("base.png", repeater(&[])),
        (
            "Repeater_p2=128_p4=9000_p6=-200.png",
            repeater(&[(2, 128), (4, 9000), (6, -200)]),
        ),
        (
            "Repeater_p3=64_p5=1000_p6=256_p7=1.png",
            repeater(&[(3, 64), (5, 1000), (6, 256), (7, 1)]),
        ),
    ];
    let mut report = Vec::new();
    for (name, m) in &frames {
        match check(name, m, &mut text) {
            None => eprintln!("{name} not present; skipped"),
            Some(bad) if !bad.is_empty() => report.push(format!(
                "{name}: {} pixels differ: {:?}",
                bad.len(),
                &bad[..bad.len().min(8)]
            )),
            Some(_) => {}
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}
