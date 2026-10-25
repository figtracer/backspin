//! The Gross Beat editor, drawn with the plugin's own painters (gfx.md sections 7 and 8) into a
//! `TDIB`. [`paint`] renders a whole frame from a [`Model`] in the form's component order; text
//! goes through a [`Text`] renderer (the QuickFontCache port).

mod painters;
pub mod sb;

pub use painters::paint;
pub use sb::{SB_HEIGHT, SbModel, ShaperCell, paint_sb};

use backspin_gfx::{Dib, Rect};

/// The editor size at scale 1 (the plugin view).
pub const WIDTH: i32 = 830;
pub const HEIGHT: i32 = 535;
/// `SynthPanel.Color` (#344b5b).
pub const PANEL: u32 = 0xff344b5b;

/// A font as the painters name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The combo labels (VolAttWheelLabel's font).
    ComboLabel,
    /// The combo values (P_Normal).
    ComboValue,
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
}

/// Draws nothing (until the text renderer is ported).
pub struct NoText;

impl Text for NoText {
    fn draw(&mut self, _: &mut Dib, _: Font, _: &str, _: Rect, _: u32, _: u32, _: i32) {}
}

/// What the painters show.
#[derive(Clone, Debug)]
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
        }
    }
}
