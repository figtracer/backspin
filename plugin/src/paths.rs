//! Where the local installs keep the files Backspin reads at runtime (it ships none of them):
//! the FL Studio install (Gross Beat's maps, artwork and factory presets), the user's FL preset
//! folder and the ShaperBox 3 preset library, on macOS and Windows.

use std::path::{Path, PathBuf};

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The FL Studio installs, newest first: the folder that holds `Plugins` and `Data`.
fn fl_installs() -> Vec<PathBuf> {
    let (parents, inner): (Vec<PathBuf>, &str) = if cfg!(target_os = "macos") {
        (
            vec![PathBuf::from("/Applications")],
            "Contents/Resources/FL",
        )
    } else {
        let mut v = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(var) {
                v.push(PathBuf::from(p).join("Image-Line"));
            }
        }
        if v.is_empty() {
            v.push(PathBuf::from(r"C:\Program Files\Image-Line"));
        }
        (v, "")
    };
    let mut out: Vec<PathBuf> = parents
        .iter()
        .flat_map(|p| std::fs::read_dir(p).into_iter().flatten().flatten())
        .filter(|e| e.file_name().to_string_lossy().starts_with("FL Studio"))
        .map(|e| e.path().join(inner))
        .filter(|p| p.join("Plugins").is_dir())
        .collect();
    out.sort();
    out.reverse();
    out
}

/// The first install that has `rel`.
fn in_fl(rel: &str) -> Option<PathBuf> {
    fl_installs()
        .into_iter()
        .map(|p| p.join(rel))
        .find(|p| p.exists())
}

/// Gross Beat's folder in the FL install (maps and artwork).
pub fn gross_beat() -> PathBuf {
    in_fl("Plugins/Fruity/Effects/Gross Beat").unwrap_or_default()
}

/// The Gross Beat preset folders: the factory presets of the FL install, then the user's.
pub fn preset_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = in_fl("Data/Patches/Plugin presets/Effects/Gross Beat")
        .into_iter()
        .collect();
    if let Some(h) = home() {
        out.push(
            h.join("Documents/Image-Line/FL Studio/Presets/Plugin presets/Effects/Gross Beat"),
        );
    }
    out
}

/// The ShaperBox 3 preset library (`presets.db` and the preset files).
pub fn shape_library() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(h) = home() {
        candidates.push(h.join("Library/Cableguys/ShaperBox3"));
        candidates.push(h.join("Documents/Cableguys/ShaperBox3"));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        candidates.push(Path::new(&a).join("Cableguys/ShaperBox3"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}
