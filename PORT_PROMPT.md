# Builder prompt: exact 1:1 Rust port of Gross Beat

Copy everything below the line into the builder agent as its task.

---

You are building an exact, sample-accurate Rust port of Image-Line Gross Beat (FL Studio 2026 effect plugin) in
the repository `~/dev/backspin`. The port is for the owner's personal use only. It is never published, so the
crate stays `publish = false`. The goal is behavior that cannot be told apart from the original: the same audio
output for the same input, state and parameters, the same presets, the same MIDI behavior, and the same editor
features. Speed of delivery matters, but correctness against the original comes first.

## Ground truth

Everything you need was reverse-engineered from the real binary and checked against it. Do not invent behavior.
If the spec is silent or unclear, read the decompile (see below) and settle it there. If that also fails, write
the question down (see "When to stop") instead of guessing.

Read these in order before you write code:

1. `SPEC.md` (repo root): the overview, the full signal flow, the cross-note decisions and the index of the
   detailed notes. It is the entry point. Where it says a note is normative, the note wins on details.
2. `.amp/in/re/notes/state.md`: plugin interface, the 9 parameters and their exact scaling, the saved-state byte
   layout and its versions, slot model, options, slot switching, MIDI, fresh-instance defaults, factory presets.
3. `.amp/in/re/notes/envelope.md`: point and envelope structures, all 13 segment shapes with exact constants and
   rounding, the per-sample envelope renderer, serialization (`.fnv`), and every editing tool.
4. `.amp/in/re/notes/dsp.md`: the audio path of `Eff_Render`: ring buffer, read heads, interpolators, click
   reduction, mixes, volume smoothing, hold, scratch, DC filter, lane skipping, slot switching inside the render.
5. `.amp/in/re/notes/ui.md`: the editor window: region map, every control with its position and colours, menus,
   mouse and keyboard behavior of the envelope editor, hints and labels.

Supporting material:

- `.amp/in/re/README.md` describes the reverse-engineering workspace. `.amp/in/re/all.c` is the Ghidra decompile
  of every function; `python3 .amp/in/re/fn.py 0xADDR` prints one function. Every spec item names its address,
  so you can always go to the source.
- `.amp/in/re/data/parse_state.py` decodes `.fst` presets and raw state blobs to JSON.
  `.amp/in/re/data/presets/*.json` holds all nine factory presets decoded. `.amp/in/re/data/fnv_parse.py` decodes
  `.fnv` envelope maps. `.amp/in/re/data/envtools_model.py` is a Python model of the editing tools. Any
  `data/*_model.py` file is a reference model; use it to make test vectors.
- `.amp/in/re/ui/` holds the decoded form resources (`*.txt`) and the extracted bitmaps and cursors
  (`ui/images/`).
- The oracle: `.amp/in/harness/target/release/harness` runs the real Gross Beat offline. Read
  `.amp/in/harness/README.md`. It loads `.fst` presets or raw state blobs, sets raw parameter values, sends MIDI
  notes, and renders 32-bit float stereo WAV. Repeated runs are bit-identical. Factory presets are in
  `"/Applications/FL Studio 2026.app/Contents/Resources/FL/Data/Patches/Plugin presets/Effects/Gross Beat"`.
  Envelope maps are in `"/Applications/FL Studio 2026.app/Contents/Resources/FL/Plugins/Fruity/Effects/Gross Beat/Data/Maps"`.

Do not edit anything under `.amp/in/`. It is the frozen reference.

## What exists and what to do with it

The workspace already has crates `crates/envelope`, `crates/dsp`, `crates/engine`, the nih-plug crate `plugin`
and `xtask` (nih-plug bundler). They were written from an older black-box description that is partly wrong
(for example 16 instead of 15 samples of latency, a cubic Hermite interpolator, a 5 Hz one-pole DC filter, and
sample-accurate slot switching). Keep the crate layout and the nih-plug and egui stack, but replace their
internals so they follow the spec exactly. Delete code that the spec does not support. Do not keep an old
path next to a new one.

## Architecture

- `crates/envelope`: points, envelopes and slots as the spec defines them, the 13 shapes, the "prepare" step that
  derives the shape coefficients, the looping block renderer (f64 output for the time lane, f32 for the volume
  lane), `.fnv` read and write, and all editing tools.
- `crates/dsp`: the ring buffer, both interpolators, click-reduction crossfades, the smoothing filters, scratch
  inertia and the DC filter, each exactly as `dsp.md` specifies.
