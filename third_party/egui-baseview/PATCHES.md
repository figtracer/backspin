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
- **Lints off.** `[lints]` tables allow the crate's warnings; they are upstream's.

The root `Cargo.toml` patches the git source to this directory.
