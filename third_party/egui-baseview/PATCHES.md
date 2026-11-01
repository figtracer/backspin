# Vendored egui-baseview

egui-baseview at BillyDM/egui-baseview `ec70c3fe6b2f070dcacbc22924431edbe24bd1c0` (the crate
only: no examples, CI files or screenshot), with these changes:

- **Modifiers from the event.** The egui modifiers come from each keyboard and mouse event's own
  modifier set. On macOS `META` (Cmd) is egui's `command` and `mac_cmd` and Control stays `ctrl`;
  elsewhere Control is `command`. Before, `command` was only set from separate Meta key events and
  every mouse event overwrote it with the Control state, so on macOS Cmd+A, Cmd+C, Cmd+V and Cmd+X
  typed their letter into text fields. A letter pressed with the command key (or Control) is never
  an `Event::Text`, a paste is an `Event::Paste`, and letters translate to egui keys in both cases
  (Cmd+Shift+Z is `Key::Z`).
- **Render on change.** The window renders a pass only when its shapes or textures differ from
  the last render, after a resize or focus change, every frame during the first second (hosts open
  the editor while the window is still unmapped), and at least every 500 ms. Before, it rendered
  whenever egui asked for a repaint, and `nih_plug_egui` asked on every frame.
- **Idle passes.** A pass runs when input arrived, when egui asked for one (a request without a
  delay sets a wake flag through egui's repaint callback, one with a delay sets a deadline), and
  otherwise every 50 ms, so the application still notices state it is not told about. Before,
  every frame ran a pass.
- **Lints off.** `[lints]` tables allow the crate's warnings; they are upstream's.

The root `Cargo.toml` patches the git source to this directory.
