//! The preset lists the editor shows. Each list comes from the local install when it is present
//! and from the embedded pack otherwise; the user's own Gross Beat presets always come from disk.

use crate::pack::{Embedded, PackData};
use crate::paths;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Where a preset's bytes are.
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Embedded(&'static [u8]),
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::File(a), Self::File(b)) => a == b,
            (Self::Embedded(a), Self::Embedded(b)) => std::ptr::eq(*a, *b),
            _ => false,
        }
    }
}

/// A named preset.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub source: Source,
}

impl Preset {
    /// The preset's bytes (`None` if its file cannot be read).
    pub fn bytes(&self) -> Option<Cow<'static, [u8]>> {
        match &self.source {
            Source::File(p) => std::fs::read(p).ok().map(Cow::Owned),
            Source::Embedded(b) => Some(Cow::Borrowed(b)),
        }
    }
}

/// A group of shape presets (`Category@Pack` in ShaperBox's database).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub presets: Vec<Preset>,
}

impl Group {
    /// The short name: the part after the category.
    pub fn short(&self) -> &str {
        self.name.rsplit('@').next().unwrap_or(&self.name)
    }
}

/// The files with extension `ext` in `dir`, sorted, named by their stem.
fn files(dir: &Path, ext: &str) -> Vec<Preset> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
        .collect();
    v.sort();
    v.into_iter()
        .map(|p| Preset {
            name: p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            source: Source::File(p),
        })
        .collect()
}

fn embedded_files(files: &[(&'static str, &'static [u8])]) -> Vec<Preset> {
    files
        .iter()
        .map(|&(name, data)| Preset {
            name: name.into(),
            source: Source::Embedded(data),
        })
        .collect()
}

