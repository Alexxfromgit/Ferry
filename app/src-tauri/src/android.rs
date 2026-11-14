//! The phone side of receiving. Downloads land in app storage first (`<app data>/incoming`);
//! once one completes, its files are copied into the shared Downloads collection under
//! `Download/Ferry` and the app's copy is deleted. While anything is transferring or being
//! saved, or any share is on (see `android_send.rs`), a foreground service with a notification
//! keeps Ferry running with the screen off.
//!
//! Saving is tracked per download in `saved.json`. Each save is an *attempt*: only one runs per
//! download at a time, and only the current one may record its outcome, so a slow or
//! superseded attempt can never overwrite a newer state. Files copied so far are recorded as
//! the attempt goes, so a retry or a restart continues instead of copying them again, and the
//! app's copy is deleted only once the outcome is on disk.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use ferry_core::{DownloadInfo, DownloadStatus, Event, ShareInfo};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_ferry_mobile::{FerryMobileExt, SavedFile, TransferStatus};
use tokio::sync::watch;

use crate::{AppState, CmdResult, fail, phone};

/// Tells the UI that a download's saved state changed (payload: the download id).
const SAVED_EVENT: &str = "ferry-saved";
/// How often the progress notification may change while the set of transfers stays the same.
const NOTIFY_EVERY: Duration = Duration::from_secs(1);
/// How often a running save records the files it has copied so far.
const RECORD_PROGRESS_EVERY: Duration = Duration::from_secs(1);

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

/// What `saved.json` keeps for one download.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(flatten)]
    state: SaveState,
    /// `DownloadInfo::created_at` of the download this belongs to. Receiving the same share
    /// again reuses the download id, and must not inherit this record.
    #[serde(default)]
    created_at: u64,
    /// The download's files and folders in app storage (see [`phone::saved_items`]), so they
    /// can be deleted even after the download is gone.
    #[serde(default)]
    items: Vec<PathBuf>,
    /// Files already copied into Downloads, by their path in app storage.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    published: BTreeMap<String, SavedFile>,
}

impl Record {
    fn belongs_to(&self, download: &DownloadInfo) -> bool {
        // 0: written before the field existed.
        self.created_at == 0 || self.created_at == download.created_at
    }
}

#[derive(Default)]
struct Saves {
    records: BTreeMap<String, Record>,
    /// The attempt currently saving each download, if any.
    running: BTreeMap<String, u64>,
    next_attempt: u64,
}

impl Saves {
    fn is_current(&self, id: &str, attempt: u64) -> bool {
        self.running.get(id) == Some(&attempt)
    }
}

pub struct Phone {
    path: PathBuf,
    /// Held while `saved.json` is written, so the newest state is always written last.
    saves: Mutex<Saves>,
    transfers: Mutex<Transfers>,
    /// The notification Ferry wants shown. One worker applies it (see [`start`]), so Android
    /// sees the changes in order and a stale one can't restart or stop the service.
    notification: watch::Sender<Option<TransferStatus>>,
}

#[derive(Default)]
struct Transfers {
    active: BTreeMap<String, DownloadInfo>,
    /// Running saves, by attempt, with the download's name.
    saving: BTreeMap<u64, String>,
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
    /// Load what was saved before. Saves interrupted by the app closing continue in [`start`].
    pub fn load(path: PathBuf) -> Self {
        let records: BTreeMap<String, Record> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            saves: Mutex::new(Saves {
                records,
                ..Saves::default()
            }),
            transfers: Mutex::default(),
            notification: watch::Sender::new(None),
        }
    }

    /// Write the records while still holding their lock.
    fn write(&self, saves: &Saves) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&saves.records).expect("saved state serializes");
        ferry_core::write_atomic(&self.path, &json).map_err(|e| format!("{e:#}"))
    }

    /// Write the records, release the lock and tell the UI about `id`.
    fn commit(&self, app: &AppHandle, saves: MutexGuard<'_, Saves>, id: &str) {
        if let Err(e) = self.write(&saves) {
            tracing::warn!("could not record saved downloads: {e}");
        }
        drop(saves);
        app.emit(SAVED_EVENT, id).ok();
    }

    fn get(&self, id: &str) -> Option<SaveState> {
        let saves = self.saves.lock().unwrap();
        saves.records.get(id).map(|r| r.state.clone())
    }

    /// Whether files are being copied into Downloads. Counts as active work: it keeps the
    /// foreground service up and holds back updates.
    pub fn saving(&self) -> bool {
        !self.transfers.lock().unwrap().saving.is_empty()
    }
}

