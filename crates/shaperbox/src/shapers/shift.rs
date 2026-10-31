//! Shift (frequency shifter, shaper id 13; shapers_c.md section 3): a pass-through.
//!
//! ShaperBox 3.6.3 ships the Shift shaper's state, UI and processor classes, but
//! `ShiftShaperProcessor::processBlock` (vtable slot 2, 0x48a720) is empty and every
//! `FrequencyShifterStereo<N>::process` (0x47c230 and the other six instances) is a bare `ret`:
//! no function in the binary runs the shifter's coefficients or delay lines. With the shaper
//! forced on the plugin returns the band unchanged (shapers_b.md 3, maxdiff 2.8e-8 = the host's
//! f32 round trip), so the graph passes its bands through unchanged. The frequency-shift
//! algorithm (Hz per `shiftRange` step, the wave to Hz mapping, the oscillator, what `numStages`
//! changes, `spread`, `antiAlias`, the feedback and delay structure) does not exist in this
//! build and cannot be ported from it.
//!
//! State (`ShiftShaperBandState`, read but unused): `mix` (1.0), `numStages` (0..6, 4 * 2^i
//! stages), `antiAlias` (false), `shiftRange` (0..7, default 3), `spread` (0.0), `delayTime`
//! (0.0), `delaySpread` (0.5), `syncOnTrigger` (false); sections `shift` and `feedback`
//! (`SectionEnvState`). The `shift` section reads its wave with the step fix (`FUN_0031399c`).
