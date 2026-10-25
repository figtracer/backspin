//! The ShaperBox 3 section of Backspin: the preset and state format, the parameter smoother and
//! the per-band shaper DSP.

pub mod bandsplit;
pub mod ftz;
pub mod graph;
pub mod shapers;
pub mod smoother;
pub mod state;
pub mod valuetree;
pub mod wave;

pub use valuetree::{Tree, Var, read_preset, write_preset};