- `crates/engine`: the equivalent of `TTestPlug`. It owns the state, the parameters as raw integer values with
  the original scaling, `ProcessEvent` (tempo), the dispatcher cases that matter (sample rate, block size, time
  signature and PPQ, playing state, MIDI in), slot switching and MIDI, and `render(input, output)` that follows
  `Eff_Render` stage for stage. It loads and saves the Gross Beat state blob in the original byte format, all
  accepted versions included, so factory `.fst` presets load directly.
- `crates/render` (new): a command-line renderer `backspin-render` that drives `crates/engine` offline. Its flags
  are the same as the harness: `--input`, `--gen impulse:PERIOD|sine:HZ|ramp[:PERIOD]` (copy the harness generators
  exactly), `--seconds`, `--sr`, `--tempo`, `--block`, `--ppq`, `--pos`, `--state` (raw blob or `.fst`, last
  event 213), `--param i=v`, `--note TIME:NOTE:on|off`, `--process-mode`, `--output`, `--dump-input`,
  `--list-params`. It must call the engine in the same order as the harness calls the real plugin (see the
  harness source, `.amp/in/harness/src/main.rs`): create, sample rate, block size, time signature, tempo,
  process mode, state, parameters, enable, play, then blocks split at note times. Output format is the same
  32-bit float stereo WAV.
- `plugin`: the nih-plug wrapper (CLAP and VST3). Map the 9 host parameters to the raw integer ranges in
  `state.md`, report 15 samples of latency, take tempo, song position and play state from the host transport,
  route MIDI notes to the engine, and save and restore the state blob in the original format. Follow `SPEC.md`
  section 4 for the host mapping: split the host block at each note (decision 3), ticks from `pos_beats` with
  PPQ 96 (decision 4), offline render as `PM_HQ_NonRealtime` (decision 5), and transport changes (decision 6).
  The egui editor follows `ui.md`.

## Numerical rules

Exact output parity depends on these rules. Follow them everywhere in the audio and envelope code.

- Use the same float type as the original at every step. The spec marks f32 and f64; when it does not, check the
  decompile. Round to f32 where the original stores to f32, even in the middle of an expression.
- Keep the operation order of the original. Floating-point addition is not associative.
- Rounding is round-half-to-even (`f32::round_ties_even`, `f64::round_ties_even`) wherever the original uses
  `frintx` or a default-mode conversion. Do not use `round()` for those.
- Where the spec or the disassembly shows a fused multiply-add (`fmadd`, `fmsub`), use `mul_add`. Elsewhere, do not
  use `mul_add`. Never enable fast-math style flags.
- The original's "IPP" calls go to Image-Line's own wrapper library (`dsp.md` section 0). Most are plain loops;
  implement them as loops in the order the spec gives. Sines, cosines and the volume tension exponent go to
  Apple Accelerate vForce (`vvsin`, `vvcos`, `vvexpf`). Call those through FFI on macOS (link the Accelerate
  framework) with the same arguments and batches as the original. libm gives 1-ulp differences. Keep a libm
  fallback behind `cfg(not(target_os = "macos"))`. The plugin runs on macOS arm64, so that is the parity target.
- `FRECPE`/`FRECPS` reciprocal estimates appear in the interpolator (`dsp.md` section 4). On aarch64 use the
  four-lane NEON intrinsics (`vrecpeq_f32`, `vrecpsq_f32`) in the same sequence and lane order. No FMA is used
  in the kernel. Elsewhere, port the emulation from
  `.amp/in/re/data/dsp_model.py`.
- Keep per-block decisions per block. The original checks slot switches once per render block; do not make them
  sample-accurate.
- Use the same state and buffer sizes as the original (for example the 2-bar ring buffer length formula), so that
  wrap-around and clamping behave the same.

## Order of work and acceptance gates

Do the steps in this order. Each step has a gate. Do not start the next step until the gate passes. Commit after
each passing gate.

1. **State.** Load and save the state blob. Gate: for all nine factory presets, your loader's values (all 9
   parameters, all options, all 72 slot names, slot settings and envelope points) match
   `.amp/in/re/data/presets/*.json`. Save then load gives the same state. Saving a loaded factory preset gives a
   blob that the real plugin loads with the same `--list-params` output.
2. **Envelopes.** Shapes, prepare, renderer. Gate: unit tests for every shape at several tensions against values
   computed from the formulas in `envelope.md` (or a Python model you write from them), and a test that the
   renderer loops every 4 beats.
3. **Volume-only render.** Gate: with time mix 0 (or an empty time slot), the port matches the harness for every
   volume slot of at least three presets (Repeater, Patterns, Stutter).
4. **Time render.** Normal interpolation first, then HQ. Gate: with volume slots empty, the port matches the
   harness on a ramp input for every time slot of Repeater, Turntablist and Pitch shifter.
