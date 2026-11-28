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

/// Like [`write_atomic`], but the file is readable only by the current user (Unix: 0600;
/// Windows: an access list for the current user and the system alone, not inherited from the
/// folder). The file has these permissions from the moment it is created, before anything is
/// written to it. Used for the node's secret keys and the registry, which holds share tokens
/// and password hashes.
pub fn write_atomic_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    write_atomic_inner(path, data, true)
}

fn write_atomic_inner(path: &Path, data: &[u8], private: bool) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut options = tempfile::Builder::new();
    options.prefix(".ferry-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if private { 0o600 } else { 0o666 };
        options.permissions(std::fs::Permissions::from_mode(mode));
    }
    #[cfg(windows)]
    let mut file = if private {
        options.make_in(dir, windows_private::create)
    } else {
        options.tempfile_in(dir)
    }
    .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(not(windows))]
    let mut file = options
        .tempfile_in(dir)
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(not(any(unix, windows)))]
    let _ = private;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.into_temp_path()
        .persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(dir) {
        dir.sync_all().ok();
    }
    Ok(())
}

/// Files only the current user (and the system) can open, on Windows.
#[cfg(windows)]
mod windows_private {
    use std::{
        ffi::OsStr,
        fs::File,
        io,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        path::Path,
        ptr,
    };

    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(Some(0)).collect()
    }

    /// A NUL-terminated UTF-16 string from Windows, freed with `LocalFree`.
    ///
    /// # Safety
    /// `text` must point to a NUL-terminated string allocated with `LocalAlloc`.
    unsafe fn take_local_string(text: *mut u16) -> String {
        // SAFETY: the caller promises a NUL-terminated LocalAlloc'd string.
        unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            value
        }
    }

    /// The current user's SID, as text (`S-1-5-21-…`).
    fn current_user_sid() -> io::Result<String> {
        // SAFETY: plain Win32 calls; every buffer is sized as the API asks, and every handle
        // and allocation is released before returning.
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
            // u64s, so the buffer is aligned for TOKEN_USER.
            let mut buffer = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let ok =
                GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len);
            let error = io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                return Err(error);
            }
            let user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let mut text: *mut u16 = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(take_local_string(text))
        }
    }

    /// The access list of private files: full access for the current user and the system,
    /// nothing for anyone else, and nothing inherited from the folder (`P`).
    pub(super) fn descriptor() -> io::Result<String> {
        Ok(format!("D:P(A;;FA;;;{})(A;;FA;;;SY)", current_user_sid()?))
    }

    /// Create a new file at `path` with the private access list already in place.
    pub(super) fn create(path: &Path) -> io::Result<File> {
        let sddl = wide(OsStr::new(&descriptor()?));
        let path = wide(path.as_os_str());
        // SAFETY: the descriptor is made and freed here; the handle CreateFileW returns is
        // owned by the `File` from then on.
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let handle = CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | DELETE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            );
            let error = io::Error::last_os_error();
            LocalFree(sd);
            if handle == INVALID_HANDLE_VALUE {
                return Err(error);
            }
            Ok(File::from_raw_handle(handle))
        }
    }

    /// The access list of a file, in SDDL.
    #[cfg(test)]
    pub(super) fn access_list(path: &Path) -> io::Result<String> {
        use windows_sys::Win32::Security::{
            ACL,
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
                SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION,
        };
        let path = wide(path.as_os_str());
        // SAFETY: as above; the descriptor and the text are freed before returning.
        unsafe {
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
            let status = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let mut text: *mut u16 = ptr::null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            );
            LocalFree(sd);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(take_local_string(text))
        }
    }
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

    #[test]
    fn private_files_are_for_the_current_user_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        // Replacing a file that others could read leaves one that they can't.
        write_atomic(&path, b"open").unwrap();
        write_atomic_private(&path, b"secret").unwrap();
        write_atomic_private(&path, b"secret again").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"secret again");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            let acl = windows_private::access_list(&path).unwrap();
            assert_eq!(
                acl,
                windows_private::descriptor().unwrap(),
                "nothing else, nothing inherited"
            );
        }
    }
}
