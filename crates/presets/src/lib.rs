//! The presets Backspin reads: the Gross Beat state presets and envelope maps, and the shape
//! preset library. They come from the local FL Studio and ShaperBox 3 installs when present, and
//! otherwise from the pack a release build embeds (`cargo xtask preset-pack`).

mod library;
mod pack;
pub mod paths;

pub use library::{
    EDITOR_FONTS, Group, Preset, Source, collect, gross_beat_presets, maps, shape_library,
};
pub use pack::{Embedded, PackData};