/// Start following the phone side: the notification worker, then saves left unfinished when
/// Ferry last closed.
pub fn start(app: &AppHandle) {
    let phone = app.state::<Phone>();
    let mut wanted = phone.notification.subscribe();
    // Anything asked for before the worker existed.
    wanted.mark_changed();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while wanted.changed().await.is_ok() {
            let status = wanted.borrow_and_update().clone();
            let app = handle.clone();
            // Wait for each change before taking the next: the latest one always lands last.
            let applied = tauri::async_runtime::spawn_blocking(move || {
                app.ferry_mobile().set_transfer(status.as_ref())
            })
            .await;
            match applied {
                Ok(Err(e)) => tracing::warn!("could not update the transfer notification: {e}"),
                Err(e) => tracing::warn!("could not update the transfer notification: {e}"),
                Ok(Ok(())) => {}
            }
        }
    });
    resume(app, true);
}

/// Bring the phone side in line with the downloads: continue unfinished saves, delete files
/// left in app storage by saves that finished or downloads that are gone. Runs at startup and
/// whenever core events were missed; only at startup (`retry_failed`) are failed saves tried
/// again.
pub fn resume(app: &AppHandle, retry_failed: bool) {
    let (Some(state), Some(phone)) = (app.try_state::<AppState>(), app.try_state::<Phone>()) else {
        return;
    };
    let downloads = state.node.list_downloads();
    {
        let mut transfers = phone.transfers.lock().unwrap();
        transfers.active = downloads
            .iter()
            .filter(|d| d.status.is_active())
            .map(|d| (d.id.clone(), d.clone()))
            .collect();
        // Shares that were on when Ferry last closed are served again: keep the phone
        // reachable. Shares turned off or removed while events were missed are dropped.
        let shares = state.node.list_shares();
        transfers
            .sharing
            .retain(|id, _| shares.iter().any(|s| &s.id == id && !s.paused));
        for share in &shares {
            transfers.track_share(share);
        }
        update_notification(app, &mut transfers, true);
    }
    // Records of downloads removed while Ferry wasn't listening.
    let orphaned: Vec<String> = {
        let saves = phone.saves.lock().unwrap();
        saves
            .records
            .keys()
            .filter(|id| !downloads.iter().any(|d| &d.id == *id))
            .cloned()
            .collect()
    };
    for id in orphaned {
        forget(app, &id, &[]);
    }
    for download in downloads {
        if download.status != DownloadStatus::Completed {
            continue;
        }
        let items = phone::saved_items(&download);
        let (leftovers, failed) = {
            let saves = phone.saves.lock().unwrap();
            let record = saves
                .records
                .get(&download.id)
                .filter(|r| r.belongs_to(&download));
            let failed = matches!(record.map(|r| &r.state), Some(SaveState::Failed { .. }));
            let leftovers = match record {
                // Ferry closed between recording the save and deleting its copy.
                Some(
                    record @ Record {
                        state: SaveState::Saved { .. },
                        ..
                    },
                ) if !saves.running.contains_key(&download.id) => Some(record.items.clone()),
                _ => None,
            };
            (leftovers, failed)
        };
        match leftovers {
            Some(items) => delete_items(items),
            None if failed && !retry_failed => {}
            None if items.iter().any(|item| item.exists()) => {
                // Failed or interrupted saves are tried again; a running one is left alone.
                start_save(app, download, true).ok();
            }
            None => {}
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
            if download.status.is_active() {
                // Only a new download of the same share makes a completed one active again: the
                // earlier record and whatever it left in app storage go.
                let stale = phone
                    .saves
                    .lock()
                    .unwrap()
                    .records
                    .contains_key(&download.id);
                if stale {
                    forget(app, &download.id, &phone::saved_items(download));
                }
            } else if download.status == DownloadStatus::Completed {
                // Already saved, failed (the user retries) or running: nothing to do.
                start_save(app, download.clone(), false).ok();
            }
        }
        Event::DownloadRemoved { download_id } => {
            let mut transfers = phone.transfers.lock().unwrap();
            let changed = transfers.active.remove(download_id).is_some();
            update_notification(app, &mut transfers, changed);
            drop(transfers);
            // Saved files stay in Downloads; anything still in app storage is deleted.
            forget(app, download_id, &[]);
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

/// Drop the record of download `id`. A running save is no longer current: it finishes copying
/// and then deletes its source itself. Otherwise what the record's download left in app
/// storage is deleted now, except `keep` (the items of a newer download under the same id).
fn forget(app: &AppHandle, id: &str, keep: &[PathBuf]) {
    let phone = app.state::<Phone>();
    let mut saves = phone.saves.lock().unwrap();
    let Some(record) = saves.records.remove(id) else {
        return;
    };
    let running = saves.running.remove(id).is_some();
    phone.commit(app, saves, id);
    if !running {
        delete_items(
            record
                .items
                .into_iter()
                .filter(|item| !keep.contains(item))
                .collect(),
        );
    }
}

/// Delete files and folders from app storage, off the calling thread.
fn delete_items(items: Vec<PathBuf>) {
    if items.is_empty() {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        for item in items {
            let removed = if item.is_dir() {
                std::fs::remove_dir_all(&item)
            } else {
                std::fs::remove_file(&item)
            };
            match removed {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    tracing::warn!("could not delete {}: {e}", item.display());
                }
                _ => {}
            }
        }
    });
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
    if let Some(phone) = app.try_state::<Phone>() {
        phone.notification.send_replace(next);
    }
}

/// The notification: receiving takes precedence, then saving into Downloads, then shares that
/// are on. `None` when there is none of these, which stops the foreground service.
fn transfer_status(transfers: &Transfers) -> Option<TransferStatus> {
    let sharing = !transfers.sharing.is_empty();
    phone::transfer_text(transfers.active.values())
        .or_else(|| {
            phone::saving_text(transfers.saving.values().map(String::as_str))
                .map(|(title, text)| (title, text, None))
        })
        .or_else(|| {
            phone::sharing_text(transfers.sharing.values()).map(|(title, text)| (title, text, None))
        })
        .map(|(title, text, progress)| TransferStatus {
            title,
            text,
            progress,
            sharing,
        })
}

/// Start saving a completed download into Downloads/Ferry. Does nothing if it is saved
/// already, or (unless `retry`) if saving it failed before. Fails if it is being saved.
fn start_save(app: &AppHandle, download: DownloadInfo, retry: bool) -> Result<(), String> {
    let phone = app.state::<Phone>();
    let mut saves = phone.saves.lock().unwrap();
    if saves.running.contains_key(&download.id) {
        return Err("Ferry is already saving this download.".into());
    }
    let previous = saves.records.remove(&download.id);
    let (previous, stale) = match previous {
        Some(record) if record.belongs_to(&download) => (Some(record), None),
        other => (None, other),
    };
    match previous.as_ref().map(|r| &r.state) {
        Some(SaveState::Saved { .. }) => {
            saves
                .records
                .insert(download.id.clone(), previous.expect("matched above"));
            return Ok(());
        }
        Some(SaveState::Failed { .. }) if !retry => {
            saves
                .records
                .insert(download.id.clone(), previous.expect("matched above"));
            return Ok(());
        }
        _ => {}
    }
    let items = phone::saved_items(&download);
    saves.next_attempt += 1;
    let attempt = saves.next_attempt;
    saves.running.insert(download.id.clone(), attempt);
    saves.records.insert(
        download.id.clone(),
        Record {
            state: SaveState::Saving,
            created_at: download.created_at,
            items: items.clone(),
            // Files the last attempt copied before it stopped.
            published: previous.map(|r| r.published).unwrap_or_default(),
        },
    );
    phone.commit(app, saves, &download.id);
    if let Some(stale) = stale {
        // The record of an earlier download of the same share that was missed.
        delete_items(
            stale
                .items
                .into_iter()
                .filter(|item| !items.contains(item))
                .collect(),
        );
    }
    {
        let mut transfers = phone.transfers.lock().unwrap();
        transfers.saving.insert(attempt, download.name.clone());
        update_notification(app, &mut transfers, true);
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || run_save(&app, &download, attempt));
    Ok(())
}

/// Copy the files, record the outcome if this attempt is still current, then delete the app's
/// copy once it is no longer needed.
fn run_save(app: &AppHandle, download: &DownloadInfo, attempt: u64) {
    let phone = app.state::<Phone>();
    let mut published = {
        let saves = phone.saves.lock().unwrap();
        saves
            .records
            .get(&download.id)
            .filter(|_| saves.is_current(&download.id, attempt))
            .map(|r| r.published.clone())
            .unwrap_or_default()
    };
    let outcome = copy_files(app, download, attempt, &mut published);
    if let Err(error) = &outcome {
        tracing::warn!(
            "could not save download {} to Downloads: {error}",
            download.id
        );
    }
    let mut items = phone::saved_items(download);
    let mut saves = phone.saves.lock().unwrap();
    let delete = if saves.is_current(&download.id, attempt) {
        saves.running.remove(&download.id);
        let record = saves
            .records
            .get_mut(&download.id)
            .expect("a running save has a record");
        let saved = outcome.is_ok();
        match outcome {
            Ok(state) => {
                record.state = state;
                record.published.clear();
            }
            Err(error) => {
                record.state = SaveState::Failed { error };
                record.published = published.clone();
            }
        }
        let written = phone.write(&saves);
        let delete = match written {
            Ok(()) => saved,
            Err(e) if saved => {
                // Keep the app's copy until the saved files are on record; trying again
                // records them without copying anything twice.
                tracing::warn!("could not record saved download {}: {e}", download.id);
                let record = saves.records.get_mut(&download.id).expect("still there");
                record.state = SaveState::Failed {
                    error: "Ferry couldn't record the saved files.".into(),
                };
                record.published = published;
                false
            }
            Err(e) => {
                tracing::warn!("could not record saved downloads: {e}");
                false
            }
        };
        drop(saves);
        app.emit(SAVED_EVENT, &download.id).ok();
        delete
    } else {
        drop(saves);
        // Removed, or received again, while saving: nobody will save these files again. A
        // newer download under the same id keeps its own.
        let newer = app
            .state::<AppState>()
            .node
            .download(&download.id)
            .filter(|d| d.created_at != download.created_at);
        if let Some(newer) = newer {
            let keep = phone::saved_items(&newer);
            items.retain(|item| !keep.contains(item));
        }
        true
    };
    if delete {
        delete_items(items);
    }
    let mut transfers = phone.transfers.lock().unwrap();
    transfers.saving.remove(&attempt);
    update_notification(app, &mut transfers, true);
}

/// Copy a completed download's files into Downloads/Ferry, skipping those in `published` and
/// adding each one copied.
fn copy_files(
    app: &AppHandle,
    download: &DownloadInfo,
    attempt: u64,
    published: &mut BTreeMap<String, SavedFile>,
) -> Result<SaveState, String> {
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
    let mut recorded_at = Instant::now();
    for file in &files {
        let path = file.to_str().ok_or("A file name isn't valid Unicode.")?;
        if let Some(done) = published.get(path) {
            saved.push(done.clone());
            continue;
        }
        let dir = phone::relative_dir(root, file)?;
        let done = mobile.save_to_downloads(path, &dir)?;
        published.insert(path.to_owned(), done.clone());
        saved.push(done);
        if recorded_at.elapsed() >= RECORD_PROGRESS_EVERY {
            record_progress(app, &download.id, attempt, published);
            recorded_at = Instant::now();
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

/// Record which files a running save has copied, so Ferry closing doesn't make it start over.
fn record_progress(
    app: &AppHandle,
    id: &str,
    attempt: u64,
    published: &BTreeMap<String, SavedFile>,
) {
    let phone = app.state::<Phone>();
    let mut saves = phone.saves.lock().unwrap();
    if !saves.is_current(id, attempt) {
        return;
    }
    if let Some(record) = saves.records.get_mut(id) {
        record.published = published.clone();
        if let Err(e) = phone.write(&saves) {
            tracing::warn!("could not record saving progress: {e}");
        }
    }
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
    let saves = phone.saves.lock().unwrap();
    saves
        .records
        .iter()
        .map(|(id, record)| (id.clone(), record.state.clone()))
        .collect()
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
    start_save(&app, download, true)
}

fn saved_files(app: &AppHandle, id: &str) -> CmdResult<Vec<SavedFile>> {
    match app.state::<Phone>().get(id) {
        Some(SaveState::Saved { files, .. }) => Ok(files),
        _ => Err("This download hasn't been saved to Downloads yet.".into()),
    }
}
