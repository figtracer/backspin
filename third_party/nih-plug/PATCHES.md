# Vendored nih-plug

nih-plug at robbert-vdh/nih-plug `de421011f41a6d10fc8c7a6084e4f4dee0143683` (the crates Backspin
uses: `nih_plug`, `nih_plug_derive`, `nih_plug_egui`, `nih_plug_xtask`), with its workspace
table removed and one fix:

- **Auxiliary bus bounds.** The CLAP and VST3 wrappers read auxiliary input and output buses
  past the end of the host's bus arrays when a host connects fewer buses than the plugin
  declares: the checks used `>` instead of `>=`, and the VST3 auxiliary input check compared
  against `num_outputs` instead of `num_inputs`. A missing bus now stays unconnected, and the
  buffer manager fills it with zeros.

The root `Cargo.toml` patches the git source to this directory.
