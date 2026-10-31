//! The Gross Beat editor, drawn with the plugin's own painters (gfx.md sections 7 and 8) into a
//! `TDIB`. [`paint`] renders a whole frame from a [`Model`] in the form's component order; text
//! goes through a [`Text`] renderer (the QuickFontCache port).

mod btn;
mod clock;
mod env;
mod knob;
mod painters;
mod prims;
pub mod sb;
mod text;

pub use env::EnvView;
pub use painters::paint;
pub use sb::{SB_HEIGHT, SbModel, ShaperCell, paint_sb};
pub use text::QfcText;

use backspin_envelope::Point;
use backspin_gfx::{Dib, Rect};

/// The editor size at scale 1 (the plugin view).
pub const WIDTH: i32 = 830;
pub const HEIGHT: i32 = 535;
/// `SynthPanel.Color` (#344b5b).
pub const PANEL: u32 = 0xff344b5b;

/// A font as the painters name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    /// The slot cells: Arial 9, aspect 0.9, weight 0.7, MaxXSpacing 1.
    SlotCell,
    /// SnapSel: Arial 11, aspect 0.8.
    Snap,
    /// MapTypeSel: ILGlyphsEx 13.
    MapType,
    /// The side and bottom toggle glyphs: ILGlyphsEx 12.
    ButtonGlyph,
    /// NameLabel / pill captions: P_Normal at height 15, italic 1/3.
    Caption,
    /// P_ILGlyphs (pill glyphs, SlotZoom).
    IlGlyphs,
    /// VolAttWheelLabel's font: the combo labels, the knob labels and the envelope axis tags
    /// (OpenSans-CondLight 11, weight 0.5, XSpacing 0.5).
    ComboLabel,
    /// The combo values (P_Normal).
    ComboValue,
    /// HoldBtn: Arial 16, aspect 0.9.
    Hold,
    /// The envelope watermark: Ubuntu-R 46, aspect 0.9, weight 1, italic 0.1, MaxXSpacing 1.
    Watermark,
    /// FreezeBtn: ILGlyphsEx 12, aspect 0.96, filter Sharper, hinting Internal.
    FreezeGlyph,
    /// MenuBtn: ILGlyphsEx 12, weight 0.1, XSpacing 0.1.
    MenuGlyph,
    /// KeyHoldBtn: ILGlyphsEx 12, aspect 0.96.
    KeyHeldGlyph,
    /// OneShotBtn: ILGlyphsEx 12, aspect 0.96, italic 0.2.
    OneShotGlyph,
}

impl Font {
    /// Every font of the form.
    pub const ALL: [Font; 14] = [
        Font::SlotCell,
        Font::Snap,
        Font::MapType,
        Font::ButtonGlyph,
        Font::Caption,
        Font::IlGlyphs,
        Font::ComboLabel,
        Font::ComboValue,
        Font::Hold,
        Font::Watermark,
        Font::FreezeGlyph,
        Font::MenuGlyph,
        Font::KeyHeldGlyph,
        Font::OneShotGlyph,
    ];
}

/// Text output: `rect` in origin coordinates of `dib`, flags and alpha as the plugin passes them
/// (`FUN_00181cc0`), colour `0xRRGGBB`.
pub trait Text {
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        dib: &mut Dib,
        font: Font,
        text: &str,
        rect: Rect,
        flags: u32,
        colour: u32,
        alpha: i32,
    );

    /// The same with the rect in doubles (`FUN_001819f0`, used by the button captions). The
    /// default truncates the rect and calls [`Text::draw`].
    #[allow(clippy::too_many_arguments)]
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
        let r = Rect::new(
            rect[0] as i32,
            rect[1] as i32,
            rect[2] as i32,
            rect[3] as i32,
        );
        self.draw(dib, font, text, r, flags, colour, alpha);
    }
}

/// Draws no text.
pub struct NoText;

impl Text for NoText {
    fn draw(&mut self, _: &mut Dib, _: Font, _: &str, _: Rect, _: u32, _: u32, _: i32) {}
}

/// What the painters show.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Slot names (empty = "Slot N") and whether a slot is empty, per group (0 time, 1 volume).
    pub names: [Vec<String>; 2],
    pub empty: [Vec<bool>; 2],
    /// The selector value (the selected slot) and the edited slot per group.
    pub selected: [i32; 2],
    pub edited: [usize; 2],
    /// The playing slot per group (-1 = none) and its progress bar width in pixels.
    pub playing: [i32; 2],
    pub bar_px: [i32; 2],
    /// The edited group (MapTypeSel): 0 time, 1 volume.
    pub group: usize,
    /// SnapSel index 0..6.
    pub snap: usize,
    /// Side toggles: Step, Slide, Freeze.
    pub step: bool,
    pub slide: bool,
    pub freeze: bool,
    /// Bottom toggles of the edited slot: link (time group only), key-held, one-shot.
    pub link: bool,
    pub key_held: bool,
    pub one_shot: bool,
    /// The scratch clock is visible (option "View scratching clock").
    pub clock: bool,
    /// The combo values of the edited slot: HOLD, TRIG SYNC, POS SYNC.
    pub combos: [String; 3],
    /// The 9 raw parameter values: 0 time slot, 1 volume slot, 2 time mix, 3 volume mix,
    /// 4 attack, 5 release, 6 tension, 7 hold, 8 scratch. The knobs show 2..=6, HOLD is down
    /// when 7 is non-zero and the clock hand follows 8 (65536 = rest).
    pub params: [i32; 9],
    /// The envelope of the edited slot of the edited group.
    pub env_points: Vec<Point>,
    /// The envelope editor's view transform.
    pub env_view: EnvView,
    /// The watermark of the envelope editor.
    pub watermark: String,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            names: [vec![String::new(); 36], vec![String::new(); 36]],
            empty: [vec![true; 36], vec![true; 36]],
            selected: [0; 2],
            edited: [0; 2],
            playing: [-1; 2],
            bar_px: [0; 2],
            group: 0,
            snap: 3,
            step: false,
            slide: false,
            freeze: false,
            link: false,
            key_held: false,
            one_shot: false,
            clock: true,
            combos: ["Off".into(), "4 beats".into(), "4 beats".into()],
            params: [0, 0, 256, 256, 1765, 5000, 0, 0, 65536],
            env_points: backspin_envelope::Envelope::slot().points,
            env_view: EnvView::default(),
            watermark: "Backspin".into(),
        }
    }
}
