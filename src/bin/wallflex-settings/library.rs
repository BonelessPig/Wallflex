//! The picture library: a `Pictures` folder next to the settings file.
//!
//! The page imports (copies) pictures into it, files or whole folders,
//! and then picks from them. Settings name a library picture by its path
//! relative to the settings file's folder (`Pictures\sky.jpg`), which
//! Wallflex resolves the same way. Inside the library, pictures are
//! named by their path relative to the library (`Trips\sky.jpg`), and
//! every such name is checked before it touches the disk, so the page
//! can't reach outside the folder.

use std::path::{Component, Path, PathBuf};

/// The library's folder, next to the settings file.
pub const FOLDER: &str = "Pictures";
/// The biggest picture the page may load or import, in bytes.
pub const MAX_BYTES: usize = 128 * 1024 * 1024;
/// Picture files, by extension (lower case), with their types: only
/// pictures, so the page can't be used to read or write other files.
const PICTURE_TYPES: [(&str, &str); 8] = [
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("bmp", "image/bmp"),
    ("gif", "image/gif"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("webp", "image/webp"),
];
/// How many folders deep an imported picture may sit in the library.
const MAX_DEPTH: usize = 4;
/// The longest file or folder name accepted, in characters.
const MAX_PART_CHARS: usize = 120;
/// Characters Windows doesn't allow in a file name (besides control
/// characters).
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Names Windows keeps for devices, with or without an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];
/// The extension a picture has while it's being written, so a half
/// written file is never listed.
const PARTIAL_EXT: &str = "part";
/// The highest `name (n).jpg` tried when the name is taken.
const MAX_COPIES: u32 = 999;
/// Separator in the names given to the page (Windows style, so they
/// read naturally in the settings file).
const SEPARATOR: &str = "\\";

/// One picture in the library.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name inside the library (`Trips\sky.jpg`).
    pub name: String,
    /// Its size in bytes.
    pub bytes: u64,
}

/// What an import did.
#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    /// The picture's name inside the library.
    pub name: String,
    /// True if the same picture was already there under that name, so
    /// nothing was written.
    pub existed: bool,
}

/// The content type of a picture file, by its extension; `None` for
/// anything that isn't a picture.
pub fn picture_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    PICTURE_TYPES
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|&(_, content_type)| content_type)
}

/// Whether `part` is a plain file or folder name Windows accepts.
fn plain_part(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default();
    !part.is_empty()
        && part != "."
        && part != ".."
        && part.chars().count() <= MAX_PART_CHARS
        && !part.ends_with(['.', ' '])
        && !part
            .chars()
            .any(|c| c.is_control() || FORBIDDEN.contains(&c))
        && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem.trim()))
}

/// The file a library name (`Trips/sky.jpg` or `Trips\sky.jpg`) stands
/// for inside `library`: `None` unless every part is a plain name, it's
/// at most [`MAX_DEPTH`] folders deep and it's a picture.
pub fn resolve(library: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split(['/', '\\']).collect();
    let fits = (1..=MAX_DEPTH.saturating_add(1)).contains(&parts.len());
    if !fits || !parts.iter().all(|p| plain_part(p)) {
        return None;
    }
    let path = parts.iter().fold(library.to_path_buf(), |p, n| p.join(n));
    picture_type(&path).map(|_| path)
}

/// `path`'s name inside `library`, Windows style.
fn name_in(library: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path
        .strip_prefix(library)
        .ok()?
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join(SEPARATOR))
}

/// Every picture in the library, sorted by name (folders first within a
/// folder, as Explorer shows them). An empty list if there's no library
/// yet.
pub fn list(library: &Path) -> Vec<Entry> {
    let mut found = Vec::new();
    let mut folders = vec![(library.to_path_buf(), 0_usize)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < MAX_DEPTH {
                folders.push((path, depth.saturating_add(1)));
            } else if kind.is_file() && picture_type(&path).is_some() {
                let bytes = entry.metadata().map_or(0, |m| m.len());
                if let Some(name) = name_in(library, &path) {
                    found.push(Entry { name, bytes });
                }
            }
        }
    }
    found.sort_by_cached_key(|e| sort_key(&e.name));
    found
}

/// Sorts a name by its folders, then by the file, ignoring case; a
/// folder's own pictures come before its subfolders'.
fn sort_key(name: &str) -> (Vec<String>, String) {
    let lower = name.to_lowercase();
    let mut parts: Vec<String> =
        lower.split(SEPARATOR).map(str::to_owned).collect();
    let file = parts.pop().unwrap_or_default();
    (parts, file)
}

/// `path` with ` (n)` added to its file name: `sky.jpg` to `sky (2).jpg`.
fn numbered(path: &Path, n: u32) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy());
    let ext = path.extension().map(|e| e.to_string_lossy());
    let stem = stem.unwrap_or_default();
    let name = match ext {
        Some(ext) => format!("{stem} ({n}).{ext}"),
        None => format!("{stem} ({n})"),
    };
    path.with_file_name(name)
}

/// Whether the file at `path` holds exactly `bytes`.
fn same_file(path: &Path, bytes: &[u8]) -> bool {
    let same_size = std::fs::metadata(path)
        .is_ok_and(|m| u64::try_from(bytes.len()) == Ok(m.len()));
    same_size && std::fs::read(path).is_ok_and(|old| old == bytes)
}

