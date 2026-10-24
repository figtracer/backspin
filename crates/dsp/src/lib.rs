//! The Gross Beat audio building blocks, bit-exact with the original on macOS arm64.
//!
//! Each module follows `.amp/in/re/notes/dsp.md`: the recording ring buffer, the read head with
//! its windowed-sinc interpolator, the one-pole smoothers, the dry delay and the DC filter. The
//! vector math that the original sends to Apple Accelerate goes through [`vforce`].

mod dc;
mod delay;
mod resampler;
mod ring;
mod smoother;
pub mod vforce;

pub use dc::DcFilter;
pub use delay::DryDelay;
pub use resampler::{Resampler, recip_estimate};
pub use ring::{GUARD, Ring};
pub use smoother::{OnePole, coef};

/// `2^-24` (0x34cb24), the denormal and "equal" threshold of the render path.
pub const EPS24: f64 = 5.960464477539063e-08;

/// `ln(0.01)` (0x3ba2d0), so `exp(LN_001 / x)` decays to 1 % in `x` samples.
pub const LN_001: f64 = -4.605170185988091;

/// The `fmaxnm` used by the threshold routines (`ippsThreshold_LT_*`): NaN loses, and +0 wins
/// over -0.
#[inline]
pub fn fmaxnm(x: f64, level: f64) -> f64 {
    if x.is_nan() || x < level || (x == level && x.is_sign_negative()) {
        level
    } else {
        x
    }
}

/// The f32 version of [`fmaxnm`].
#[inline]
pub fn fmaxnm32(x: f32, level: f32) -> f32 {
    fmaxnm(x as f64, level as f64) as f32
}
