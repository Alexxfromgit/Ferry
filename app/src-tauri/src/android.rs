//! The phone side of receiving. Downloads land in app storage first (`<app data>/incoming`);
//! once one completes, its files are copied into the shared Downloads collection under
//! `Download/Ferry` and the app's copy is deleted. While anything is transferring, or any
//! share is on (see `android_send.rs`), a foreground service with a notification keeps Ferry
//! running with the screen off.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use ferry_core::{DownloadInfo, DownloadStatus, Event, ShareInfo};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_ferry_mobile::{FerryMobileExt, SavedFile, TransferStatus};

use crate::{AppState, CmdResult, fail, phone};

/// Tells the UI that a download's saved state changed (payload: the download id).
const SAVED_EVENT: &str = "ferry-saved";
/// How often the progress notification may change while the set of transfers stays the same.
const NOTIFY_EVERY: Duration = Duration::from_secs(1);

/// What became of a completed download's files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SaveState {
    Saving,
    Saved {
        /// Folder or file shown to the user, e.g. `Download/Ferry/Holiday`.
        location: String,
        files: Vec<SavedFile>,
    },
    Failed {
        error: String,
    },
}

pub struct Phone {
    path: PathBuf,
    saved: Mutex<BTreeMap<String, SaveState>>,
    transfers: Mutex<Transfers>,
}

#[derive(Default)]
struct Transfers {
    active: BTreeMap<String, DownloadInfo>,
    /// Shares that are on, by id.
    sharing: BTreeMap<String, phone::Sharing>,
    shown: Option<TransferStatus>,
    shown_at: Option<Instant>,
}

impl Transfers {
    /// Track a share's on/off state. True if the set of shares that are on changed.
    fn track_share(&mut self, share: &ShareInfo) -> bool {
        if share.paused {
            return self.sharing.remove(&share.id).is_some();
        }
        let fresh = phone::Sharing {
            name: share.name.clone(),
            peers: 0,
            bytes_per_sec: 0,
        };
        self.sharing.insert(share.id.clone(), fresh).is_none()
    }
}

impl Phone {
    /// Load what was saved before. A save interrupted by the app closing is retried later.
    pub fn load(path: PathBuf) -> Self {
        let mut saved: BTreeMap<String, SaveState> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        saved.retain(|_, s| !matches!(s, SaveState::Saving));
        Self {
            path,
            saved: Mutex::new(saved),
            transfers: Mutex::default(),
        }
    }

    fn set(&self, app: &AppHandle, id: &str, state: Option<SaveState>) {
        let snapshot = {
            let mut saved = self.saved.lock().unwrap();
            match state {
                Some(state) => saved.insert(id.to_owned(), state),
                None => saved.remove(id),
            };
            saved.clone()
        };
        let json = serde_json::to_vec_pretty(&snapshot).expect("saved state serializes");
        if let Err(e) = ferry_core::write_atomic(&self.path, &json) {
            tracing::warn!("could not record saved downloads: {e}");
        }
        app.emit(SAVED_EVENT, id).ok();
    }

    fn get(&self, id: &str) -> Option<SaveState> {
        self.saved.lock().unwrap().get(id).cloned()
    }
}

/// Save completed downloads whose files are still waiting in app storage, e.g. because the
/// app closed while saving.
pub fn resume(app: &AppHandle) {
    let state = app.state::<AppState>();
    let phone = app.state::<Phone>();
    // Shares that were on when Ferry last closed are served again: keep the phone reachable.
    {
        let mut transfers = phone.transfers.lock().unwrap();
        let mut changed = false;
        for share in state.node.list_shares() {
            changed |= transfers.track_share(&share);
        }
        update_notification(app, &mut transfers, changed);
    }
    for download in state.node.list_downloads() {
        let waiting = download.status == DownloadStatus::Completed
            && phone::saved_items(&download)
                .iter()
                .any(|item| item.exists())
            && !matches!(phone.get(&download.id), Some(SaveState::Saved { .. }));
        if waiting {
            save(app.clone(), download);
        }
    }
}

