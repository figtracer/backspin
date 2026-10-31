//! Where the local installs keep the files Backspin reads at runtime: the FL Studio install
//! (Gross Beat's maps, artwork and factory presets), the user's FL preset folder and the
//! ShaperBox 3 preset library, on macOS and Windows.

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

/// The factory Gross Beat state presets of the FL install.
pub fn factory_presets() -> Option<PathBuf> {
    in_fl("Data/Patches/Plugin presets/Effects/Gross Beat")
}

/// The user's own Gross Beat state presets.
pub fn user_presets() -> Option<PathBuf> {
    home()
        .map(|h| h.join("Documents/Image-Line/FL Studio/Presets/Plugin presets/Effects/Gross Beat"))
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

/// The fonts FL Studio ships (`Shared/Artwork/Fonts` of the install): OpenSans, Ubuntu,
/// `ILGlyphsEx.ilfont` and others.
pub fn fonts() -> Option<PathBuf> {
    in_fl("Shared/Artwork/Fonts")
}

/// The system font folders, the shared folder (`CSIDL_FONTS`) first.
pub fn system_fonts() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
        v.push(PathBuf::from("/System/Library/Fonts"));
        v.push(PathBuf::from("/Library/Fonts"));
        if let Some(h) = home() {
            v.push(h.join("Library/Fonts"));
        }
    } else if cfg!(windows) {
        let win = std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        v.push(win.join("Fonts"));
        if let Some(a) = std::env::var_os("LOCALAPPDATA") {
            v.push(Path::new(&a).join(r"Microsoft\Windows\Fonts"));
        }
    } else {
        v.push(PathBuf::from("/usr/share/fonts/truetype/msttcorefonts"));
        if let Some(h) = home() {
            v.push(h.join(".fonts"));
        }
    }
    v
}

/// A system font file by name (for example `Arial.ttf`), or `None` if no font folder has it.
pub fn system_font(name: &str) -> Option<PathBuf> {
    system_fonts()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}
