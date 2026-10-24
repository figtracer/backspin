# Backspin: exact port spec of Gross Beat

Backspin is a personal, unpublished Rust port of Image-Line Gross Beat (FL Studio 2026, `Gross Beat_x64.dylib`).
The target is behavior that cannot be told apart from the original: bit-identical audio for the same input,
state and parameters, the same preset format, the same MIDI behavior, and the same editor.

This file is the entry point. It gives the signal flow, the index of the detailed notes, and the decisions that
cross notes or that the notes leave to the port. The notes are normative for details. If this file and a note
disagree on a detail, the note wins, and this file has a bug.

This file replaces an older black-box spec (git history, commit 57fd882). That spec is wrong in several places;
section 6 lists them.

## 1. Sources

All paths are relative to the repository root. Addresses are arm64 VM addresses in the thin slice
`.amp/in/re/gb_arm64.dylib` (image base 0, VM address equals file offset).

| Note | Scope | Verified against the real plugin |
|---|---|---|
| `.amp/in/re/notes/state.md` | plugin interface, the 9 parameters and their scaling, saved-state layout and versions, slot model, options, slot switching, MIDI, fresh-instance defaults, factory presets | parser matches all 9 presets (parameters, 72 slot names, byte count); DC filter; MIDI behavior |
| `.amp/in/re/notes/envelope.md` | point and envelope structures, 13 segment shapes, prepare step, per-sample renderer, `.fnv` format, every editing tool, menu reachability | parser decodes all `.fnv` maps and 72 envelopes per preset; tool model self-test (`data/envtools_model.py`) |
| `.amp/in/re/notes/dsp.md` | `Eff_Render` (0x6a450): block flow, ring buffer, read path, interpolator, click reduction, mixes, volume lane, smoothers, hold, slot switching in the render, lane skipping, DC filter, stopped transport | Python model `data/dsp_model.py` is bit-identical to the plugin on 18 renders |
| `.amp/in/re/notes/ui.md` | editor window: region map, controls, colours, fonts, menus, envelope editor mouse and keyboard behavior, hints | read from the decoded form resources and handlers; checked against the FL thumbnail |

Tools and data:

- `.amp/in/re/all.c`: Ghidra decompile of every function. `python3 .amp/in/re/fn.py 0xADDR` prints one.
- `.amp/in/re/data/parse_state.py`, `presets/*.json`: state decoder and the nine decoded factory presets.
- `.amp/in/re/data/fnv_parse.py`: `.fnv` decoder.
- `.amp/in/re/data/dsp_model.py`: reference render model. `--write-state` writes state blobs for the harness.
  `--accel` uses Accelerate and gives bit-identical output.
- `.amp/in/re/data/envtools_model.py`: reference model of the editing tools.
- `.amp/in/re/ui/`: decoded forms (`*.txt`), bitmaps and cursors (`images/`), FL thumbnail.
- `.amp/in/harness/`: the oracle. It runs the real plugin offline and writes 32-bit float stereo WAV. Repeated
  runs are bit-identical. See its `README.md`.

## 2. Signal flow

Per render block, stereo interleaved f32 (dsp.md section 1 has the exact pseudocode):

```
input ──> [transport stopped?] ──yes──> output = input, one-block raised-cosine fade from the last output
   │no
   ├─> trigger check for both lanes at the block-start tick (block granularity)
   ├─> hold step
   ├─> ring.record(input)                       (skipped while holding: the ring is frozen)
   ├─> dry = input delayed 15 frames            (the reported latency; always applied while playing)
   ├─> time lane, if active:                    envelope (f64) ─> offset = env · 2 bars ─> + scratch
   │                                            ─> read position and speed ─> windowed-sinc read (8 or 16 taps)
   │                                            ─> click reduction ─> time mix with dry
   │   else wet = dry
   ├─> volume lane: envelope (f32) ─> tension curve ─> volume mix ─> two one-pole smoothers
   ├─> out = wet · vol
   └─> DC filter, if "Remove DC offset" is on
```

