//! Decodes and re-encodes every ShaperBox 3 factory preset installed on this machine.

use backspin_shaperbox::read_preset;
use std::path::PathBuf;

fn preset_files() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let root = PathBuf::from(home).join("Library/Cableguys/ShaperBox3");
    let mut out = Vec::new();
    for a in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
                if f.path().extension().is_some_and(|e| e == "dat") {
                    out.push(f.path());
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_preset_round_trips() {
    let files = preset_files();
    if files.is_empty() {
        eprintln!("ShaperBox 3 presets not installed; skipped");
        return;
    }
    let (mut ok, mut encrypted) = (0, 0);
    for f in &files {
        let data = std::fs::read(f).unwrap();
        if !data.starts_with(b"#zip#\0") {
            encrypted += 1;
            continue;
        }
        let tree = read_preset(&data).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        assert_eq!(tree.kind, "PluginState", "{}", f.display());
        let mut raw = Vec::new();
        tree.write(&mut raw);
        let original = miniz_oxide::inflate::decompress_to_vec_zlib(&data[6..]).unwrap();
        assert!(raw == original, "{}: re-encoded bytes differ", f.display());
        ok += 1;
    }
    assert!(encrypted <= 1, "{encrypted} presets without a plain header");
    assert!(ok > 1500, "only {ok} presets decoded");
}
