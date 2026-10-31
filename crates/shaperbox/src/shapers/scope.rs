//! Oscilloscope (shaper id 12; shapers_b.md section 8, shapers_c.md section 4): display only,
//! the audio passes through.
//!
//! `OscilloscopeShaperProcessor::processBlock` (0x4a0214) copies the incoming block aside, runs
//! the band chain for the display taps (an `OscilloscopeClient` with min/max decimating rings
//! phase-locked to the section LFO) and copies the saved input back over the output, so the
//! shaper is bit-transparent: with the shaper on (split 0.3/0.7, every `viewMode`, `gain` 0.6,
//! `showSidechain`) the plugin's output equals its input after the 16-sample base delay within
//! the WAV's f32 rounding (shapers_b.md 8; re-rendered for shapers_c.md 4 with the Init rack,
//! bit-exact against `sbrender`). The graph therefore passes its bands through unchanged.
//!
//! State (`OscilloscopeState`, UI only): `viewMode` (0..3), `gain` (vertical zoom),
//! `showSidechain`, `selectedWaveGroup`; per band `mix` (1.0, unused) and the `oscilloscope`
//! section whose LFO `length` is the time span of one screen.
