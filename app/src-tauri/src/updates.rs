use std::{path::PathBuf, time::Duration};

use ferry_core::DownloadStatus;
use serde::Serialize;
use tauri::{AppHandle, Manager, State, utils::platform::bundle_type};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::{AppState, CmdResult, fail, save_settings};

/// Serialize checks, cache writes and installs.
#[derive(Default)]
pub struct PendingUpdate(tokio::sync::Mutex<()>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum UpdateSupport {
    Available,
    PackageManager,
    Unavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    support: UpdateSupport,
    auto_check: bool,
    ready_version: Option<String>,
}

fn support(app: &AppHandle) -> UpdateSupport {
    use tauri::utils::config::BundleType;

    if bundle_type() == Some(BundleType::Deb) {
        return UpdateSupport::PackageManager;
    }
    let has_key = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config["pubkey"].as_str())
        .is_some_and(|key| !key.is_empty());
    if cfg!(debug_assertions) || !has_key {
        return UpdateSupport::Unavailable;
    }

    let installed = match bundle_type() {
        Some(BundleType::Msi | BundleType::Nsis) => cfg!(target_arch = "x86_64"),
        #[cfg(target_os = "linux")]
        Some(BundleType::AppImage) => cfg!(target_arch = "x86_64") && app.env().appimage.is_some(),
        #[cfg(target_os = "macos")]
        Some(BundleType::App) => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.ends_with("Contents/MacOS")))
            .unwrap_or(false),
        _ => false,
    };
    if installed {
        UpdateSupport::Available
    } else {
        UpdateSupport::Unavailable
    }
}

fn updater(app: &AppHandle) -> CmdResult<Updater> {
    let builder = app.updater_builder().timeout(Duration::from_secs(20));
    // Tauri runs this hook before starting the Windows installer. Keep the window and
    // tray usable if Windows refuses to launch it (for example, on a canceled UAC prompt).
    #[cfg(windows)]
    let builder = builder.on_before_exit(|| {});
    builder.build().map_err(fail)
}

struct UpdateCache(PathBuf);

impl UpdateCache {
    fn new(app: &AppHandle) -> CmdResult<Self> {
        Ok(Self(
            app.path().app_cache_dir().map_err(fail)?.join("update"),
        ))
    }

    fn load(&self, updater: &Updater) -> Option<Update> {
        let update = std::fs::read(self.0.join("manifest.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .and_then(|manifest| updater.restore_update(manifest).ok().flatten());
        if update.is_some() && self.0.join("package").is_file() {
            return update;
        }
        self.clear();
        None
    }

    fn save(&self, update: &Update, package: &[u8]) -> CmdResult<()> {
        std::fs::create_dir_all(&self.0).map_err(fail)?;
        ferry_core::write_atomic(&self.0.join("package"), package).map_err(fail)?;
        let manifest = serde_json::to_vec(&update.raw_json).map_err(fail)?;
        ferry_core::write_atomic(&self.0.join("manifest.json"), &manifest).map_err(fail)
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(self.0.join("manifest.json"));
        let _ = std::fs::remove_file(self.0.join("package"));
    }
}

#[tauri::command]
pub async fn update_info(
    app: AppHandle,
    state: State<'_, AppState>,
    pending: State<'_, PendingUpdate>,
) -> CmdResult<UpdateInfo> {
    let _guard = pending.0.lock().await;
    let support = support(&app);
    let ready_version = if matches!(support, UpdateSupport::Available) {
        updater(&app).ok().and_then(|updater| {
            UpdateCache::new(&app)
                .ok()?
                .load(&updater)
                .map(|update| update.version)
        })
    } else {
        None
    };
    Ok(UpdateInfo {
        version: app.package_info().version.to_string(),
        support,
        auto_check: state.settings.lock().unwrap().auto_check_updates,
        ready_version,
    })
}

#[tauri::command]
pub fn set_auto_check_updates(state: State<'_, AppState>, enabled: bool) -> CmdResult<()> {
    let mut saved = state.settings.lock().unwrap();
    let mut next = saved.clone();
    next.auto_check_updates = enabled;
    save_settings(&state.settings_path, &next)?;
    *saved = next;
    Ok(())
}

#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
    state: State<'_, AppState>,
    pending: State<'_, PendingUpdate>,
    automatic: bool,
) -> CmdResult<Option<String>> {
    let _guard = pending.0.lock().await;
    if !matches!(support(&app), UpdateSupport::Available)
        || (automatic && !state.settings.lock().unwrap().auto_check_updates)
    {
        return Ok(None);
    }

    let result = check_and_download(&app).await;
    result.map_err(|error| {
        tracing::warn!("update check failed: {error}");
        "Ferry couldn't check for or download updates. Try again in a moment.".into()
    })
}

async fn check_and_download(app: &AppHandle) -> CmdResult<Option<String>> {
    let updater = updater(app)?;
    let cache = UpdateCache::new(app)?;
    if let Some(update) = cache.load(&updater) {
        return Ok(Some(update.version));
    }
    let Some(mut update) = updater.check().await.map_err(fail)? else {
        return Ok(None);
    };
    update.timeout = Some(Duration::from_secs(600));
    let package = update.download(|_, _| {}, || {}).await.map_err(fail)?;
    let version = update.version.clone();
    tauri::async_runtime::spawn_blocking(move || cache.save(&update, &package))
        .await
        .map_err(fail)??;
    Ok(Some(version))
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    state: State<'_, AppState>,
    pending: State<'_, PendingUpdate>,
) -> CmdResult<()> {
    let _guard = pending.0.lock().await;
    if !matches!(support(&app), UpdateSupport::Available) {
        return Err("This copy of Ferry can't install updates.".into());
    }
    if state.node.list_downloads().iter().any(|download| {
        matches!(
            download.status,
            DownloadStatus::Connecting | DownloadStatus::Downloading | DownloadStatus::Exporting
        )
    }) {
        return Err("Wait for active transfers to finish before restarting.".into());
    }
    #[cfg(windows)]
    state.node.flush_store().await.map_err(fail)?;

    let updater = updater(&app)?;
    let cache = UpdateCache::new(&app)?;
    tauri::async_runtime::spawn_blocking(move || -> CmdResult<()> {
        let update = cache
            .load(&updater)
            .ok_or("No update is ready to install.")?;
        let package = std::fs::read(cache.0.join("package")).map_err(|error| {
            cache.clear();
            fail(error)
        })?;
        // Reverify the package during install. Discard an unreadable or invalid cache
        // so the next check can download a replacement.
        if let Err(error) = update.install(package) {
            use tauri_plugin_updater::Error;
            if matches!(
                error,
                Error::Minisign(_)
                    | Error::Base64(_)
                    | Error::SignatureUtf8(_)
                    | Error::SignedVersionMismatch { .. }
                    | Error::MissingSignedVersion
                    | Error::InvalidUpdaterFormat
            ) {
                cache.clear();
            }
            tracing::warn!("update installation failed: {error}");
            return Err(
                "Ferry couldn't install the update. Try again or download it from GitHub.".into(),
            );
        }
        Ok(())
    })
    .await
    .map_err(fail)??;
    app.request_restart();
    Ok(())
}
