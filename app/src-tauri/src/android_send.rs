//! The phone side of sending. Android hands out picked files, and files shared to Ferry from
//! other apps, as `content://` URIs, which may stop working once the activity that received
//! them is gone. Ferry copies them into its own storage (`<app data>/outgoing/<n>`) and shares
//! that copy, which is served in place and deleted when the share is removed.
//!
//! Shares keep the phone reachable: while any is on, the transfer notification (and its
//! foreground service) stays up with a "Stop sharing" button; see `android.rs`.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use ferry_core::ShareInfo;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_ferry_mobile::{FerryMobileExt, MobileEvent};

use crate::{AppState, CmdResult, fail};

/// Files shared to Ferry from another app are waiting (payload: none; see `take_incoming`).
const INCOMING_EVENT: &str = "ferry-incoming";
/// Progress of copying picked files into Ferry's storage (payload: [`StageProgress`]).
const STAGING_EVENT: &str = "ferry-staging";
/// Something to tell the user that didn't come from a command (payload: the text).
const NOTICE_EVENT: &str = "ferry-notice";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageProgress {
    done_bytes: u64,
    total_bytes: u64,
}

/// What the phone's Send screen picks.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PickKind {
    Files,
    Folder,
}

/// The copies behind each share, and files shared to Ferry that the UI hasn't taken yet.
pub struct Outgoing {
    /// `outgoing.json`: share id → folder name under `dir`.
    path: PathBuf,
    dir: PathBuf,
    copies: Mutex<BTreeMap<String, String>>,
    incoming: Mutex<Vec<Vec<String>>>,
    next: AtomicU64,
}

impl Outgoing {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("outgoing.json");
        let copies = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            dir: data_dir.join("outgoing"),
            copies: Mutex::new(copies),
            incoming: Mutex::default(),
            next: AtomicU64::new(0),
        }
    }

    fn save(&self, copies: &BTreeMap<String, String>) {
        let json = serde_json::to_vec_pretty(copies).expect("outgoing copies serialize");
        if let Err(e) = ferry_core::write_atomic(&self.path, &json) {
            tracing::warn!("could not record shared copies: {e}");
        }
    }

    /// A new, empty folder for one share's copies.
    fn new_dir(&self) -> std::io::Result<(String, PathBuf)> {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let name = format!("{millis}-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let dir = self.dir.join(&name);
        std::fs::create_dir_all(&dir)?;
        Ok((name, dir))
    }

    /// Remember that `share_id` serves the copies in `name`. If the same files were already
    /// shared (same id), the earlier copies keep serving and these are deleted.
    fn record(&self, share_id: &str, name: String) {
        let mut copies = self.copies.lock().unwrap();
        if copies.contains_key(share_id) {
            remove_dir(&self.dir.join(name));
            return;
        }
        copies.insert(share_id.to_owned(), name);
        self.save(&copies);
    }

    /// Delete the copies behind a removed share.
    pub fn forget(&self, share_id: &str) {
        let mut copies = self.copies.lock().unwrap();
        if let Some(name) = copies.remove(share_id) {
            self.save(&copies);
            remove_dir(&self.dir.join(name));
        }
    }

    /// Delete copies no share uses: left over from a share removed while Ferry was closing,
    /// or from a copy interrupted part way.
    fn tidy(&self, live: &HashSet<String>) {
        let mut copies = self.copies.lock().unwrap();
        let before = copies.len();
        copies.retain(|id, _| live.contains(id));
        if copies.len() != before {
            self.save(&copies);
        }
        let used: HashSet<&String> = copies.values().collect();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !used.contains(&name) {
                    remove_dir(&entry.path());
                }
            }
        }
    }
}

fn remove_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("could not delete {}: {e}", dir.display());
    }
}

/// At startup: tidy up copies, and start listening to Android (Share menu, the notification's
/// Stop button, Android's time limit, copy progress).
pub fn start(app: &AppHandle, data_dir: &Path) {
    let outgoing = Outgoing::load(data_dir);
    let live = app
        .state::<AppState>()
        .node
        .list_shares()
        .into_iter()
        .map(|s| s.id)
        .collect();
    outgoing.tidy(&live);
    app.manage(outgoing);

    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let events = app.clone();
        if let Err(e) = app
            .ferry_mobile()
            .watch(move |event| on_mobile_event(&events, event))
        {
            tracing::warn!("could not listen to Android: {e}");
        }
    });
}

