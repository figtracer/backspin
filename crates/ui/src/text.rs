//! [`QfcText`]: the [`Text`] renderer over the QuickFontCache port (`backspin_qfc`), with the
//! fonts of the Gross Beat form (gfx.md section 5.6). Image-Line's fonts load at runtime from the
//! local FL Studio install and system fonts from the OS font folders; a font that is missing
//! falls back to the first available fallback font, and without any font the text is skipped.

use crate::{Font, Text};
use backspin_gfx::{Dib, Rect};
use backspin_presets::paths;
use backspin_qfc::{self as qfc, Desc, FALLBACK_FONTS, Item, Request, Source, Target};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Where a font file lives.
#[derive(Clone, Copy)]
enum Dir {
    /// FL Studio's `Shared/Artwork/Fonts`.
    Fl,
    /// The system font folders.
    System,
}

/// A `TQuickFont` of the form: the file, the descriptor and the spacings the request carries.
struct Spec {
    dir: Dir,
    file: &'static str,
    desc: Desc,
    xspacing: f64,
    max_xspacing: f64,
}

fn spec(font: Font) -> Spec {
    let d = Desc::default();
    let (dir, file, desc, xspacing, max_xspacing) = match font {
        Font::SlotCell => (
            Dir::System,
            "Arial.ttf",
            Desc {
                height: 9.0,
                aspect: 0.9,
                weight: 0.7,
                ..d
            },
            0.0,
            1.0,
        ),
        Font::Snap => (
            Dir::System,
            "Arial.ttf",
            Desc {
                height: 11.0,
                aspect: 0.8,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::Hold => (
            Dir::System,
            "Arial.ttf",
            Desc {
                height: 16.0,
                aspect: 0.9,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::MapType => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc { height: 13.0, ..d },
            0.0,
            8.0,
        ),
        Font::ButtonGlyph => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc { height: 12.0, ..d },
            0.0,
            8.0,
        ),
        Font::FreezeGlyph => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc {
                height: 12.0,
                aspect: 0.96,
                filter: 2,
                hinting: 3,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::MenuGlyph => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc {
                height: 12.0,
                weight: 0.1,
                ..d
            },
            0.1,
            8.0,
        ),
        Font::KeyHeldGlyph => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc {
                height: 12.0,
                aspect: 0.96,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::OneShotGlyph => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc {
                height: 12.0,
                aspect: 0.96,
                italic: 0.2,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::IlGlyphs => (
            Dir::Fl,
            "ILGlyphsEx.ilfont",
            Desc {
                height: 16.0,
                filter: 2,
                hinting: 3,
                flags: 0x20,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::Caption => (
            Dir::Fl,
            "OpenSans-Regular.ttf",
            Desc {
                height: 15.0,
                aspect: 0.92,
                weight: -0.075,
                italic: (1.0f64 / 3.0) as f32,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::ComboValue => (
            Dir::Fl,
            "OpenSans-Regular.ttf",
            Desc {
                height: 11.0,
                aspect: 0.92,
                weight: -0.075,
                ..d
            },
            0.0,
            8.0,
        ),
        Font::ComboLabel => (
            Dir::Fl,
            "OpenSans-CondLight.ttf",
            Desc {
                height: 11.0,
                weight: 0.5,
                ..d
            },
            0.5,
            8.0,
        ),
        Font::Watermark => (
            Dir::Fl,
            "Ubuntu-R.ttf",
            Desc {
                height: 46.0,
                aspect: 0.9,
                weight: 1.0,
                italic: 0.1,
                ..d
            },
            0.0,
            1.0,
        ),
    };
    Spec {
        dir,
        file,
        desc,
        xspacing,
        max_xspacing,
    }
}

/// Where [`QfcText::locate`] found a font's file.
enum Located {
    File(PathBuf),
    Memory(&'static [u8]),
}

/// An opened font and the spacings of its requests.
struct Entry {
    font: qfc::Font,
    xspacing: f64,
    max_xspacing: f64,
}

/// The QuickFontCache text renderer. Fonts open on first use and keep their rasterised glyphs
/// (per font, size, flags and character) for the life of the renderer, so keep one alive across
/// repaints.
pub struct QfcText {
    fonts: HashMap<Font, Option<Entry>>,
    files: HashMap<PathBuf, Option<Source>>,
    fallbacks: Arc<[(PathBuf, i64)]>,
    fl_fonts: Option<PathBuf>,
    /// FL fonts from memory (a release build's embedded pack), for machines without FL.
    embedded: Vec<(&'static str, &'static [u8])>,
    items: Vec<Item>,
    utf16: Vec<u16>,
}

impl Default for QfcText {
    fn default() -> Self {
        Self::new()
    }
}

impl QfcText {
    /// Finds the font folders and the fallback fonts; no font file is read yet.
    pub fn new() -> Self {
        Self {
            fonts: HashMap::with_capacity(16),
            files: HashMap::new(),
            fallbacks: FALLBACK_FONTS
                .iter()
                .filter_map(|&(name, index)| paths::system_font(name).map(|p| (p, index)))
                .collect(),
            fl_fonts: paths::fonts(),
            embedded: Vec::new(),
            items: Vec::with_capacity(64),
            utf16: Vec::with_capacity(64),
        }
    }

    /// The same, with FL fonts in memory used when the FL install has none: `(file name, bytes)`.
    pub fn with_embedded(embedded: Vec<(&'static str, &'static [u8])>) -> Self {
        Self {
            embedded,
            ..Self::new()
        }
    }

    /// Returns true if every font of the form resolves to its own file (no fallback font).
    pub fn complete(&self) -> bool {
        Font::ALL.iter().all(|&f| self.locate(&spec(f)).is_some())
    }

    /// Where a font's file is: on disk, else (for the FL fonts) in memory.
    fn locate(&self, s: &Spec) -> Option<Located> {
        match s.dir {
            Dir::Fl => self
                .fl_fonts
                .as_ref()
                .map(|d| d.join(s.file))
                .filter(|p| p.is_file())
                .map(Located::File)
                .or_else(|| {
                    self.embedded
                        .iter()
                        .find(|(name, _)| *name == s.file)
                        .map(|&(_, data)| Located::Memory(data))
                }),
            Dir::System => paths::system_font(s.file).map(Located::File),
        }
    }

    fn source(&mut self, path: PathBuf, index: i64) -> Option<Source> {
        self.files
            .entry(path)
            .or_insert_with_key(|p| Source::read(p, index))
            .clone()
    }

    fn entry(&mut self, font: Font) -> Option<&mut Entry> {
        if !self.fonts.contains_key(&font) {
            let s = spec(font);
            let primary = match self.locate(&s) {
                Some(Located::File(p)) => self.source(p, 0),
                Some(Located::Memory(data)) => Some(Source {
                    data: data.into(),
                    index: 0,
                }),
                None => None,
            };
            let fallback = self.fallbacks.first().cloned();
            let fallbacks = self.fallbacks.clone();
            // qfc:0x62c30: a face that does not load falls back to the first registered font.
            let opened = primary
                .and_then(|src| qfc::Font::new(&src, s.desc, fallbacks.clone()))
                .or_else(|| {
                    let (p, i) = fallback?;
                    let src = self.source(p, i)?;
                    qfc::Font::new(&src, s.desc, fallbacks)
                });
            let entry = opened.map(|font| Entry {
                font,
                xspacing: s.xspacing,
                max_xspacing: s.max_xspacing,
            });
            self.fonts.insert(font, entry);
        }
        self.fonts.get_mut(&font).and_then(Option::as_mut)
    }
}

impl Text for QfcText {
    fn draw(
        &mut self,
        dib: &mut Dib,
        font: Font,
        text: &str,
        r: Rect,
        flags: u32,
        colour: u32,
        alpha: i32,
    ) {
        let rect = [r.x0 as f64, r.y0 as f64, r.x1 as f64, r.y1 as f64];
        self.draw_f(dib, font, text, rect, flags, colour, alpha);
    }

    /// `FUN_001819f0`: the window is the clip of `dib`; the rect moves by the clip origin (the
    /// fonts' `YOffset` is 0).
    fn draw_f(
        &mut self,
        dib: &mut Dib,
        font: Font,
        text: &str,
        rect: [f64; 4],
        flags: u32,
        colour: u32,
        alpha: i32,
    ) {
        if alpha <= 0 && flags & 0x80000 == 0 {
            return;
        }
        let c = dib.clip();
        let (ox, oy) = dib.origin();
        let (x0, y0) = (c.x0.max(-ox), c.y0.max(-oy));
        let (x1, y1) = (c.x1.min(dib.width() - ox), c.y1.min(dib.height() - oy));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut utf16 = std::mem::take(&mut self.utf16);
        let mut items = std::mem::take(&mut self.items);
        utf16.clear();
        utf16.extend(text.encode_utf16());
        let stride = dib.width() as usize;
        if let Some(e) = self.entry(font) {
            let req = Request {
                rect: [
                    rect[0] - x0 as f64,
                    rect[1] - y0 as f64,
                    rect[2] - x0 as f64,
                    rect[3] - y0 as f64,
                ],
                text: &utf16,
                flags,
                colour: colour & 0xffffff,
                alpha,
                xspacing: e.xspacing,
                yspacing: 1.0,
                max_xspacing: e.max_xspacing,
            };
            let mut t = Target {
                px: dib.pixels_mut(),
                stride,
                x: x0 + ox,
                y: y0 + oy,
                w: x1 - x0,
                h: y1 - y0,
            };
            qfc::draw(&mut e.font, &mut items, &mut t, &req);
        }
        self.utf16 = utf16;
        self.items = items;
    }
}

// The plugin keeps the renderer in its editor state, which moves between threads.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<QfcText>();
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Without an FL install, the FL fonts come from the embedded copies.
    #[test]
    fn embedded_fonts_stand_in_for_the_fl_install() {
        let Some(dir) = paths::fonts() else { return };
        let embedded: Vec<(&'static str, &'static [u8])> = backspin_presets::EDITOR_FONTS
            .iter()
            .filter_map(|&name| Some((name, &*std::fs::read(dir.join(name)).ok()?.leak())))
            .collect();
        let without = QfcText {
            fl_fonts: None,
            ..QfcText::new()
        };
        let with = QfcText {
            fl_fonts: None,
            ..QfcText::with_embedded(embedded)
        };
        assert!(!without.complete());
        assert!(with.complete());
    }
}
