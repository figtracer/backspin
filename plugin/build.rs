//! Embeds the preset pack named by `BACKSPIN_PRESET_PACK` (built by `cargo xtask preset-pack`),
//! or an empty pack when it is unset: release builds ship the presets, development builds read
//! the local installs.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=BACKSPIN_PRESET_PACK");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("presets.pack");
    match std::env::var_os("BACKSPIN_PRESET_PACK") {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", PathBuf::from(&p).display());
            std::fs::copy(&p, &out).expect("BACKSPIN_PRESET_PACK is not readable");
        }
        None => std::fs::write(&out, []).unwrap(),
    }
}