fn on_mobile_event(app: &AppHandle, event: MobileEvent) {
    match event {
        MobileEvent::Shared { uris } => {
            app.state::<Outgoing>().incoming.lock().unwrap().push(uris);
            app.emit(INCOMING_EVENT, ()).ok();
        }
        MobileEvent::SharedNothing => {
            app.emit(NOTICE_EVENT, "Ferry can only share files.").ok();
        }
        MobileEvent::StopSharing => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { turn_off_shares(&app).await });
        }
        MobileEvent::Timeout => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                turn_off_shares(&app).await;
                app.emit(
                    NOTICE_EVENT,
                    "Android ended background sharing for today, so Ferry turned your shares off. \
                     Turn them on again to keep sharing.",
                )
                .ok();
            });
        }
        MobileEvent::StageProgress {
            done_bytes,
            total_bytes,
        } => {
            app.emit(
                STAGING_EVENT,
                StageProgress {
                    done_bytes,
                    total_bytes,
                },
            )
            .ok();
        }
    }
}

/// Turn every share off. The links stay valid and work again once a share is turned on.
async fn turn_off_shares(app: &AppHandle) {
    let node = app.state::<AppState>().node.clone();
    for share in node.list_shares().into_iter().filter(|s| !s.paused) {
        if let Err(e) = node.set_share_paused(&share.id, true).await {
            tracing::warn!("could not turn off {}: {e:#}", share.name);
        }
    }
}

/// Let the user pick files or a folder. Resolves with what to pass to [`share_items`], empty
/// when they cancel.
#[tauri::command]
pub async fn pick_items(app: AppHandle, kind: PickKind) -> CmdResult<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || match kind {
        PickKind::Files => app.ferry_mobile().pick_files(),
        PickKind::Folder => app
            .ferry_mobile()
            .pick_folder()
            .map(|f| f.into_iter().collect()),
    })
    .await
    .map_err(fail)?
    .map_err(|e| e.to_string())
}

/// Files shared to Ferry from other apps, oldest first, each batch once.
#[tauri::command]
pub fn take_incoming(outgoing: State<'_, Outgoing>) -> Vec<Vec<String>> {
    std::mem::take(&mut *outgoing.incoming.lock().unwrap())
}

/// Copy picked or shared items into Ferry's storage and share them.
#[tauri::command]
pub async fn share_items(app: AppHandle, uris: Vec<String>) -> CmdResult<ShareInfo> {
    if uris.is_empty() {
        return Err("Nothing to share.".into());
    }
    let outgoing = app.state::<Outgoing>();
    let (name, dir) = outgoing.new_dir().map_err(fail)?;
    let result = stage_and_share(&app, uris, &dir).await;
    match &result {
        Ok(share) => outgoing.record(&share.id, name),
        Err(_) => remove_dir(&dir),
    }
    result
}

async fn stage_and_share(app: &AppHandle, uris: Vec<String>, dir: &Path) -> CmdResult<ShareInfo> {
    let dest = dir.to_string_lossy().into_owned();
    let mobile = app.clone();
    let staged =
        tauri::async_runtime::spawn_blocking(move || mobile.ferry_mobile().stage(&uris, &dest))
            .await
            .map_err(fail)??;
    let paths = staged.paths.into_iter().map(PathBuf::from).collect();
    let node = app.state::<AppState>().node.clone();
    node.create_share(paths).await.map_err(fail)
}

/// Offer a share's link to other apps (messengers, email…) through the share sheet.
#[tauri::command]
pub async fn share_link(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let share = state
        .node
        .share(&id)
        .ok_or("This share no longer exists.")?;
    tauri::async_runtime::spawn_blocking(move || {
        app.ferry_mobile()
            .share_text(&share.link, Some(&share.name))
    })
    .await
    .map_err(fail)?
    .map_err(|e| e.to_string())
}