Units: the envelope x axis is in beats, and a slot is 4 beats long and loops. On the time lane, y = 1 means no
delay and y = 0 means 2 bars (8 beats) back. `spb` (samples per bar) is `f32(f32(sr · 240) / tempo)`.

## 3. Behavior map

| Feature | Note and section |
|---|---|
| Parameters 0..8, raw ranges, defaults, display text | state.md 2 |
| Saved state, version handling, `.fst` event 213 | state.md 3 |
| Slots: names, trigger sync, position sync, force hold, link, key-held, one-shot | state.md 4; dsp.md 7, 8 |
| Options: Remove DC, attack compensation, HQ, scratch clock view, click reduction, MIDI triggering | state.md 4; dsp.md 3.5, 5, 10; ui.md 2.3 |
| MIDI slot selection (notes 60..95 time, 24..59 volume) and the key-held stack | state.md 5 |
| Fresh instance and `Default.fst` | state.md 6; envelope.md 6 |
| Segment shapes and their constants | envelope.md 2 |
| Envelope rendering and the "big step" | envelope.md 3; dsp.md 9 |
| `.fnv` files and the length fix | envelope.md 4 |
| Editing tools and their menu reachability | envelope.md 5 (table 5.0) |
| Ring buffer, read path, interpolator | dsp.md 2, 3, 4 |
| Click reduction | dsp.md 5 |
| Time mix, volume lane, smoothers | dsp.md 6 |
| Hold and force hold | dsp.md 7 |
| Flush, play and stop | dsp.md 1, 11 |
| Editor layout, controls, menus, mouse and keyboard | ui.md |

## 4. Decisions for the port

These settle questions that cross notes or that a port must decide. Each decision keeps parity with the
original as the harness observes it.

1. **Vector math.** The plugin computes sines, cosines and the volume tension exponent with Accelerate vForce
   (`vvsin`, `vvcos`, `vvexpf`) through Image-Line's IPP-compatible wrapper (dsp.md 0). On macOS, call these
   Accelerate functions through FFI with the same arguments and batches the spec gives. That is the only way to
   be bit-identical. On other platforms, use correctly rounded libm; the difference is at most 1 f32 ulp on
   about 0.5 % of samples. All other IPP calls are plain loops; implement them as loops in the given order.
2. **Block granularity.** The trigger check, hold events and slot activation run once per block at the
   block-start tick, as in the original (dsp.md 1). Keep that. Do not make them sample-accurate.
3. **MIDI timing.** In FL, notes reach the plugin through its GUI thread, so their timing is not deterministic.
   The harness splits the render block at each note time and delivers the note between the two parts. The port
   does the same: in the plugin, split the host block at each note's timing offset and handle the note between
   the parts. The renderer CLI splits at the same times as the harness.
4. **Song position.** The plugin reads one tick value per block: `rne(FHD_GetMixingTime(GT_Ticks))`, with PPQ
   from the time signature (dsp.md 1, state.md 1). In nih-plug, compute `ticks = rne(pos_beats · PPQ)` at the
   block start (and at each split point), with PPQ = 96, the plugin's own default. The renderer CLI takes
   `--ppq` like the harness.
5. **Process mode.** `FPD_ProcessMode` selects 16 taps if the HQ option is on or `PM_HQ_NonRealtime` is set,
   else 8 (dsp.md 12). In the plugin, treat nih-plug `ProcessMode::Offline` as `PM_HQ_NonRealtime`. The renderer
   CLI takes `--process-mode` like the harness.
6. **Transport.** Map host "playing" to `FPD_SetPlaying`. A change to playing runs the flush and activates both
   lanes; a change to stopped clears the hold and the active slots (dsp.md 1). When the host gives no tempo,
   use 120 BPM.
7. **Sync table.** The original keeps the trigger and position sync table in a global, so the last instance's PPQ
   wins (state.md open question 6). Keep it per instance.
8. **Fresh instance.** A new instance has empty slot names, flat slots and MIDI triggering off. Loading
   `Default.fst` (a v4 state) gives the same slots with MIDI triggering on, because states up to v8 have no
   MIDI byte (state.md 3, 6). Reproduce both.
