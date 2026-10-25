//! Image-Line's QuickFontCache text renderer, ported from `libQuickFontCachev2_x64.dylib`
//! (gfx.md section 5): FreeType outlines, the AGG glyph rasterizer at three columns per pixel,
//! the glyph filters and the gamma-correct subpixel compositing.

pub mod ft;