/// Follow core events: keep the transfer notification current and save completed downloads.
pub fn on_event(app: &AppHandle, event: &Event) {
    let Some(phone) = app.try_state::<Phone>() else {
        return;
    };
    match event {
        Event::DownloadUpdated { download } => {
            {
                let mut transfers = phone.transfers.lock().unwrap();
                let changed = if download.status.is_active() {
                    transfers
                        .active
                        .insert(download.id.clone(), download.clone())
                        .is_none()
                } else {
                    transfers.active.remove(&download.id).is_some()
                };
                update_notification(app, &mut transfers, changed);
            }
            let unsaved = !matches!(
                phone.get(&download.id),
                Some(SaveState::Saving | SaveState::Saved { .. })
            );
            if download.status == DownloadStatus::Completed && unsaved {
                save(app.clone(), download.clone());
            }
        }
        Event::DownloadRemoved { download_id } => {
            let mut transfers = phone.transfers.lock().unwrap();
            let changed = transfers.active.remove(download_id).is_some();
            update_notification(app, &mut transfers, changed);
            drop(transfers);
            // The saved files stay in Downloads; Ferry just forgets about them.
            if phone.get(download_id).is_some() {
                phone.set(app, download_id, None);
            }
        }
        Event::ShareCreated { share } | Event::ShareUpdated { share } => {
            let mut transfers = phone.transfers.lock().unwrap();
            let changed = transfers.track_share(share);
            update_notification(app, &mut transfers, changed);
        }
        Event::ShareRemoved { share_id } => {
            let mut transfers = phone.transfers.lock().unwrap();
            let changed = transfers.sharing.remove(share_id).is_some();
            update_notification(app, &mut transfers, changed);
        }
        Event::ShareActivity {
            share_id,
            active_peers,
            bytes_per_sec,
            ..
        } => {
            let mut transfers = phone.transfers.lock().unwrap();
            let changed = match transfers.sharing.get_mut(share_id) {
                Some(sharing) => {
                    let started_or_ended = (sharing.peers == 0) != (*active_peers == 0);
                    sharing.peers = *active_peers;
                    sharing.bytes_per_sec = *bytes_per_sec;
                    started_or_ended
                }
                None => false,
            };
            update_notification(app, &mut transfers, changed);
        }
        _ => {}
    }
}

fn update_notification(app: &AppHandle, transfers: &mut Transfers, changed: bool) {
    let next = transfer_status(transfers);
    let due = transfers
        .shown_at
        .is_none_or(|at| at.elapsed() >= NOTIFY_EVERY);
    if next == transfers.shown || !(changed || due || next.is_none()) {
        return;
    }
    transfers.shown = next.clone();
    transfers.shown_at = Some(Instant::now());
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = app.ferry_mobile().set_transfer(next.as_ref()) {
            tracing::warn!("could not update the transfer notification: {e}");
        }
    });
}

/// The notification: receiving takes precedence, then shares that are on. `None` when there
/// is neither, which stops the foreground service.
fn transfer_status(transfers: &Transfers) -> Option<TransferStatus> {
    let sharing = !transfers.sharing.is_empty();
    if let Some((title, text, progress)) = phone::transfer_text(transfers.active.values()) {
        return Some(TransferStatus {
            title,
            text,
            progress,
            sharing,
        });
    }
    phone::sharing_text(transfers.sharing.values()).map(|(title, text)| TransferStatus {
        title,
        text,
        progress: None,
        sharing: true,
    })
}

/// Copy a completed download into Downloads/Ferry, then delete the app's copy.
fn save(app: AppHandle, download: DownloadInfo) {
    let phone = app.state::<Phone>();
    phone.set(&app, &download.id, Some(SaveState::Saving));
    tauri::async_runtime::spawn_blocking(move || {
        let result = save_files(&app, &download);
        let state = match result {
            Ok(saved) => saved,
            Err(error) => {
                tracing::warn!(
                    "could not save download {} to Downloads: {error}",
                    download.id
                );
                SaveState::Failed { error }
            }
        };
        app.state::<Phone>().set(&app, &download.id, Some(state));
    });
}

