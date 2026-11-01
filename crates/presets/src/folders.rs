//! The Gross Beat preset folders as a tree (Factory, User, the imported folders and their
//! subfolders) and the file operations the preset browser offers. Writes stay inside the user's
//! folder and the imported folders: the factory presets and the embedded pack are read only, and
//! no operation overwrites or deletes a file.

use crate::library::{Preset, embedded_files, files, imported_folders};
use crate::pack::Embedded;
use crate::paths;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

/// How many levels of subfolders a root folder shows.
const DEPTH: usize = 3;

/// A folder of Gross Beat presets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    /// The title of a root folder, or the folder's name on disk.
    pub name: String,
    /// The folder on disk; `None` for the embedded factory presets.
    pub path: Option<PathBuf>,
    /// Whether presets can be written here: the user's folder and the imported folders.
    pub writable: bool,
    /// Whether this is an imported root folder, which can be removed from the list.
    pub imported: bool,
    pub presets: Vec<Preset>,
    pub children: Vec<Folder>,
}

impl Folder {
    /// Reads `dir` and its subfolders down to `depth` levels.
    fn scan(name: String, dir: &Path, depth: usize, writable: bool) -> Self {
        let mut children = Vec::new();
        if depth > 0 {
            let mut subs: Vec<PathBuf> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            subs.sort();
            children.extend(subs.iter().map(|s| {
                let name = s
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Self::scan(name, s, depth - 1, writable)
            }));
        }
        let mut presets = files(dir, "fst");
        presets.sort_by_cached_key(|p| p.name.to_lowercase());
        Self {
            name,
            path: Some(dir.to_path_buf()),
            writable,
            imported: false,
            presets,
            children,
        }
    }

    /// How many presets this folder and its subfolders hold.
    pub fn count(&self) -> usize {
        self.presets.len() + self.children.iter().map(Self::count).sum::<usize>()
    }
}

/// The Gross Beat preset folders: Factory (the FL install's, else the pack's), User (the FL
/// user preset folder, shown before it exists so presets can be copied there), then each
/// imported folder.
pub fn gross_beat_folders(pack: Option<&Embedded>) -> Vec<Folder> {
    let factory = match paths::factory_presets() {
        Some(d) => Folder::scan("Factory".into(), &d, 0, false),
        None => Folder {
            name: "Factory".into(),
            presets: pack
                .map(|p| embedded_files(&p.gross_beat))
                .unwrap_or_default(),
            ..Folder::default()
        },
    };
    let mut out = vec![factory];
    if let Some(d) = paths::user_presets() {
        out.push(Folder::scan("User".into(), &d, DEPTH, true));
    }
    for d in imported_folders() {
        let name = d.file_name().map_or_else(
            || d.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut f = Folder::scan(name, &d, DEPTH, true);
        f.imported = true;
        out.push(f);
    }
    out
}

/// Whether presets may be written in `dir`: the user's folder, an imported folder, or a
/// subfolder of one.
pub fn writable(dir: &Path) -> bool {
    paths::user_presets()
        .into_iter()
        .chain(imported_folders())
        .any(|root| dir.starts_with(root))
}

fn check_writable(dir: &Path) -> Result<()> {
    if writable(dir) {
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            format!("{} is read only", dir.display()),
        ))
    }
}

/// A folder or preset name: not empty, no path separators, not `.` or `..`.
fn check_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("\"{name}\" is not a valid name"),
        ));
    }
    Ok(name)
}

/// A path in `dir` for `stem.ext` that no file has yet: `stem.ext`, then `stem (2).ext` and so
/// on.
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// The stem and extension of a preset file.
fn parts(file: &Path) -> Result<(String, String)> {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = file.extension().map(|s| s.to_string_lossy().into_owned());
    stem.zip(ext).ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a preset file", file.display()),
        )
    })
}

