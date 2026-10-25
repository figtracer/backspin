//! The Gross Beat editor's drawing code, ported from `gb_arm64.dylib` (see
//! `.amp/in/gfx/notes/gfx.md`): the `TDIB` canvas and its raster primitives, the gradient LUT
//! builder, and the Agg2D vector canvas bound to each bitmap.

pub mod dib;
pub mod gradient;

pub use backspin_agg2d::{Agg2D, DrawPath};
pub use dib::{Dib, Rect};
pub use gradient::gradient_lut;

/// `DAT_003545f0`: the plugin's pi is a float32 constant, widened to double for the Agg2D calls.
pub const PI: f64 = std::f32::consts::PI as f64;

/// The UI scale helper `FUN_00153750`: `Round(n * scale - 0.49999997)` (half to even).
pub fn s(n: f64, scale: f64) -> i32 {
    (n * scale - 0.49999997).round_ties_even() as i32
}
