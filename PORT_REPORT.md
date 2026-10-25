# Backspin port report

Backspin is a Rust port of Image-Line Gross Beat (FL Studio 2026) for macOS arm64, built from the
notes in `.amp/in/re/notes/` and checked against the real plugin through the offline harness.
This file lists the parity results, every known difference from the original, and the open
questions.

## Gates

| Gate | Result | Evidence |
|---|---|---|
| 1. State | pass | The loader matches all nine decoded presets (`crates/engine/tests/state.rs`). Blobs saved by the port load in the real plugin with identical `--list-params` output for all nine presets. |
| 2. Envelopes | pass | 15,640 shape weights match a model written from envelope.md (`scripts/shape_vectors.py`). All 13 shapes at 13 tension levels render bit-identical in the real plugin through a volume slot. |
| 3. Volume render | pass | Every named volume slot of every preset, time slot 0 (parity group `presets`). |
| 4. Time render | pass | Every named time slot of every preset with HQ on (group `presets`), and HQ off and process mode 2 (group `options`). |
| 5. Audio path | pass | The full parity matrix below. Unit tests cover the three code-only paths. |
| 6. MIDI | pass | Group `midi` plus the MIDI-driven `features` cases; `crates/engine/tests/midi.rs`. |
| 7. Plugin | pass | `cargo xtask plugin-check`: the CLAP and VST3 bundles load in minimal hosts, take the factory presets through the plugin state, and render bit-identical to `backspin-render` (RESULTS_PLUGIN). |
| 8. Editor | see below | Layout, controls and every menu item of ui.md section 3. |
| 9. Editing tools | pass | 55 tool cases match the reference models bit for bit (`crates/envelope/tests/tools.rs`). Every tool is in the same menu as in the original. |

## Parity matrix

`cargo xtask parity` renders each case with the harness and with `backspin-render`, with
identical flags and input, and compares every sample. The tolerance is zero. The per-case table is
written to `target/parity/report.md`.

RESULTS_MATRIX

What the groups cover:

- `presets`: every named time slot (volume slot 0) and every named volume slot (time slot 0) of
  the nine factory presets, plus 10 random (time, volume) pairs per preset with a fixed seed.
  Inputs: ramp, sine 1 kHz, impulse train, white noise (fixed seed). 48 kHz, 120 BPM, block 512.
- `configs`: sample rates 44100, 48000, 96000 × tempos 90, 120, 174 × blocks 512, 64, 333, two
  pairs per preset, ramp and noise.
- `options`: HQ on and off, click reduction off, low and high, attack compensation on and off,
  Remove DC flipped, each with and without `--process-mode 2`, on five presets.
- `sweeps`: time mix, volume mix, attack, release, tension, hold and scratch offset at the minimum,
  the default, the maximum and two values in between.
- `pos`: song position 0, 1 and 2.5 beats.
- `midi`: note switching, key-held return, nested key-held notes, a repeated note on the current
  slot, at three block sizes.
- `features`: crafted states for what no factory preset uses: force hold Last and Next with every
  click mode, one-shot, link, key-held with force hold, and trigger and position sync
  combinations, switched by MIDI notes, with and without the HOLD parameter.

## Known differences from the original

Audio (none on macOS arm64 in the cases above):

- On other platforms the vForce calls fall back to libm, which differs by at most one f32 ulp on
  about 0.5 % of samples (SPEC.md decision 1). The FRECPE emulation for non-aarch64 targets is
  checked against the NEON kernel by a unit test.
- The tempo-change crossfade, the stopped-transport fade and the scratch inertia are ported from
  the code only; the harness cannot drive them (decision 12). Unit tests check them against the
  formulas.
- Host mapping: MIDI notes split the host block at the note position (decision 3); FL delivers
  them through its GUI thread, so the original's timing is not deterministic. A host position
  jump while playing is treated as `FPD_SongPosChanged`.
- The sync table is per instance; the original keeps it in a global (decision 7).

- Slot parameters outside 0..35 (`--param 0=36`, `-1`, `1000`) crash the original in the next render (it
  indexes the slot table without a bound check); the port renders without crashing. FL never sends these
  values, so no project can reach them.

State:

- Point byte 13 is written as the original writes it: the sign that Prepare derives for points
  1 and up, and the loaded byte for point 0 (envelope.md 1.1). SPEC.md decision 9 says to keep the
  byte unchanged; the note wins on details. Blobs saved this way load in the real plugin with the
  same values.
- Every load runs the original's length fix, so an envelope whose first point is past 0 gains a
  copy of that point on each load (Momentary and Patterns volume slot 29). This is the original's
  behavior.

Editor and tools:

- The FL framework widgets (knob bevels, button styles, hover effects) and the IL glyph font were
  not reverse engineered. The editor draws them after the thumbnail, with vector stand-ins for
  the glyphs (decision 14). Positions, sizes, colours, texts and behavior follow ui.md.
- The window size is 824 × 560 (ui.md 2.1). Compact mode (SlotZoom) asks the host for a 266 px
  wide window through `request_resize`; nih-plug's egui wrapper cannot resize its own view from
  code, so a host that does not resize keeps the full window with the compact layout on the left.
- Hints show as tooltips in the editor; FL shows them in its hint bar. FL's own parameter items
  in the knob menu (Edit events, Link to controller) and FL's undo are not available through the
  plugin API.
- Copy state / Paste state use a clipboard inside the process (all Backspin instances share it),
  not the system clipboard format "Image Line Envelope". Copy value / Paste value use the system
  text clipboard.
- Create sequence: Randomize, Full random and Humanize use their own generator; the original uses
  FPC `Random` with FL's process state (decision 10).
- Filter: the convolution sums in f64 and rounds to f32 once; FL may differ in the last bit
  (decision 11).
- Analyze audio file decodes with Symphonia (WAV, AIFF, FLAC, MP3, Ogg Vorbis); FL uses its own
  loader, so compressed files can decode to slightly different frames, and WavPack, REX and M4A
  files are not supported.
- The scratch clock springs back to "no offset" (65536) on release. ui.md lists the control's
  default as 0 with 65536 per turn; 0 and 65536 are the same hand position, and 65536 is the
  parameter default.
- The -1..-7 axis tags are drawn on the time group only.
- The tension drag speed (`DAT_003b1ecc` is runtime data) is 1/200 per pixel.

## Independent check

`.amp/in/check/indep.py` (outside the repository, next to the harness) runs 572 cases written apart from the
parity matrix: 49 real-world states taken from FL projects (as saved and with random slots, block 97), parameter
automation inside the render (`--param-at`: slot switching, ramps every 64 samples and random jumps on every
parameter), out-of-range parameter values, block sizes 1 to 8192, 22.05 and 192 kHz, 20 to 999 BPM, PPQ 24 and
960, fractional song positions, process mode flags, a 60 s render, hostile inputs (silence, DC, ±10.0, denormals,
NaN and infinity bursts) and MIDI edge cases. All 572 match bit for bit or, for the 29 out-of-range slot cases,
crash the original only (above). It found one bug, now fixed: the plugin applied host parameter changes before it
set the song position of the block, so the HOLD button scheduled its start from the previous block.

## Open questions

PORT_QUESTIONS.md has no open entries. The open items of SPEC.md section 5 remain as they are
there; none blocks bit-identical audio.