9. **State compatibility.** Load every version the original accepts and ignore a state whose raw version word is
   `>= 0x90004`. Save version `0x00090003`. Keep unexplained fields (point byte 13, slot header +0x44/+0x48) and
   write them back unchanged.
10. **Random tools.** "Create sequence" Rand, Full random and Humanize use the FPC `Random` generator with FL's
    process state, which cannot be reproduced (envelope.md 7). Use any good generator with the same value
    formulas, and list this in the port report as a known difference.
11. **Filter tool rounding.** Accumulate the convolution sum in f64 and round to f32 once, as envelope.md 5.10
    says. FL may differ in the last bit; this is a known difference.
12. **Code-only paths.** The tempo change crossfade (dsp.md 3.6), the stopped-transport fade (dsp.md 11) and
    the scratch inertia time constants (dsp.md 3.3) were read from code and could not be driven through the
    harness. Implement them as written, and test them with unit tests against the formulas.
13. **Editor guides.** The decompiled UI code has no separate "safety line" or "reverse guide" (ui.md 5.6).
    Draw only the guides ui.md describes. The vertical time axis shows the delay in beats, 0 to 8
    (`-(1 - v) · 8`), which matches the 2-bar scale in dsp.md 3.2 and closes ui.md open question 4.
14. **Editor look.** The FL framework widgets (knob bevels, button styles, hover blends) were not reverse
    engineered (ui.md open question 7). Match them from the thumbnail and screenshots. Positions, sizes,
    colours, texts and behavior come from ui.md.
15. **Image-Line assets.** The extracted bitmaps, cursors and the preset files are for this personal build only.
    Load factory presets from the FL install path at run time instead of copying them into the repository.

## 5. Open questions that remain

These are the only items the notes leave open. None blocks a bit-identical audio port on macOS.

- state.md: the meaning of point byte 13 and slot header +0x44/+0x48 (kept unchanged, decision 9);
  `FPD_GetParamType` names (the numbers are certain); dispatcher ID 57; plugin flag bit 28. None changes audio.
- envelope.md: filter-tool last-bit rounding (decision 11); the FPC random generator (decision 10); FL's audio
  file decoder for "Analyze audio file" (use a standard decoder; known difference); two editor quirks read from
  code but not seen running.
- dsp.md: vForce on non-Apple platforms (decision 1); the code-only paths (decision 12).
- ui.md: the real default window size (824 × 535 or 560; check a screenshot at 100 %), the cursor id mapping,
  the `sConjPack_64fc` overlay, framework widget rendering (decision 14), some text alignment flags, and redo
  menu item visibility.

## 6. Corrections to the old black-box spec

The old spec was wrong on these points. The port must follow the notes:

- Latency is 15 samples, not 16.
- The interpolator is a windowed sinc with 8 taps per side (16 with HQ or offline render), with a per-sample
  cutoff that follows the playback speed in HQ. It is not a cubic Hermite with a 32-tap HQ sinc.
- Click reduction does not crossfade between two read heads over 2 or 10 ms. It adds back the jump and decays it
  exponentially over 25 or 35 ms, and with HQ it also sweeps the low-pass (dsp.md 5).
- "Remove DC offset" is a second-order high-pass near 4 Hz (`k = f32((44100/sr) · 2.2835e-5)`, d = 0.96), not a
  5 Hz one-pole. It runs only while the transport plays.
- Slot switching has block granularity.
- The HOLD button and force hold "Next" start the hold `4 · PPQ` ticks after the event, not on a bar grid. Force
  hold "Last" starts at the next block. While holding, the ring buffer is frozen; it does not record its own
  output (dsp.md 7).
- The volume tension is a single curve with `k = sign(t) · (1001^|t| − 1)`, not `g^(1+4t)` (state.md 2).
- The attack and release controls map raw 0..10000 through `e^(v · 1e-4 · ln 501) − 1` ms (release doubled),
  and the smoothers use `exp(ln 0.01 / max(x, 1))` coefficients (dsp.md 6.3).
- There is no LFO tool for slots (envelope.md 5.12) and no "safety line" (ui.md 5.6).
