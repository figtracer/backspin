# Vendored nih-plug

nih-plug at robbert-vdh/nih-plug `de421011f41a6d10fc8c7a6084e4f4dee0143683` (the crates Backspin
uses: `nih_plug`, `nih_plug_derive`, `nih_plug_egui`, `nih_plug_xtask`), with its workspace
table removed and these changes:

- **Auxiliary bus bounds.** The CLAP and VST3 wrappers read auxiliary input and output buses
  past the end of the host's bus arrays when a host connects fewer buses than the plugin
  declares: the checks used `>` instead of `>=`, and the VST3 auxiliary input check compared
  against `num_outputs` instead of `num_inputs`. A missing bus now stays unconnected, and the
  buffer manager fills it with zeros.
- **No depth or stencil buffer.** `nih_plug_egui` asked for a 24-bit depth and 8-bit stencil
  buffer, which egui never uses. Each is a full-size buffer at the window's pixel size; the
  editor now asks for neither.
- **Repaints on request.** `nih_plug_egui` asked egui for a repaint on every frame, so the
  vendored egui-baseview rendered every frame. The editor now keeps the open window's egui context
  and asks for a repaint when the host changes a parameter (`param_value_changed`,
  `param_modulation_changed`, `param_values_changed`); egui-baseview renders when the pass output
  changes and runs idle passes at a low rate (third_party/egui-baseview/PATCHES.md).

The root `Cargo.toml` patches the git source to this directory.
