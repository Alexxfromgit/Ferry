//! Small JSON registry of shares and downloads, so both survive a restart.
//!
//! Blob data itself lives in the iroh-blobs store, and each share's full file list lives in its
//! own file (`shares/<id>.json`), so this registry stays small however many files are shared.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{events::ConnectionKind, link::FileEntry, util};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    /// Hex BLAKE3 hash of the collection root; also the share id.
    pub id: String,
    pub name: String,
    /// Local paths that were shared (files are served from there, not copied).
    pub source_paths: Vec<PathBuf>,
    /// Number of files; the list itself is read with `Node::share_files` when needed.
    #[serde(default)]
    pub file_count: usize,
    pub total_size: u64,
    pub created_at: u64,
    /// Filled in with the node's current address whenever a share is read (empty on disk).
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub ticket: String,
    /// Items in the selection that were not shared, so the user can be told. At most
    /// [`MAX_SKIPPED_LISTED`] are listed; `skipped_count` is the full number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedItem>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skipped_count: usize,
}

pub const MAX_SKIPPED_LISTED: usize = 100;

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Why something in a shared selection was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    /// Files the OS creates by itself (`.DS_Store`, `Thumbs.db`, `desktop.ini`, …).
    SystemFile,
    /// Symbolic links and junctions (not followed, so nothing outside the selection is shared).
    Link,
    /// A folder with no files in it; folders only exist through the files they contain.
    EmptyFolder,
    /// Could not be read (permissions, file in use, vanished while scanning).
    Unreadable,
    /// Not a regular file (device, socket, pipe).
    NotAFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedItem {
    /// Path relative to the shared item's parent, `/`-separated (e.g. `Photos/.DS_Store`).
    pub name: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    Connecting,
    Downloading,
    Exporting,
    Completed,
    /// Stopped by the user; partial data is kept so it can resume.
    Paused,
    Failed,
}

impl DownloadStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Connecting | Self::Downloading | Self::Exporting)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfo {
    /// Hex BLAKE3 hash of the collection root; also the download id.
    pub id: String,
    pub ticket: String,
    pub dest_dir: PathBuf,
    pub name: String,
    pub status: DownloadStatus,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub created_at: u64,
    /// Final location (file or folder) once completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Top-level names chosen in `dest_dir` when saving started (original → saved as), so a
    /// retried save writes to the same place instead of creating `Name (1)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_names: Option<BTreeMap<String, String>>,
    // Runtime-only fields below.
    #[serde(default)]
    pub bytes_per_sec: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionKind>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub shares: Vec<ShareInfo>,
    #[serde(default)]
    pub downloads: Vec<DownloadInfo>,
}

/// File lists found inside a registry written by an older version (share id → files). They are
/// moved into per-share files on start.
pub type LegacyFileLists = Vec<(String, Vec<FileEntry>)>;

impl Registry {
    /// Load the registry. A file that can't be read is set aside as `registry.json.corrupt-<time>`
    /// and an empty registry is used, so a damaged file never stops Ferry from starting.
    pub fn load(path: &Path) -> anyhow::Result<(Self, LegacyFileLists)> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Self::default(), Vec::new()));
            }
            Err(e) => return Err(e.into()),
        };
        let parsed = serde_json::from_slice::<serde_json::Value>(&bytes).and_then(|mut value| {
            let legacy = take_legacy_files(&mut value);
            serde_json::from_value::<Registry>(value).map(|registry| (registry, legacy))
        });
        match parsed {
            Ok((mut registry, legacy)) => {
                for (id, files) in &legacy {
                    if let Some(share) = registry.shares.iter_mut().find(|s| &s.id == id) {
                        share.file_count = files.len();
                    }
                }
                Ok((registry, legacy))
            }
            Err(e) => {
                let aside = path.with_extension(format!("json.corrupt-{}", util::now_unix()));
                tracing::warn!(
                    "registry {} is damaged ({e}); keeping a copy at {} and starting empty",
                    path.display(),
                    aside.display()
                );
                std::fs::rename(path, &aside).ok();
                Ok((Self::default(), Vec::new()))
            }
        }
    }

    /// Serialized form for disk, without runtime-only fields.
    pub fn to_json(&self) -> anyhow::Result<Vec<u8>> {
        let mut copy = self.clone();
        for share in &mut copy.shares {
            share.link.clear();
            share.ticket.clear();
        }
        for download in &mut copy.downloads {
            download.bytes_per_sec = 0;
            download.eta_secs = None;
            download.connection = None;
        }
        Ok(serde_json::to_vec_pretty(&copy)?)
    }

    pub fn share(&self, id: &str) -> Option<&ShareInfo> {
        self.shares.iter().find(|s| s.id == id)
    }

    pub fn download_mut(&mut self, id: &str) -> Option<&mut DownloadInfo> {
        self.downloads.iter_mut().find(|d| d.id == id)
    }
}