/// Moves preset `file` into `dir` and returns its new path, with a count added to the name
/// when `dir` already has a file of that name. Both folders must be writable.
pub fn move_preset(file: &Path, dir: &Path) -> Result<PathBuf> {
    let from = file.parent().unwrap_or(file);
    check_writable(from)?;
    check_writable(dir)?;
    if from == dir {
        return Ok(file.to_path_buf());
    }
    let (stem, ext) = parts(file)?;
    std::fs::create_dir_all(dir)?;
    let to = unique_path(dir, &stem, &ext);
    if std::fs::rename(file, &to).is_err() {
        // Another volume: copy, then remove the original once the copy is complete.
        let n = std::fs::copy(file, &to)?;
        if n != std::fs::metadata(file)?.len() {
            let _ = std::fs::remove_file(&to);
            return Err(Error::other(format!(
                "the copy of {} is incomplete",
                file.display()
            )));
        }
        std::fs::remove_file(file)?;
    }
    Ok(to)
}

/// Renames preset `file` to `name` (without the extension) in its folder and returns its new
/// path. The folder must be writable and no file of that name may exist.
pub fn rename_preset(file: &Path, name: &str) -> Result<PathBuf> {
    let dir = file.parent().unwrap_or(file);
    check_writable(dir)?;
    let name = check_name(name)?;
    let (stem, ext) = parts(file)?;
    if name == stem {
        return Ok(file.to_path_buf());
    }
    let to = dir.join(format!("{name}.{ext}"));
    if to.exists() {
        return Err(Error::new(
            ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    std::fs::rename(file, &to)?;
    Ok(to)
}

/// Copies `preset` (a file or an embedded preset) into `dir` and returns the new path, with a
/// count added to the name when `dir` already has a file of that name.
pub fn copy_preset(preset: &Preset, dir: &Path) -> Result<PathBuf> {
    check_writable(dir)?;
    let name = check_name(&preset.name)?;
    let bytes = preset.bytes().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            format!("cannot read the preset {}", preset.name),
        )
    })?;
    std::fs::create_dir_all(dir)?;
    let to = unique_path(dir, name, "fst");
    std::fs::write(&to, bytes)?;
    Ok(to)
}