fn save_files(app: &AppHandle, download: &DownloadInfo) -> Result<SaveState, String> {
    // Only this download's own items: the staging folder may hold other downloads too.
    let items = phone::saved_items(download);
    if items.is_empty() {
        return Err("The download has no files to save.".into());
    }
    let root = &download.dest_dir;
    let mut files = Vec::new();
    for item in &items {
        collect_files(item, &mut files).map_err(fail)?;
    }
    let mobile = app.ferry_mobile();
    let mut saved = Vec::with_capacity(files.len());
    for file in &files {
        let dir = phone::relative_dir(root, file)?;
        let path = file.to_str().ok_or("A file name isn't valid Unicode.")?;
        saved.push(mobile.save_to_downloads(path, &dir)?);
    }
    // Everything is in Downloads now; free the space in app storage.
    for item in &items {
        let removed = if item.is_dir() {
            std::fs::remove_dir_all(item)
        } else {
            std::fs::remove_file(item)
        };
        if let Err(e) = removed {
            tracing::warn!("could not delete {} after saving: {e}", item.display());
        }
    }
    let location = match (items.as_slice(), files.as_slice(), saved.first()) {
        // A single file: where Android put it (it may have renamed it).
        ([item], [file], Some(first)) if item == file => {
            format!("{}/{}", phone::DOWNLOADS_DIR, first.name)
        }
        // A single folder.
        ([item], ..) => format!(
            "{}/{}",
            phone::DOWNLOADS_DIR,
            item.file_name().unwrap_or_default().to_string_lossy()
        ),
        // Several items, side by side in Download/Ferry.
        _ => phone::DOWNLOADS_DIR.to_owned(),
    };
    Ok(SaveState::Saved {
        location,
        files: saved,
    })
}

/// Every file under `path` (or `path` itself), in a stable order.
fn collect_files(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        out.push(path.to_owned());
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for entry in entries {
        collect_files(&entry, out)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ commands

/// Save state of every download Ferry has saved or is saving, by download id.
#[tauri::command]
pub fn saved_downloads(phone: State<'_, Phone>) -> BTreeMap<String, SaveState> {
    phone.saved.lock().unwrap().clone()
}

/// Open a saved single file, or the Downloads view for a saved folder.
#[tauri::command]
pub async fn open_download(app: AppHandle, id: String) -> CmdResult<()> {
    let files = saved_files(&app, &id)?;
    tauri::async_runtime::spawn_blocking(move || match files.as_slice() {
        [one] => app.ferry_mobile().open(&one.uri),
        _ => app.ferry_mobile().show_downloads(),
    })
    .await
    .map_err(fail)?
}

/// Offer a saved download's files to another app.
#[tauri::command]
pub async fn share_download(app: AppHandle, id: String) -> CmdResult<()> {
    let uris: Vec<String> = saved_files(&app, &id)?.into_iter().map(|f| f.uri).collect();
    tauri::async_runtime::spawn_blocking(move || app.ferry_mobile().share(&uris))
        .await
        .map_err(fail)?
}

/// Try saving a download to Downloads again after it failed.
#[tauri::command]
pub fn retry_save(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let download = state.node.download(&id).ok_or("unknown download")?;
    if download.status != DownloadStatus::Completed {
        return Err("This download hasn't finished yet.".into());
    }
    save(app, download);
    Ok(())
}

fn saved_files(app: &AppHandle, id: &str) -> CmdResult<Vec<SavedFile>> {
    match app.state::<Phone>().get(id) {
        Some(SaveState::Saved { files, .. }) => Ok(files),
        _ => Err("This download hasn't been saved to Downloads yet.".into()),
    }
}