/// Remove `files` arrays from shares in an older registry and return them.
fn take_legacy_files(value: &mut serde_json::Value) -> LegacyFileLists {
    let Some(shares) = value.get_mut("shares").and_then(|s| s.as_array_mut()) else {
        return Vec::new();
    };
    shares
        .iter_mut()
        .filter_map(|share| {
            let obj = share.as_object_mut()?;
            let id = obj.get("id")?.as_str()?.to_string();
            let files = obj.remove("files")?;
            let files: Vec<FileEntry> = serde_json::from_value(files).ok()?;
            Some((id, files))
        })
        .collect()
}

/// Each share's full file list, one JSON file per share, written once when the share is made.
pub struct FileLists {
    dir: PathBuf,
}

impl FileLists {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn write(&self, id: &str, files: &[FileEntry]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        util::write_atomic(&self.path(id), &serde_json::to_vec(files)?)
    }

    pub fn read(&self, id: &str) -> anyhow::Result<Vec<FileEntry>> {
        let bytes = std::fs::read(self.path(id))
            .map_err(|e| anyhow::anyhow!("the file list of this share is missing ({e})"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn remove(&self, id: &str) {
        std::fs::remove_file(self.path(id)).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_registry_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let (registry, _) = Registry::load(&path).unwrap();
        assert!(registry.shares.is_empty());
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn file_lists_of_older_registries_are_taken_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let old = r#"{"shares": [{"id": "abc", "name": "A", "sourcePaths": [], "totalSize": 3,
            "createdAt": 1, "files": [{"name": "A/x", "size": 1}, {"name": "A/y", "size": 2}]}],
            "downloads": []}"#;
        std::fs::write(&path, old).unwrap();
        let (registry, legacy) = Registry::load(&path).unwrap();
        assert_eq!(registry.shares[0].file_count, 2);
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].0, "abc");
        assert_eq!(legacy[0].1.len(), 2);
        // Saved again without the list.
        let json = String::from_utf8(registry.to_json().unwrap()).unwrap();
        assert!(!json.contains("A/x"));
    }
}

#[cfg(test)]
mod size_probe {
    use super::*;

    /// Not a regular test: measures the registry and file-list costs of one very large share.
    /// Run with .
    #[test]
    #[ignore]
    fn large_share_cost() {
        let files: Vec<FileEntry> = (0..100_000)
            .map(|i| FileEntry {
                name: format!(
                    "Photo library/{}/{:02}/IMG_{i:06}.HEIC",
                    2000 + i % 25,
                    i % 12 + 1
                ),
                size: 3_000_000 + i as u64,
            })
            .collect();
        let registry = Registry {
            shares: vec![ShareInfo {
                id: "0".repeat(64),
                name: "Photo library".into(),
                source_paths: vec![PathBuf::from("C:/Users/example/Pictures/Photo library")],
                file_count: files.len(),
                total_size: 0,
                created_at: 0,
                link: String::new(),
                ticket: String::new(),
                skipped: Vec::new(),
                skipped_count: 0,
            }],
            downloads: Vec::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        let lists = FileLists::new(dir.path().join("shares"));
        let t = std::time::Instant::now();
        lists.write(&registry.shares[0].id, &files).unwrap();
        let list_write = t.elapsed();
        let path = dir.path().join("registry.json");
        let t = std::time::Instant::now();
        let json = registry.to_json().unwrap();
        crate::util::write_atomic(&path, &json).unwrap();
        let save = t.elapsed();
        let t = std::time::Instant::now();
        let _ = Registry::load(&path).unwrap();
        let load = t.elapsed();
        let ipc = serde_json::to_vec(&registry.shares).unwrap();
        println!(
            "file list written once in {list_write:?}; registry {} bytes, each save {save:?}, load {load:?}; list_shares payload {} bytes",
            json.len(),
            ipc.len()
        );
    }
}
