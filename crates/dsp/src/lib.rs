//! DSP building blocks for Backspin.

mod interp;
mod ring;
mod smooth;

pub use interp::{cubic, Interpolator, Sinc, SINC_HALF_TAPS};
pub use ring::Ring;
pub use smooth::{one_pole_coeff, AsymSmoother, DcBlocker, Inertia, OnePole};

/// A stereo frame.
pub type Frame = [f32; 2];
