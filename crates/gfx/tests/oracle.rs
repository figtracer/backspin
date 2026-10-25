//! Regions of the editor checked against the real Gross Beat (a harness capture, kept outside the
//! repository in `.amp/in/gfx/data/oracle/base.png`; the tests skip when it is absent).

use backspin_gfx::{Dib, DrawPath, PI, Rect, gradient_lut, s};
use std::path::PathBuf;

/// The plugin view starts at this row of the capture.
const PLUGIN_Y: usize = 25;
const PANEL: u32 = 0xff344b5b;

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

    /// The oracle pixel at form coordinates, `0x00RRGGBB`.
    fn at(&self, x: i32, y: i32) -> u32 {
        let i = ((y as usize + PLUGIN_Y) * self.w + x as usize) * 3;
        (self.rgb[i] as u32) << 16 | (self.rgb[i + 1] as u32) << 8 | self.rgb[i + 2] as u32
    }
}

/// Compares `region` of the bitmap with the oracle, skipping pixels for which `skip` is true.
fn compare(
    dib: &Dib,
    o: &Oracle,
    region: Rect,
    skip: impl Fn(i32, i32) -> bool,
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
                if first.len() < 6 {
                    first.push(format!("({x},{y}) oracle {want:06x} port {got:06x}"));
                }
            }
        }
    }
    (bad, first)
}

/// `TopBgPaintBoxPaint` (0x6266c) at scale 1 with the scratch clock visible (gfx.md 8.1).
fn top_bg(dib: &mut Dib) {
    let (w, sc) = (266, 1.0);
    let clock_left = 186;
    dib.fill_rect(Rect::new(0, 0, w, 91), 0xff171f24, 0x100);
    let a = dib.agg();
    a.reset_path();
    a.line_width(1.0);
    a.line_color(0xff323a40);
    a.fill_color(0xff746e66);
    let f = |n: f64| s(n, sc) as f64;
    a.move_to(f(0.0), f(0.0));
    a.line_to(f(0.0), f(61.0));
    a.line_to(f(39.0), f(61.0));
    a.line_to(f(60.0), f(53.0));
    let cx = (clock_left - s(10.0, sc)) as f64;
    a.line_to(cx, f(53.0));
    a.arc_to(f(5.0), f(5.0), PI * 0.5, false, true, f(5.0) + cx, f(58.0));
    a.line_to(f(5.0) + cx, f(85.0));
    a.arc_to(
        f(5.0),
        f(5.0),
        PI * 0.5,
        false,
        false,
        f(10.0) + cx,
        f(90.0),
    );
    a.line_to(f(1.0) + w as f64, f(90.0));
    a.line_to(f(1.0) + w as f64, f(0.0));
    a.line_to(f(0.0), f(0.0));
    a.linear_gradient((w / 2) as f64, 0.0, (w / 2) as f64, f(90.0), 1);
    gradient_lut(a, 72.0, -48.0, 0xff746e66, 1);
    a.draw_path(DrawPath::FillAndStroke);
    a.reset_path();
    a.fill_color(0xff3a352c);
    a.move_to(f(36.0), f(14.0));
    a.line_to(f(118.0), f(14.0));
    a.arc_to(f(14.0), f(14.0), PI, false, true, f(118.0), f(45.0));
    a.line_to(f(36.0), f(45.0));
    a.line_to(f(36.0), f(15.0));
    a.draw_path(DrawPath::Fill);
}

#[test]
fn top_background_matches() {
    let Some(o) = Oracle::load() else {
        eprintln!("oracle capture not present; skipped");
        return;
    };
    let mut dib = Dib::new(830, 535, PANEL);
    top_bg(&mut dib);
    // Text and the controls painted on top are not drawn here.
    let skip = |x: i32, y: i32| {
        (36..127).contains(&x) && (14..45).contains(&y)
            || [
                (7, 5, 48, 48),
                (7, 66, 20, 20),
                (109, 60, 63, 30),
                (186, 11, 72, 72),
            ]
            .iter()
            .any(|&(bx, by, bw, bh)| (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y))
    };
    let (bad, first) = compare(&dib, &o, Rect::new(0, 0, 266, 91), skip);
    assert!(bad == 0, "{bad} pixels differ: {first:?}");
}
