//! Writes the frame and the ShaperBox section to the PNG named by the argument, and prints the
//! time of a full-frame paint (the first one, which opens the fonts and rasterises the glyphs,
//! and the mean of the next 200).

use backspin_gfx::Dib;
use backspin_ui::{
    HEIGHT, Model, PANEL, QfcText, SB_HEIGHT, SbModel, ShaperCell, WIDTH, paint, paint_sb,
};
use std::time::Instant;

fn main() {
    let out = std::env::args().nth(1).expect("output path");
    let mut text = QfcText::new();
    let model = Model::default();
    let mut dib = Dib::new(WIDTH, HEIGHT + SB_HEIGHT, PANEL);
    let t = Instant::now();
    paint(&mut dib, &model, &mut text);
    let first = t.elapsed();
    let n = 200;
    let t = Instant::now();
    for _ in 0..n {
        let mut d = Dib::new(WIDTH, HEIGHT + SB_HEIGHT, PANEL);
        paint(&mut d, &model, &mut text);
    }
    println!(
        "paint: first {:.2} ms, then {:.3} ms per frame",
        first.as_secs_f64() * 1e3,
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );
    let names = [
        "time",
        "crush",
        "filter",
        "pan",
        "reverb",
        "volume",
        "delay",
        "width",
        "noise",
        "drive",
        "dynamics",
        "liquid",
        "oscilloscope",
        "shift",
        "limiter",
        "pitch",
    ];
    let table: Vec<f32> = (0..8192)
        .map(|i| (i as f32 / 8192.0 * std::f32::consts::TAU).sin() * 0.8)
        .collect();
    let m = SbModel {
        enabled: true,
        shapers: names
            .iter()
            .enumerate()
            .map(|(i, n)| ShaperCell {
                name: n.to_string(),
                visible: i % 3 != 1,
                on: i % 2 == 0,
                ported: true,
            })
            .collect(),
        selected: 5,
        band: 1,
        bands_active: [false, true, false],
        table,
        points: vec![(0.25, 0.8), (0.75, -0.8)],
        ..Default::default()
    };
    dib.move_origin(0, HEIGHT);
    paint_sb(&mut dib, &m, &mut text);
    dib.move_origin(0, -HEIGHT);
    let (w, h) = (dib.width() as u32, dib.height() as u32);
    let rgb: Vec<u8> = dib
        .pixels()
        .iter()
        .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
        .collect();
    let f = std::fs::File::create(out).unwrap();
    let mut e = png::Encoder::new(f, w, h);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
