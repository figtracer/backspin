<div align="center">

# Backspin

**A bit-exact Rust port of a time and volume gating effect, as CLAP and VST3.**

[Quick start](#quick-start) | [Parity report](PORT_REPORT.md) | [Spec](SPEC.md) | [Builder prompt](PORT_PROMPT.md)

</div>

---

Backspin is a Rust reimplementation of Image-Line Gross Beat, built on
[nih-plug](https://github.com/robbert-vdh/nih-plug). It aims for audio that
cannot be told apart from the original: the same input, state and parameters
give the same output, sample for sample.

This is still being developed, a lot more to go.

## Highlights

- Matches the original bit for bit on 5,086 offline renders on macOS arm64,
  across presets, sample rates, tempos, block sizes, options and MIDI
  ([report](PORT_REPORT.md)).
- Reads and writes the original's state format, so factory presets load
  and saved states load back into the original.
- 36 time slots and 36 volume slots with the 13 envelope shapes, MIDI slot
  switching, hold, scratching and all the editing tools.
- The original's editor, drawn code-exactly: the full frame matches the real
  editor with 0 differing pixels, text included, through ports of its
  Blend2D wrapper and its QuickFontCache text renderer.
- A shape section under the form: per-band shapers (time, crush, filter,
  pan, reverb, volume, width, noise, drive, dynamics, liquid, pitch) with
  envelope followers, read from a local ShaperBox 3 library or the presets a
  release embeds, most of them bit-exact against ShaperBox 3.
- Clamp, a clipper at the end of the chain (Type A and B, LR/MS modes, up to
  16x oversampling), bit-exact against its C++ port.
- Installers for macOS (Apple Silicon) and Windows (x64) that embed the
  presets and the editor's fonts.

## Quick start

Download an installer from the
[releases](https://github.com/figtracer/backspin/releases). On macOS, open
`Backspin-<version>-arm64.pkg`: it installs `Backspin.vst3` and `Backspin.clap` into
`/Library/Audio/Plug-Ins`; it is not notarized, so right-click it and choose Open. On
Windows, run `Backspin-<version>-windows-x64-setup.exe`: it installs into
`Common Files\VST3` and `Common Files\CLAP`; it is not signed, so SmartScreen may ask
(More info, Run anyway). The Windows build needs a CPU with FMA (Intel since 2013, AMD
since 2012). To build it
yourself, run `scripts/package.sh`. To build from source and install for your user only:

```sh
git clone https://github.com/figtracer/backspin.git
cd backspin
cargo xtask bundle backspin --release
cp -r target/bundled/Backspin.clap ~/Library/Audio/Plug-Ins/CLAP/
cp -r target/bundled/Backspin.vst3 ~/Library/Audio/Plug-Ins/VST3/
```

Rescan plugins in your DAW and add Backspin to an audio track. To look at the
editor without a host:

```sh
cargo run --release -p backspin --features standalone --bin backspin-standalone -- --backend dummy
```

## Render offline

`backspin-render` drives the engine from the command line with the same flags
as the test harness:

```sh
cargo run --release -p backspin-render -- --gen ramp --seconds 4 --output out.wav \
    --tempo 120 --sr 48000 --block 512 --state Repeater.fst --param 0=20 --param 1=0
```

`--state` takes a raw state blob or an FL `.fst` preset. `--note TIME:NOTE:on|off`
switches slots by MIDI (time slots on notes 60 to 95, volume slots on 24 to 59).
`--save-state` writes the state after the parameters are set.

## What matches and what does not

| Area | Matches the original |
| --- | :---: |
| Audio for the same input, state and parameters (macOS arm64) | Yes, bit for bit |
| Audio on other platforms | Within one f32 ulp on about 0.5 % of samples |
| State format: load and save | Yes |
| MIDI slot switching | Yes, with notes split at their sample position |
| Envelope shapes and editing tools | Yes, bit for bit against the reference models |
| Tempo-change crossfade, stopped-transport fade, scratch inertia | Ported from the code; no oracle test |
| Editor layout, colours, menus and behavior | Yes |
| Editor widget styles and glyph font | No, vector stand-ins |
| Random sequence generators and the envelope clipboard format | No |

The parity tests compare against the real plugin through an offline host. The
host and the reverse-engineering notes are not part of this repository, so
`cargo xtask parity` needs a local copy of both and an FL Studio install.
[PORT_REPORT.md](PORT_REPORT.md) lists every known difference.

## Documentation

[Parity report](PORT_REPORT.md), [port spec](SPEC.md) and the
[builder prompt](PORT_PROMPT.md) that the port was built from.

## License

[GPL-3.0-or-later](LICENSE). Built with [nih-plug](https://github.com/robbert-vdh/nih-plug)
and [egui](https://github.com/emilk/egui). Not affiliated with Image-Line. Gross
Beat and FL Studio are trademarks of Image-Line.