/// Copies a picture into the library as `name` (checked as by
/// [`resolve`]). If the name is taken by another picture, `name (2)` and
/// so on are tried; if it's taken by the same picture, nothing is
/// written. The file is written beside its place and then renamed, so a
/// failed import leaves no half picture behind under its name.
///
/// # Errors
///
/// A message for the page if the name isn't allowed or writing fails.
pub fn import(
    library: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<Imported, String> {
    let target = resolve(library, name)
        .ok_or_else(|| format!("'{name}' isn't a picture name I can use"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("'{name}' is bigger than 128 MB"));
    }
    if let Some(folder) = target.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("can't make {}: {e}", folder.display()))?;
    }
    for n in 1..=MAX_COPIES {
        let path = if n == 1 {
            target.clone()
        } else {
            numbered(&target, n)
        };
        let existed = path.exists();
        if existed && !same_file(&path, bytes) {
            continue;
        }
        if !existed {
            write_new(&path, bytes)?;
        }
        let name = name_in(library, &path)
            .ok_or_else(|| format!("{} has an odd name", path.display()))?;
        return Ok(Imported { name, existed });
    }
    Err(format!("too many pictures named like '{name}'"))
}

/// Writes `bytes` to `path` by way of a `.part` file.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension(PARTIAL_EXT);
    std::fs::write(&partial, bytes)
        .map_err(|e| format!("can't write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| {
        // Don't leave the partial file; the rename's error is the news.
        let _ = std::fs::remove_file(&partial);
        format!("can't write {}: {e}", path.display())
    })
}

/// Deletes the library picture `name`, and any folders it leaves empty.
///
/// # Errors
///
/// A message for the page if the name isn't allowed or deleting fails.
pub fn delete(library: &Path, name: &str) -> Result<(), String> {
    let path = resolve(library, name)
        .ok_or_else(|| format!("'{name}' isn't a library picture"))?;
    std::fs::remove_file(&path)
        .map_err(|e| format!("can't delete {}: {e}", path.display()))?;
    let mut folder = path.parent();
    while let Some(dir) = folder
        && dir != library
    {
        // Fails once a folder still has something in it: then stop.
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        folder = dir.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Entry, delete, import, list, picture_type, resolve};

    /// A fresh scratch library for one test.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("wallflex-library-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn knows_pictures_by_extension() {
        let jpeg = Path::new(r"C:\pictures\sky.JPG");
        assert_eq!(picture_type(jpeg), Some("image/jpeg"));
        assert_eq!(picture_type(Path::new("sky.webp")), Some("image/webp"));
        assert_eq!(picture_type(Path::new(r"C:\a\config.toml")), None);
        assert_eq!(picture_type(Path::new(r"C:\a\noext")), None);
    }

    #[test]
    fn resolves_only_plain_names_inside() {
        let lib = Path::new(r"C:\lib");
        let sky = PathBuf::from(r"C:\lib\Trips\sky.jpg");
        assert_eq!(resolve(lib, "Trips/sky.jpg"), Some(sky.clone()));
        assert_eq!(resolve(lib, r"Trips\sky.jpg"), Some(sky));
        for bad in [
            "../sky.jpg",
            r"..\sky.jpg",
            "./sky.jpg",
            "/sky.jpg",
            r"C:\sky.jpg",
            "C:sky.jpg",
            "Trips//sky.jpg",
            "sky.txt",
            "sky",
            "con.jpg",
            "Nul .jpg",
            "trip./sky.jpg",
            "a/b/c/d/e/sky.jpg",
            "sky?.jpg",
            "",
        ] {
            assert_eq!(resolve(lib, bad), None, "{bad}");
        }
        assert!(resolve(lib, "a/b/c/d/sky.jpg").is_some());
        assert!(resolve(lib, "console.jpg").is_some());
    }

    #[test]
    fn imports_lists_and_deletes() {
        let lib = scratch("import");
        let first = import(&lib, "Trips/sky.jpg", b"one").unwrap();
        assert_eq!(first.name, r"Trips\sky.jpg");
        assert!(!first.existed);
        let again = import(&lib, "Trips/sky.jpg", b"one").unwrap();
        assert_eq!(again.name, r"Trips\sky.jpg");
        assert!(again.existed);
        let other = import(&lib, "Trips/sky.jpg", b"two").unwrap();
        assert_eq!(other.name, r"Trips\sky (2).jpg");
        import(&lib, "Beach.PNG", b"three").unwrap();
        std::fs::write(lib.join("notes.txt"), b"x").unwrap();
        let names: Vec<String> =
            list(&lib).into_iter().map(|e| e.name).collect();
        assert_eq!(
            names,
            ["Beach.PNG", r"Trips\sky (2).jpg", r"Trips\sky.jpg"]
        );
        assert!(import(&lib, "../out.jpg", b"x").is_err());

        delete(&lib, r"Trips\sky.jpg").unwrap();
        delete(&lib, r"Trips\sky (2).jpg").unwrap();
        assert!(!lib.join("Trips").exists());
        assert!(delete(&lib, "notes.txt").is_err());
        assert_eq!(
            list(&lib),
            [Entry {
                name: "Beach.PNG".to_owned(),
                bytes: 5
            }]
        );
        std::fs::remove_dir_all(&lib).unwrap();
    }

    #[test]
    fn lists_nothing_without_a_library() {
        assert_eq!(list(&scratch("missing")), []);
    }
}
