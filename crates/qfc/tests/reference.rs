//! The port against the reference data of `.amp/in/gfx/data/qfc_ref.json` (written by
//! `qfc_ref.py`, kept outside the repository): glyph records of the editor fonts from the
//! Python model and random text calls drawn by FL's real QuickFontCache. The tests skip when
//! the data or the fonts are absent. Bit-exactness needs the macOS libm (the stroker and the
//! LUTs call `atan`, `cos`, `sin` and `pow`).

use backspin_qfc::{Desc, FALLBACK_FONTS, Font, Request, Source, Target, draw};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn reference() -> Option<Value> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.amp/in/gfx/data/qfc_ref.json");
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

fn font_path(file: &str) -> Option<PathBuf> {
    backspin_presets::paths::system_font(file)
        .or_else(|| backspin_presets::paths::fonts().map(|d| d.join(file)))
        .filter(|p| p.is_file())
}

fn fallbacks() -> Arc<[(PathBuf, i64)]> {
    FALLBACK_FONTS
        .iter()
        .filter_map(|&(n, i)| backspin_presets::paths::system_font(n).map(|p| (p, i)))
        .collect()
}

/// The fonts of the reference, opened once per descriptor.
#[derive(Default)]
struct Fonts(HashMap<String, Option<(Font, [f64; 3])>>);

impl Fonts {
    fn get(&mut self, f: &Value) -> Option<&mut (Font, [f64; 3])> {
        let key = f.to_string();
        self.0
            .entry(key)
            .or_insert_with(|| {
                let num = |k: &str| f[k].as_f64().unwrap();
                let desc = Desc {
                    height: num("height") as f32,
                    aspect: num("aspect") as f32,
                    weight: num("weight") as f32,
                    weight_aspect: num("weight_aspect") as f32,
                    italic: num("italic") as f32,
                    sharpness: num("sharpness") as f32,
                    filter: num("filter") as u16,
                    hinting: num("hinting") as u16,
                    flags: num("flags") as u32,
                };
                let src = Source::read(&font_path(f["file"].as_str()?)?, 0)?;
                let font = Font::new(&src, desc, fallbacks())?;
                Some((
                    font,
                    [num("xspacing"), num("yspacing"), num("max_xspacing")],
                ))
            })
            .as_mut()
    }
}

#[test]
fn glyph_records_match_the_model() {
    let Some(r) = reference() else {
        eprintln!("qfc_ref.json not present; skipped");
        return;
    };
    let mut fonts = Fonts::default();
    let (mut checked, mut bad) = (0, Vec::new());
    for g in r["glyphs"].as_array().unwrap() {
        let Some((font, _)) = fonts.get(&g["font"]) else {
            continue;
        };
        let code = g["code"].as_u64().unwrap() as u32;
        let got = font.glyph(code);
        let rows: Vec<String> = match &got.bits {
            Some(b) => (0..got.h as usize)
                .map(|y| {
                    b[y * got.stride + 3..y * got.stride + 3 + got.w as usize]
                        .iter()
                        .map(|v| format!("{v:02x}"))
                        .collect()
                })
                .collect(),
            None => Vec::new(),
        };
        let want_rows: Vec<String> = g["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let (w, h) = if got.bits.is_some() {
            (got.w, got.h)
        } else {
            (0, 0)
        };
        checked += 1;
        if w as i64 != g["w"].as_i64().unwrap()
            || h as i64 != g["h"].as_i64().unwrap()
            || got.bearing as f64 != g["bearing"].as_f64().unwrap()
            || got.top as i64 != g["top"].as_i64().unwrap()
            || got.adv != g["adv"].as_f64().unwrap()
            || rows != want_rows
        {
            bad.push(format!(
                "{} U+{code:04X}: port {w}x{h} b{} t{} a{}, model {}x{} b{} t{} a{}",
                g["font"]["file"],
                got.bearing,
                got.top,
                got.adv,
                g["w"],
                g["h"],
                g["bearing"],
                g["top"],
                g["adv"]
            ));
        }
    }
    eprintln!("{checked} glyphs checked");
    assert!(
        bad.is_empty(),
        "{} of {checked} glyphs differ:\n{}",
        bad.len(),
        bad[..bad.len().min(10)].join("\n")
    );
}

#[test]
fn text_calls_match_the_library() {
    let Some(r) = reference() else {
        eprintln!("qfc_ref.json not present; skipped");
        return;
    };
    let mut fonts = Fonts::default();
    let mut items = Vec::new();
    let (mut checked, mut bad) = (0, Vec::new());
    for c in r["texts"].as_array().unwrap() {
        let Some((font, sp)) = fonts.get(&c["font"]) else {
            continue;
        };
        let (w, h) = (
            c["w"].as_i64().unwrap() as i32,
            c["h"].as_i64().unwrap() as i32,
        );
        let bg = c["bg"].as_u64().unwrap() as u32;
        let mut px = vec![bg; (w * h) as usize];
        let mut want = px.clone();
        for p in c["pixels"].as_array().unwrap() {
            want[p[0].as_u64().unwrap() as usize] = p[1].as_u64().unwrap() as u32;
        }
        let rect: Vec<f64> = c["rect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let text: Vec<u16> = c["text"].as_str().unwrap().encode_utf16().collect();
        let req = Request {
            rect: [rect[0], rect[1], rect[2], rect[3]],
            text: &text,
            flags: c["flags"].as_u64().unwrap() as u32,
            colour: c["colour"].as_u64().unwrap() as u32,
            alpha: c["alpha"].as_i64().unwrap() as i32,
            xspacing: sp[0],
            yspacing: sp[1],
            max_xspacing: sp[2],
        };
        let mut t = Target {
            px: &mut px,
            stride: w as usize,
            x: 0,
            y: 0,
            w,
            h,
        };
        draw(font, &mut items, &mut t, &req);
        checked += 1;
        let diff = px.iter().zip(&want).filter(|(a, b)| a != b).count();
        if diff > 0 {
            bad.push(format!(
                "{} h{} {:?} flags {:#x} rect {rect:?}: {diff} pixels differ",
                c["font"]["file"], c["font"]["height"], c["text"], req.flags
            ));
        }
    }
    eprintln!("{checked} text calls checked");
    assert!(
        bad.is_empty(),
        "{} of {checked} calls differ:\n{}",
        bad.len(),
        bad[..bad.len().min(10)].join("\n")
    );
}
