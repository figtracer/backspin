//! The ShaperBox 3 section of Backspin: the preset and state format, and (later) the shapers.

pub mod valuetree;

pub use valuetree::{Tree, Var, read_preset, write_preset};