5. **Everything else in the audio path.** Click reduction (off, low, high), attack compensation, hold and force
   hold (off, last, next), scratch offset, DC filter, one-shot, link, trigger sync and position sync. Gate: the
   parity matrix below passes.
6. **MIDI.** Gate: note-driven slot switching, key-held return, nested key-held notes and "no retrigger on the
   current slot" match the harness (use `--note`).
7. **Plugin.** Gate: the CLAP and VST3 bundles build with `cargo xtask bundle backspin --release`, load in a host,
   and a factory preset loaded through the plugin's state renders the same as `backspin-render`.
8. **Editor.** Build the editor from `ui.md`: the layout and colours at 100 % scale, both slot grids, the knobs,
   the options menu, the envelope editor with all mouse and keyboard behavior, snapping, freeze, slide, step
   editing, the point menu, the playback locator and the guides. Gate: a side-by-side comparison with
   `.amp/in/re/ui/images/thumbnail_GrossBeat.png` shows the same layout, and every menu item in `ui.md` section 3
   exists and does what `ui.md` and `envelope.md` say.
9. **Editing tools.** Every tool in `envelope.md` section 5. Gate: unit tests against
   `.amp/in/re/data/envtools_model.py` (extend the model if a tool is missing from it), and each tool is reachable
   from the same menu as in the original.

## Parity matrix

Add `cargo xtask parity` (or a script under `scripts/`). For each case it renders the same input with the
harness and with `backspin-render`, using identical flags, and compares the outputs sample by sample. Use the
harness `--dump-input` file as the port's `--input` so both get the same input. It prints one row per case with
the max absolute difference and the index of the first sample that differs by more than the tolerance.

Cases:

- All nine factory presets. For each, every time slot that has a name with the volume slot at 0, and every volume
  slot that has a name with the time slot at 0. Also 10 random (time, volume) pairs with a fixed seed.
- Inputs: ramp (shows read position), sine 1 kHz, impulse train, and white noise with a fixed seed. The harness
  has no noise generator, so write the noise to a WAV file once and pass it to both with `--input`.
- Sample rates 44100, 48000 and 96000. Tempos 90, 120 and 174. Block sizes 512 and 64, and one odd size (333).
- Each option on and off on top of a few presets: HQ resampling, click reduction low and high, attack
  compensation, Remove DC offset. Craft the state blobs with your own saver, or by flipping the option bytes as
  `state.md` describes.
- Parameter sweeps: time mix, volume mix, attack, release, tension, hold, scratch offset at their minimum,
  default, maximum and two values in between.
- Song position: `--pos` 0, 1, 2.5 beats.

Tolerance: zero. The audio path is proven reproducible bit for bit: `.amp/in/re/data/dsp_model.py --accel`
matches the real plugin exactly on 18 renders that cover every stage. So any nonzero difference on macOS is a bug
in the port until proven otherwise. When a case fails, find the first differing sample, then find the stage that
diverges. The Python model is the fastest tool for this, because you can print any intermediate value from it
(envelope buffer, offset, read position, speed, kernel taps, declick state, smoother state) and compare it with
the same value dumped from the port. Fix the port, not the test. The model supports Hold and linear segments
only; for other shapes, compare the envelope buffers against your shape unit tests first.

## Rules

- Write Rust in the style of the existing crates. Edition 2024. Run `cargo fmt`, `cargo clippy --workspace
  --all-targets` and `cargo test --workspace` before each commit.
- No allocation, locks or I/O in the audio thread. Allocate buffers when the sample rate, block size or tempo
  changes, as the original does.
- Commit with Conventional Commits and a scope, for example `feat(engine): render volume lane`. Do not add
  co-author or "generated with" lines. Do not push.
- Do not edit `.amp/in/`.
- Keep the 15-sample latency. Do not compensate it inside the engine; the host does that.

## When to stop and ask

Stop and write the question in `PORT_QUESTIONS.md` (one entry per question: what you need, what the spec and the
decompile say, what you tried, and which parity cases it blocks) when one of these happens. Then go on with work
that does not depend on the answer.

- The spec and the decompile disagree, or the decompile does not settle a detail.
- A parity case fails and you cannot find the cause after reading the relevant functions.
- A behavior needs host information that nih-plug does not give.

## Done

The port is done when all gates pass, the parity matrix passes at the tolerance above, the editor and the
editing tools are complete, and `PORT_REPORT.md` lists: the parity matrix results (a table), every known
difference from the original with its reason (for example the FPC random generator in "Create sequence", which
cannot be reproduced bit for bit), and any open entries left in `PORT_QUESTIONS.md`.
