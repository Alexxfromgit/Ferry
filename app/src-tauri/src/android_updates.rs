//! Updates on Android, behind the same commands as the desktop updater (`updates.rs`) so the UI
//! is shared. The work happens in Kotlin, in `UpdatePlugin` of the app module, which each
//! channel implements its own way:
//!
//! - **direct** (`gen/android/app/src/direct`): checks downloads.xenvaultinteractive.com,
//!   downloads and verifies a newer APK, and hands it to Android's installer.
//! - **play** (`gen/android/app/src/play`): Google Play's in-app updates.

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager, Runtime, State, Wry,
    plugin::{Builder, PluginHandle, TauriPlugin},
};

use crate::{AppState, CmdResult, android::Phone, fail, save_settings};

struct Updater<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("ferry-updates")
        .setup(|app, api| {
            let handle =
                api.register_android_plugin("com.xenvaultinteractive.ferry", "UpdatePlugin")?;
            app.manage(Updater(handle));
            Ok(())
        })
        .build()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum UpdateSupport {
    /// Ferry downloads and installs updates itself.
    Available,
    /// Google Play delivers updates; Ferry asks it to.
    Store,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    support: UpdateSupport,
    auto_check: bool,
    ready_version: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ready {
    ready_version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckArgs {
    automatic: bool,
    /// Whether an update may install right away (Play's immediate flow restarts the app).
    allow_immediate: bool,
}

/// Whether installing now would cut off work: a transfer either way, files being copied into
/// Downloads, or a share being prepared.
fn busy(app: &AppHandle, state: &AppState) -> bool {
    state
        .node
        .list_downloads()
        .iter()
        .any(|download| download.status.is_active())
        || app.try_state::<Phone>().is_some_and(|phone| phone.busy())
}

/// Run an `UpdatePlugin` command on a blocking thread (the call waits for Android's answer).
async fn call<T: serde::de::DeserializeOwned + Send + 'static>(
    app: &AppHandle,
    command: &'static str,
    args: impl Serialize + Send + 'static,
) -> CmdResult<T> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<Updater<Wry>>()
            .0
            .run_mobile_plugin(command, args)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(fail)?
}

fn support() -> UpdateSupport {
    if env!("FERRY_DISTRIBUTION") == "direct" {
        UpdateSupport::Available
    } else {
        UpdateSupport::Store
    }
}

#[tauri::command]
pub async fn update_info(app: AppHandle, state: State<'_, AppState>) -> CmdResult<UpdateInfo> {
    let auto_check = state.settings.lock().unwrap().auto_check_updates;
    let ready: Ready = call(&app, "ready", ()).await.unwrap_or_default();
    Ok(UpdateInfo {
        version: app.package_info().version.to_string(),
        support: support(),
        auto_check,
        ready_version: ready.ready_version,
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
    automatic: bool,
) -> CmdResult<Option<String>> {
    if automatic && !state.settings.lock().unwrap().auto_check_updates {
        return Ok(None);
    }
    let args = CheckArgs {
        automatic,
        allow_immediate: !busy(&app, &state),
    };
    let ready: Ready = call(&app, "check", args).await.map_err(|error| {
        tracing::warn!("update check failed: {error}");
        "Ferry couldn't check for updates. Try again in a moment.".to_owned()
    })?;
    Ok(ready.ready_version)
}

#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    // Installing replaces the running app, which would cut off a transfer.
    if busy(&app, &state) {
        return Err("Wait for active transfers to finish before updating.".into());
    }
    call::<serde::de::IgnoredAny>(&app, "install", ())
        .await
        .map(drop)
}
