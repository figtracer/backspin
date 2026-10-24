//! The Gross Beat engine (`TTestPlug`): state, parameters, slot switching, MIDI and the render
//! callback, bit-exact with the original.
//!
//! The host drives it like FL Studio drives the plugin: [`Engine::set_sample_rate`],
//! [`Engine::set_block_size`], [`Engine::set_time_sig`], [`Engine::set_tempo`],
//! [`Engine::set_process_mode`], state and parameters, [`Engine::set_playing`], then
//! [`Engine::render`] per block. Before every call that can read the song position, the host sets
//! it with [`Engine::set_song_ticks`] (the original asks the host with `FHD_GetMixingTime`).

mod bank;
mod engine;
mod params;
mod render;
mod state;

pub use backspin_dsp as dsp;
pub use backspin_envelope as envelope;
pub use bank::{Bank, SLOTS, Slot};
pub use engine::{
    Engine, LATENCY, PM_HQ_NON_REALTIME, PM_IS_RENDERING, REC_FROM_MIDI, REC_GET_VALUE,
    REC_UPDATE_CONTROL, REC_UPDATE_VALUE,
};
pub use params::{NUM_PARAMS, PARAMS, ParamInfo, param_text};
pub use state::{STATE_VERSION, StateError, fst_state};
