# Backspin behavior spec

Backspin is an original implementation of a beat-repeat / time-and-volume
mapping effect, with the same feature set as Image-Line Gross Beat. This spec
describes behavior only. It was written from the public Gross Beat manual and
from black-box observation of the shipped plugin (parameter list, menu items,
signal flow). No Image-Line code, presets, artwork or names are used.

## Signal flow

```
input ─┬─> ring buffer (2 bars) ──> time reader ──> time mix ──> volume ──> DC blocker ──> out
       └─> dry delay (latency) ──────────────────────┘
```

- The engine reports a fixed latency of `GUARD` frames (16). Then the
  interpolator always has future frames, and dry and wet stay aligned.
- Tempo comes from the host. When the host gives no tempo, 120 BPM is used.
- The envelope position follows the host song position while the host plays.
  When the host is stopped, the envelope position does not move.

## Lanes and slots

There are two lanes: **time** and **volume**. Each lane has 36 slots. Each
slot has a name, an envelope, and these per-slot settings:

| Setting | Values | Meaning |
|---------|--------|---------|
| Trigger sync | Off, 1/4, 1/2, 1, 2, 4 beats | A selected slot starts at the next grid line of this size. Off starts it at once. |
| Position sync | Off, 1/4, 1/2, 1, 2, 4 beats | Off starts the envelope from its beginning. A size starts it at `song position mod size`. |
| Key-held | bool | When triggered by MIDI, the slot plays while the key is down. On release, the previous slot comes back. |
| One-shot | bool | The slot plays its envelope once, then the lane goes back to slot 1. |
| Link | bool (time lane only) | Selecting time slot N also selects volume slot N. |
| Force hold | Off, Last, Next | While the slot is active, the engine holds a bar (see Hold). |

Slot selection comes from the slot parameters (automation), from the editor,
or from MIDI notes when "Trigger slots from MIDI notes" is on:

- MIDI notes 60..=95 select time slots 1..=36 (FL calls note 60 "C5").
- MIDI notes 24..=59 select volume slots 1..=36.

## Envelopes

- Each envelope is 4 beats (1 bar) long and loops.
- Points have `x` (beats, 0..=4), `y` (0..=1), a shape and a tension (-1..=1).
  The shape and tension of a point describe the segment that ends at that
  point.
- Shapes: Hold, Smooth, Single curve, Single curve 2, Single curve 3,
  Double curve, Double curve 2, Double curve 3, Half sine, Stairs,
  Smooth stairs, Pulse, Wave. For Stairs, Smooth stairs, Pulse and Wave, the
  tension sets the step or cycle count.

### Time lane

- `y = 1` (top) is live audio. `y = 0` (bottom) is 8 beats back.
- `offset_beats = (1 - y) * 8`. The read position is
  `now - GUARD - offset_beats * samples_per_beat`.
- Playback speed is the slope of the read position. A flat line plays at
  normal speed. A slope that follows the safety line (`dy/dx = -1/8`) stops
  playback. A steeper downward slope reverses.
- Audio that was never recorded (before the plugin started) reads as silence.
- "Semitone up/down" changes the slope that ends at a point, so its speed
  changes by a factor of `2^(±1/12)`.

### Volume lane

- `y` is the gain, from 0 to 1.
- Global tension (TNS, -1..=1) bends the gain: `g^(1+4t)` for t >= 0, and
  `1 - (1-g)^(1+4|t|)` for t < 0.
- Volume mix: `g' = 1 - mix + mix * g`.
- Attack and release smoothing (0..=500 ms and 0..=1000 ms): an asymmetric
  one-pole filter that uses the attack time when the gain rises and the
  release time when it falls.

## Global controls

| Control | Range | Meaning |
|---------|-------|---------|
| Time slot | 1..=36 | Active time slot |
| Volume slot | 1..=36 | Active volume slot |
| Time mix | 0..=100% | `out = dry*(1-m) + wet*m` |
| Volume mix | 0..=100% | See above |
| Attack | 0..=500 ms | Volume smoothing attack |
| Release | 0..=1000 ms | Volume smoothing release |
| Tension | -100..=100% | Volume envelope tension |
| Hold | bool | Bar repeat: loops the last bar while on |
| Scratch | 0..=100% | Extra time offset (0..8 beats) with inertia (3 cascaded one-pole filters) |

## Hold (bar repeat)

While hold is active, the ring buffer records its own output from one bar
earlier instead of the input. That loops the last bar, and the time envelope
keeps working on the looped audio. "Last" starts the loop at once. "Next"
records one more bar of input first, then loops it. The global Hold button
uses "Last".

## Options

| Option | Meaning |
|--------|---------|
| Trigger slots from MIDI notes | Enables MIDI slot selection |
| Click reduction: Off, Low (drums), High (melodic) | On a read-position jump, crossfades from the old read head to the new one over 2 ms (Low) or 10 ms (High) |
| Smoothing attack compensation | Delays the wet read position by the attack time, so transients at gate openings land after the volume ramp |
| Remove DC offset | 5 Hz one-pole high-pass on the output |
| HQ resampling | 32-tap windowed-sinc interpolation that adapts its cutoff to the speed. Otherwise 4-point cubic Hermite. |
| View scratching clock | Shows the scratch control in the editor |

## Envelope editor

- Right-click on empty space adds a point. Left-drag moves a point. Shift
  locks vertical movement and Ctrl locks horizontal movement. Alt turns off
  snapping while dragging.
- Alt+click or the point menu deletes a point. The point menu also sets the
  shape and copies or pastes the value.
- Dragging the segment's middle handle changes its tension. Right-click on
  the handle resets it.
- Snap: Off, 1/16, 1/12, 1/8, 1/6, 1/4, 1/3 beat.
- Step editing: drag to draw points at each snap step. Shift draws pulses.
- Slide remaining points: moving a point also moves every point after it.
- Freeze: turns editing off.
- Tools: flip vertically, scale levels, normalize levels, decimate points,
  smooth abrupt changes, turn all points smooth, create sequence, LFO,
  semitone slope up/down, undo/redo, copy/paste slot, reset slot.
- Time lane overlays: the safety line, reverse guides and beat ruler.
- The playback locator is drawn while the host plays.