/// The Gross Beat state presets in groups: the factory presets (the FL install's, else the
/// pack's), the user's own, then each imported folder (its `.fst` files, subfolders included).
pub fn gross_beat_groups(pack: Option<&Embedded>) -> Vec<Group> {
    let factory = paths::factory_presets()
        .map(|d| files(&d, "fst"))
        .filter(|v| !v.is_empty())
        .or_else(|| pack.map(|p| embedded_files(&p.gross_beat)))
        .unwrap_or_default();
    let mut out = vec![Group {
        name: "Factory".into(),
        presets: factory,
    }];
    if let Some(d) = paths::user_presets() {
        out.push(Group {
            name: "User".into(),
            presets: files(&d, "fst"),
        });
    }
    for d in imported_folders() {
        let name = d.file_name().map_or_else(
            || d.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        out.push(Group {
            name,
            presets: files_deep(&d, "fst", 3),
        });
    }
    out.retain(|g| !g.presets.is_empty());
    out
}

/// The Gross Beat state presets of every group, in order.
pub fn gross_beat_presets(pack: Option<&Embedded>) -> Vec<Preset> {
    gross_beat_groups(pack)
        .into_iter()
        .flat_map(|g| g.presets)
        .collect()
}

/// The list of imported Gross Beat preset folders (`preset-folders.txt`, one path per line).
fn folders_file() -> Option<PathBuf> {
    paths::config().map(|d| d.join("preset-folders.txt"))
}

/// The imported Gross Beat preset folders that still exist.
pub fn imported_folders() -> Vec<PathBuf> {
    let Some(text) = folders_file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

/// Adds a folder to the imported Gross Beat preset folders.
pub fn import_folder(dir: &Path) -> std::io::Result<()> {
    let file = folders_file().ok_or(std::io::ErrorKind::NotFound)?;
    let mut all = imported_folders();
    if !all.iter().any(|p| p == dir) {
        all.push(dir.to_path_buf());
    }
    write_folders(&file, &all)
}

/// Forgets every imported Gross Beat preset folder.
pub fn forget_folders() -> std::io::Result<()> {
    match folders_file() {
        Some(file) => write_folders(&file, &[]),
        None => Ok(()),
    }
}

fn write_folders(file: &Path, all: &[PathBuf]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = all.iter().map(|p| format!("{}\n", p.display())).collect();
    std::fs::write(file, text)
}

/// The files with extension `ext` in `dir` and its subfolders down to `depth` levels, sorted by
/// path, named by their stem.
fn files_deep(dir: &Path, ext: &str, depth: usize) -> Vec<Preset> {
    let mut out = files(dir, ext);
    if depth > 0 {
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        for s in subs {
            out.extend(files_deep(&s, ext, depth - 1));
        }
    }
    out
}

/// The envelope maps (`.fnv`): the FL install's, else the pack's.
pub fn maps(pack: Option<&Embedded>) -> Vec<Preset> {
    let local = files(&paths::gross_beat().join("Data/Maps"), "fnv");
    if local.is_empty()
        && let Some(p) = pack
    {
        return embedded_files(&p.maps);
    }
    local
}

/// The shape preset library: "All presets" first, then the groups in the plugin's order. It is
/// the local ShaperBox 3 library when installed, else the pack's.
pub fn shape_library(pack: Option<&Embedded>) -> Vec<Group> {
    let (mut all, mut groups) = match paths::shape_library().and_then(|r| from_db(&r)) {
        Some(lib) => lib,
        None => match pack {
            Some(p) if !p.shapes.is_empty() => {
                let all = embedded_files(&p.shapes);
                let groups = p
                    .groups
                    .iter()
                    .map(|(name, items)| Group {
                        name: (*name).into(),
                        presets: items
                            .iter()
                            .filter_map(|&i| all.get(i as usize).cloned())
                            .collect(),
                    })
                    .collect();
                (all, groups)
            }
            _ => (Vec::new(), Vec::new()),
        },
    };
    all.sort_by_cached_key(|p| p.name.to_lowercase());
    groups.insert(
        0,
        Group {
            name: "All presets".into(),
            presets: all,
        },
    );
    groups
}

/// Preset hash `h` is the file `h[0]/h[1]/h.dat`, if it is present.
fn preset_path(root: &Path, hash: &str) -> Option<PathBuf> {
    let mut c = hash.chars();
    let (a, b) = (c.next()?, c.next()?);
    let path = root
        .join(a.to_string())
        .join(b.to_string())
        .join(format!("{hash}.dat"));
    path.is_file().then_some(path)
}

/// Every preset of the library's `presets.db` and its groups.
fn from_db(root: &Path) -> Option<(Vec<Preset>, Vec<Group>)> {
    let db = rusqlite::Connection::open_with_flags(
        root.join("presets.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let rows = |sql: &str| -> Option<Vec<(String, String, String)>> {
        let mut q = db.prepare(sql).ok()?;
        let r = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .ok()?
            .flatten()
            .collect();
        Some(r)
    };
    let file = |name: String, path: PathBuf| Preset {
        name,
        source: Source::File(path),
    };
    let all: Vec<Preset> = rows("select '', hash, name from presets")?
        .into_iter()
        .filter_map(|(_, hash, name)| Some(file(name, preset_path(root, &hash)?)))
        .collect();
    if all.is_empty() {
        return None;
    }
    let mut groups: Vec<Group> = Vec::new();
    let positions = rows(
        "select pp.pack, p.hash, p.name from pack_positions pp join presets p \
         on p.hash = pp.preset order by pp.pack, pp.position",
    )?;
    for (group, hash, name) in positions {
        let Some(path) = preset_path(root, &hash) else {
            continue;
        };
        let preset = file(name, path);
        match groups.last_mut() {
            Some(g) if g.name == group => g.presets.push(preset),
            _ => groups.push(Group {
                name: group,
                presets: vec![preset],
            }),
        }
    }
    Some((all, groups))
}

/// The FL fonts the editor draws with (Arial comes from the system on both platforms).
pub const EDITOR_FONTS: [&str; 4] = [
    "ILGlyphsEx.ilfont",
    "OpenSans-CondLight.ttf",
    "OpenSans-Regular.ttf",
    "Ubuntu-R.ttf",
];

/// Reads every preset of the local installs into a pack (for `cargo xtask preset-pack`).
pub fn collect() -> Result<PackData, String> {
    let read = |p: &Preset| {
        p.bytes()
            .map(|b| (p.name.clone(), b.into_owned()))
            .ok_or_else(|| format!("cannot read {}", p.name))
    };
    let factory = paths::factory_presets().ok_or("no FL Studio install with Gross Beat presets")?;
    let gross_beat = files(&factory, "fst")
        .iter()
        .map(read)
        .collect::<Result<_, _>>()?;
    let maps = files(&paths::gross_beat().join("Data/Maps"), "fnv")
        .iter()
        .map(read)
        .collect::<Result<_, _>>()?;
    let root = paths::shape_library().ok_or("no ShaperBox 3 library")?;
    let (all, groups) = from_db(&root).ok_or("cannot read the ShaperBox 3 presets.db")?;
    let shapes = all.iter().map(read).collect::<Result<_, _>>()?;
    let index = |p: &Preset| {
        all.iter()
            .position(|q| q.source == p.source)
            .map(|i| i as u32)
    };
    let groups = groups
        .iter()
        .map(|g| (g.name.clone(), g.presets.iter().filter_map(index).collect()))
        .collect();
    let dir = paths::fonts().ok_or("no FL Studio fonts folder")?;
    let fonts = EDITOR_FONTS
        .iter()
        .map(|&name| {
            std::fs::read(dir.join(name))
                .map(|d| (name.to_string(), d))
                .map_err(|e| format!("{name}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    Ok(PackData {
        gross_beat,
        maps,
        shapes,
        groups,
        fonts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An imported folder's presets (subfolders included) appear as their own group, and
    /// forgetting the folders removes it.
    #[test]
    fn imported_folders_add_a_group() {
        let root = std::env::temp_dir().join(format!("backspin-presets-{}", std::process::id()));
        let (config, presets) = (root.join("config"), root.join("My Gross Beat"));
        std::fs::create_dir_all(presets.join("sub")).unwrap();
        std::fs::write(presets.join("A.fst"), b"a").unwrap();
        std::fs::write(presets.join("sub/B.fst"), b"b").unwrap();
        std::fs::write(presets.join("notes.txt"), b"-").unwrap();
        // SAFETY: the only test in this crate that reads or writes the variable.
        unsafe { std::env::set_var("BACKSPIN_CONFIG_DIR", &config) };
        import_folder(&presets).unwrap();
        import_folder(&presets).unwrap();
        assert_eq!(imported_folders(), std::slice::from_ref(&presets));
        let groups = gross_beat_groups(None);
        let g = groups.iter().find(|g| g.name == "My Gross Beat").unwrap();
        let names: Vec<&str> = g.presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        forget_folders().unwrap();
        assert!(imported_folders().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