/// Creates the subfolder `name` in `dir` and returns its path. `dir` must be writable.
pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf> {
    check_writable(dir)?;
    let to = dir.join(check_name(name)?);
    if to.exists() {
        return Err(Error::new(
            ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    std::fs::create_dir_all(&to)?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Source, forget_folder, import_folder};
    use std::sync::{Mutex, MutexGuard};

    /// The tests set `BACKSPIN_CONFIG_DIR`, so they run one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    /// A fresh root folder with the config folder inside it, and the lock on the variable.
    fn sandbox(name: &str) -> (MutexGuard<'static, ()>, PathBuf) {
        let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("backspin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: every test that reads or writes the variable holds `ENV`.
        unsafe { std::env::set_var("BACKSPIN_CONFIG_DIR", root.join("config")) };
        (guard, root)
    }

    fn names(f: &Folder) -> Vec<&str> {
        f.presets.iter().map(|p| p.name.as_str()).collect()
    }

    /// An imported folder appears as a root with its subfolders, and forgetting it removes it.
    #[test]
    fn imported_folders_are_roots() {
        let (_env, root) = sandbox("roots");
        let presets = root.join("My Gross Beat");
        std::fs::create_dir_all(presets.join("sub")).unwrap();
        std::fs::write(presets.join("A.fst"), b"a").unwrap();
        std::fs::write(presets.join("sub/B.fst"), b"b").unwrap();
        std::fs::write(presets.join("notes.txt"), b"-").unwrap();
        import_folder(&presets).unwrap();
        import_folder(&presets).unwrap();
        assert_eq!(imported_folders(), std::slice::from_ref(&presets));
        let folders = gross_beat_folders(None);
        assert_eq!(folders[0].name, "Factory");
        assert!(!folders[0].writable);
        let f = folders.iter().find(|f| f.name == "My Gross Beat").unwrap();
        assert!(f.writable && f.imported);
        assert_eq!(names(f), ["A"]);
        assert_eq!(f.children.len(), 1);
        assert_eq!(f.children[0].name, "sub");
        assert_eq!(names(&f.children[0]), ["B"]);
        assert_eq!(f.count(), 2);
        forget_folder(&presets).unwrap();
        assert!(imported_folders().is_empty());
        assert!(!gross_beat_folders(None).iter().any(|f| f.imported));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Move, rename, copy and new folder work inside an imported folder, never overwrite, and
    /// refuse folders outside the writable ones.
    #[test]
    fn file_operations_stay_in_writable_folders() {
        let (_env, root) = sandbox("ops");
        let lib = root.join("Lib");
        let sub = lib.join("Sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(lib.join("A.fst"), b"a").unwrap();
        std::fs::write(sub.join("A.fst"), b"other a").unwrap();
        import_folder(&lib).unwrap();
        assert!(writable(&sub));
        assert!(!writable(&root));

        // A move into a folder that has the name picks a new one and keeps both files.
        let moved = move_preset(&lib.join("A.fst"), &sub).unwrap();
        assert_eq!(moved, sub.join("A (2).fst"));
        assert_eq!(std::fs::read(&moved).unwrap(), b"a");
        assert_eq!(std::fs::read(sub.join("A.fst")).unwrap(), b"other a");
        assert!(!lib.join("A.fst").exists());
        // Moving into the folder the file is in changes nothing.
        assert_eq!(move_preset(&moved, &sub).unwrap(), moved);

        let renamed = rename_preset(&moved, "B").unwrap();
        assert_eq!(renamed, sub.join("B.fst"));
        assert_eq!(rename_preset(&renamed, "B").unwrap(), renamed);
        let clash = rename_preset(&renamed, "A").unwrap_err();
        assert_eq!(clash.kind(), ErrorKind::AlreadyExists);
        assert_eq!(
            rename_preset(&renamed, "x/y").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            rename_preset(&renamed, " ").unwrap_err().kind(),
            ErrorKind::InvalidInput
        );

        let embedded = Preset {
            name: "A".into(),
            source: Source::Embedded(b"factory"),
        };
        assert_eq!(copy_preset(&embedded, &sub).unwrap(), sub.join("A (2).fst"));
        let new_dir = lib.join("Fresh");
        assert_eq!(
            copy_preset(&embedded, &new_dir).unwrap(),
            new_dir.join("A.fst")
        );
        assert_eq!(std::fs::read(new_dir.join("A.fst")).unwrap(), b"factory");

        assert_eq!(create_folder(&lib, "Made").unwrap(), lib.join("Made"));
        assert!(lib.join("Made").is_dir());
        assert_eq!(
            create_folder(&lib, "Made").unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );

        // Nothing is written outside the writable folders.
        let outside = root.join("Outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("C.fst"), b"c").unwrap();
        let denied = ErrorKind::PermissionDenied;
        assert_eq!(
            move_preset(&outside.join("C.fst"), &sub)
                .unwrap_err()
                .kind(),
            denied
        );
        assert_eq!(move_preset(&renamed, &outside).unwrap_err().kind(), denied);
        assert_eq!(
            rename_preset(&outside.join("C.fst"), "D")
                .unwrap_err()
                .kind(),
            denied
        );
        assert_eq!(copy_preset(&embedded, &outside).unwrap_err().kind(), denied);
        assert_eq!(create_folder(&outside, "X").unwrap_err().kind(), denied);
        assert!(outside.join("C.fst").exists() && renamed.exists());

        let folders = gross_beat_folders(None);
        let f = folders.iter().find(|f| f.name == "Lib").unwrap();
        let kids: Vec<&str> = f.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(kids, ["Fresh", "Made", "Sub"]);
        assert_eq!(names(&f.children[2]), ["A", "A (2)", "B"]);
        forget_folder(&lib).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
