//! Image-Line's QuickFontCache text renderer, ported from `libQuickFontCachev2_x64.dylib`
//! (gfx.md section 5): FreeType outlines, the AGG glyph rasterizer at three columns per pixel,
//! the glyph filters and the gamma-correct subpixel compositing.
//!
//! A [`Font`] is one cached font (a file at a [`Desc`]); [`draw`] lays out and composites one
//! text request into a 32 bpp [`Target`].

mod font;
pub mod ft;
mod layout;
mod raster;

pub use font::{Desc, FALLBACK_FONTS, Font, Glyph, Metrics, Source};
pub use layout::{Item, Request, Target, draw};
