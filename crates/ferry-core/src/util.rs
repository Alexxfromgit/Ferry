use std::{
    collections::HashSet,
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::Context;

/// Write a file atomically and durably: write a unique temp file next to the target, flush it
/// to disk, rename it over the target, then (on Unix) sync the directory.
pub fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    write_atomic_inner(path, data, false)
}

/// Like [`write_atomic`], but the file is readable only by the current user (Unix: 0600).
/// Used for the node's secret key.
pub fn write_atomic_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    write_atomic_inner(path, data, true)
}

fn write_atomic_inner(path: &Path, data: &[u8], private: bool) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let result = (|| {
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(dir) {
            dir.sync_all().ok();
        }
        anyhow::Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

/// Convert a relative path into a `/`-separated collection entry name.
///
/// Rejects anything that is not a plain relative path (`..`, roots, prefixes) and components
/// that contain a path separator.
pub fn relative_path_to_name(path: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .with_context(|| format!("path is not valid UTF-8: {}", path.display()))?;
                anyhow::ensure!(
                    !part.contains('/') && !part.contains('\\'),
                    "invalid path component {part:?}"
                );
                parts.push(part);
            }
            other => anyhow::bail!("invalid path component {other:?} in {}", path.display()),
        }
    }
    anyhow::ensure!(!parts.is_empty(), "empty path");
    Ok(parts.join("/"))
}

/// Files that operating systems create on their own; never worth sending.
pub fn is_os_junk(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".ds_store" | "thumbs.db" | "desktop.ini" | ".localized"
    ) || lower.starts_with("._")
}

const MAX_COMPONENT_LEN: usize = 255;
const MAX_NAME_LEN: usize = 4096;

fn is_windows_reserved(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.len() == 4
        && stem[3..]
            .chars()
            .all(|c| c.is_ascii_digit() || "¹²³".contains(c)))
}

/// Check one `/`-separated entry name received from a peer.
///
/// The rules are the union of what Windows, macOS and Linux accept, so a share made anywhere can
/// be saved everywhere without silently losing or merging files.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty() && name.len() <= MAX_NAME_LEN,
        "file name in share is empty or too long"
    );
    for part in name.split('/') {
        let bad = part.is_empty()
            || part == "."
            || part == ".."
            || part.len() > MAX_COMPONENT_LEN
            || part.ends_with('.')
            || part.ends_with(' ')
            || part.chars().any(|c| {
                c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
            })
            || is_windows_reserved(part);
        anyhow::ensure!(
            !bad,
            "the share contains a file name that can't be saved safely: {name:?}"
        );
    }
    Ok(())
}

/// Check all names of a share before any file data is downloaded: every name must be valid, and
/// no two may end up as the same file on a case-insensitive file system, nor may a file share
/// its name with a folder (`a` and `a/b`).
pub fn validate_names<'a>(names: impl IntoIterator<Item = &'a str>) -> anyhow::Result<()> {
    let mut files = HashSet::new();
    let mut dirs = HashSet::new();
    for name in names {
        validate_name(name)?;
        let lower = name.to_lowercase();
        anyhow::ensure!(
            files.insert(lower.clone()),
            "the share contains two files with the same name: {name:?}"
        );
        let mut prefix = String::new();
        let parts: Vec<&str> = lower.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            dirs.insert(prefix.clone());
        }
    }
    if let Some(clash) = files.iter().find(|f| dirs.contains(*f)) {
        anyhow::bail!("the share uses {clash:?} as both a file and a folder name");
    }
    Ok(())
}

/// Turn a validated collection entry name into a path under `root`.
///
/// Names come from the network, so every component is checked again to prevent writing outside
/// `root` (path traversal).
pub fn safe_join(root: &Path, name: &str) -> anyhow::Result<PathBuf> {
    validate_name(name)?;
    let mut path = root.to_path_buf();
    for part in name.split('/') {
        path.push(part);
    }
    Ok(path)
}

/// Return `path` or, if something exists there (including a dangling symlink), the first free
/// `name (n).ext` variant next to it that is not in `taken` (lower-case names already chosen).
pub fn non_conflicting(path: &Path, taken: &HashSet<String>) -> PathBuf {
    let free = |p: &Path| {
        p.symlink_metadata().is_err()
            && !taken.contains(
                &p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
            )
    };
    if free(path) {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|candidate| free(candidate))
        .expect("infinite iterator")
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_slash_separated() {
        let name = relative_path_to_name(Path::new("photos/2026/a.jpg")).unwrap();
        assert_eq!(name, "photos/2026/a.jpg");
        assert!(relative_path_to_name(Path::new("../a")).is_err());
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new("/downloads");
        assert!(safe_join(root, "a/b.txt").is_ok());
        for bad in [
            "../x",
            "a/../../x",
            "",
            "a//b",
            "c:\\x",
            "C:x",
            "a\\..\\b",
            ".",
            "/etc/passwd",
        ] {
            assert!(safe_join(root, bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn windows_hostile_names_are_rejected() {
        for bad in [
            "NUL",
            "nul.txt",
            "dir/COM1.log",
            "LPT9",
            "notes.",
            "notes ",
            "a?b",
            "a*b",
            "a\"b",
            "a<b",
            "a|b",
            "tab\there",
            "CONIN$",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["null.txt", "COM10.txt", "console.log", ".hidden", "a b/c.d"] {
            assert!(validate_name(good).is_ok(), "{good:?} should be accepted");
        }
        assert!(validate_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn colliding_names_are_rejected() {
        assert!(validate_names(["a/b.txt", "a/c.txt"]).is_ok());
        assert!(validate_names(["Photo.jpg", "photo.jpg"]).is_err());
        assert!(validate_names(["x", "x"]).is_err());
        assert!(validate_names(["a", "a/b"]).is_err());
        assert!(validate_names(["A", "a/b"]).is_err());
    }

    #[test]
    fn conflicting_names_get_a_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.pdf");
        let none = HashSet::new();
        assert_eq!(non_conflicting(&file, &none), file);
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            non_conflicting(&file, &none),
            dir.path().join("report (1).pdf")
        );
        let taken: HashSet<String> = ["report (1).pdf".to_string()].into();
        assert_eq!(
            non_conflicting(&file, &taken),
            dir.path().join("report (2).pdf")
        );
    }

    #[test]
    fn junk_files_are_recognized() {
        for junk in [".DS_Store", "Thumbs.db", "desktop.ini", "._IMG_1.jpg"] {
            assert!(is_os_junk(junk));
        }
        assert!(!is_os_junk("IMG_1.jpg"));
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
