//! The frame checked against the real Gross Beat (a harness capture of the default state with the
//! volume group edited, kept outside the repository in `.amp/in/gfx/data/oracle/base.png`; the
//! test skips when it is absent). Text is masked: the rects handed to the text renderer and the
//! controls not ported yet are skipped.

use backspin_gfx::{Dib, Rect};
use backspin_ui::{Font, HEIGHT, Model, PANEL, Text, WIDTH, paint};
use std::path::PathBuf;

/// The plugin view starts at this row of the capture.
const PLUGIN_Y: usize = 25;

struct Oracle {
    w: usize,
    rgb: Vec<u8>,
}

impl Oracle {
    fn load() -> Option<Self> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.amp/in/gfx/data/oracle/base.png");
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

/// Records the text rects in form coordinates.
#[derive(Default)]
struct Mask(Vec<Rect>);

impl Text for Mask {
    fn draw(&mut self, dib: &mut Dib, _: Font, _: &str, r: Rect, _: u32, _: u32, _: i32) {
        // Glyphs may overhang their rect by a pixel or two.
        let (ox, oy) = dib.origin();
        let r = Rect::new(r.x0 - 2, r.y0, r.x1 + 2, r.y1);
        self.0
            .push(r.offset(ox, oy).intersect(dib.clip().offset(ox, oy)));
    }
}

fn model() -> Model {
    Model {
        names: [vec!["x".into(); 36], vec!["x".into(); 36]],
        group: 1,
        ..Default::default()
    }
}

/// Checks one region and returns the count of differing pixels and the first few.
fn compare(
    dib: &Dib,
    o: &Oracle,
    region: Rect,
    skip: &dyn Fn(i32, i32) -> bool,
) -> (usize, Vec<String>) {
    let mut bad = 0;
    let mut first = Vec::new();
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            if skip(x, y) {
                continue;
            }
            let (want, got) = (o.at(x, y), dib.pixel(x, y) & 0xffffff);
            if want != got {
                bad += 1;
                if first.len() < 8 {
                    first.push(format!("({x},{y}) oracle {want:06x} port {got:06x}"));
                }
            }
        }
    }
    (bad, first)
}

#[test]
fn frame_matches() {
    let Some(o) = Oracle::load() else {
        eprintln!("oracle capture not present; skipped");
        return;
    };
    let mut dib = Dib::new(WIDTH, HEIGHT, PANEL);
    let mut mask = Mask::default();
    paint(&mut dib, &model(), &mut mask);
    let rects = mask.0;
    // Controls not ported yet: the knobs, HOLD, Options, SlotZoom, the clock, the name label.
    let pending = [
        Rect::new(7, 5, 55, 53),
        Rect::new(7, 66, 27, 86),
        Rect::new(109, 60, 172, 90),
        Rect::new(186, 11, 258, 83),
        Rect::new(7, 472, 266, 535),
        Rect::new(0, 410, 266, 472),
    ];
    let skip = |x: i32, y: i32| {
        rects
            .iter()
            .chain(&pending)
            .any(|r| (r.x0..r.x1).contains(&x) && (r.y0..r.y1).contains(&y))
    };
    let regions = [
        ("top", Rect::new(0, 0, 266, 91)),
        ("grids", Rect::new(0, 91, 266, 410)),
        ("side", Rect::new(266, 0, 297, HEIGHT)),
        ("bottom tabs", Rect::new(455, 510, 830, 535)),
    ];
    let mut report = Vec::new();
    for (name, r) in regions {
        let (bad, first) = compare(&dib, &o, r, &skip);
        if bad > 0 {
            report.push(format!("{name}: {bad} pixels differ: {first:?}"));
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}
