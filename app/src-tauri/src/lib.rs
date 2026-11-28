//! Ferry desktop shell: a thin Tauri layer over `ferry-core`.
//!
//! Commands map 1:1 to `Node` methods; core events are forwarded to the webview as
//! `ferry-event`. Incoming links (`ferry://` or web deep links, `.ferry` files opened from the
//! file manager, or a second app launch) are queued and announced with `ferry-open-link`. When
//! the UI may have missed events, `ferry-resync` tells it to reload its lists.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_send;
#[cfg(target_os = "android")]
mod android_updates;
mod phone;
#[cfg(desktop)]
mod updates;

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use ferry_core::{
    AccessError, Config, DownloadInfo, DownloadStatus, Event, Node, NodeStatus, Preview,
    RelayChoice, SHARE_FILE_EXTENSION, ShareInfo, ShareLink,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent, path::BaseDirectory};
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

const EVENT: &str = "ferry-event";
const OPEN_LINK: &str = "ferry-open-link";
const RESYNC: &str = "ferry-resync";
const MAX_LOG: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    download_dir: PathBuf,
    relay: RelayChoice,
    close_to_tray: bool,
    #[serde(default = "default_auto_check_updates")]
    auto_check_updates: bool,
    /// Show the mascot artwork behind the UI. Missing from settings saved before it existed.
    #[serde(default = "yes")]
    show_artwork: bool,
    /// Make short links through Ferry's link service (and open them).
    #[serde(default = "yes")]
    short_links: bool,
    /// Connect to devices elsewhere only through the relay, so they never learn this
    /// device's IP address.
    #[serde(default)]
    hide_ip: bool,
}

fn default_auto_check_updates() -> bool {
    true
}

fn yes() -> bool {
    true
}

/// Where "Source code" in Settings → About leads. Fixed here so the webview can't open other URLs.
const PROJECT_PAGE: &str = "https://github.com/Alexxfromgit/Ferry";

struct AppState {
    node: Node,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    pending_links: Mutex<Vec<IncomingShare>>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum IncomingShare {
    Link { link: String },
    File { path: PathBuf },
}

/// A preview, or why the sender wants a (different) password first.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum PreviewOutcome {
    Ready {
        preview: Preview,
    },
    PasswordRequired,
    WrongPassword,
    #[serde(rename_all = "camelCase")]
    TooManyGuesses {
        retry_after_secs: u64,
    },
}

type CmdResult<T> = Result<T, String>;

fn fail(e: impl Into<anyhow::Error>) -> String {
    format!("{:#}", e.into())
}

fn load_settings(path: &Path, default_download_dir: PathBuf) -> Settings {
    let defaults = Settings {
        download_dir: default_download_dir,
        relay: RelayChoice::Default,
        close_to_tray: true,
        auto_check_updates: default_auto_check_updates(),
        show_artwork: true,
        short_links: true,
        hide_ip: false,
    };
    let Some(mut settings) = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
    else {
        return defaults;
    };
    if !settings.download_dir.is_absolute() {
        settings.download_dir = defaults.download_dir;
    }
    settings
}

/// Settings coming from the UI must be usable, so a typo can't break the next start.
fn validate_settings(settings: &Settings) -> Result<(), String> {
    settings.relay.validate().map_err(fail)?;
    if settings.hide_ip && settings.relay == RelayChoice::Disabled {
        return Err(
            "Hiding your IP address needs relays. Choose public relays or your own first.".into(),
        );
    }
    if !settings.download_dir.is_absolute() {
        return Err("Choose a download folder with a full path.".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ commands

#[tauri::command]
fn node_status(state: State<'_, AppState>) -> NodeStatus {
    state.node.status()
}

#[tauri::command]
fn list_shares(state: State<'_, AppState>) -> Vec<ShareInfo> {
    state.node.list_shares()
}

/// One share with a link built from the current addresses (call at the moment of copying).
#[tauri::command]
fn get_share(state: State<'_, AppState>, id: String) -> CmdResult<ShareInfo> {
    state.node.share(&id).ok_or_else(|| "unknown share".into())
}

#[tauri::command]
async fn create_share(state: State<'_, AppState>, paths: Vec<PathBuf>) -> CmdResult<ShareInfo> {
    let node = state.node.clone();
    node.create_share(paths).await.map_err(fail)
}

#[tauri::command]
async fn set_share_paused(
    state: State<'_, AppState>,
    id: String,
    paused: bool,
) -> CmdResult<ShareInfo> {
    let node = state.node.clone();
    node.set_share_paused(&id, paused).await.map_err(fail)
}

#[tauri::command]
async fn remove_share(
    #[cfg_attr(not(target_os = "android"), allow(unused_variables))] app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<()> {
    let node = state.node.clone();
    node.remove_share(&id).await.map_err(fail)?;
    // Phones share copies of the picked files; they go with the share.
    #[cfg(target_os = "android")]
    app.state::<android_send::Outgoing>().forget(&id);
    Ok(())
}

/// Write a `.ferry` file. The extension is always `.ferry`, whatever the path says.
#[tauri::command]
async fn save_share_file(state: State<'_, AppState>, id: String, path: PathBuf) -> CmdResult<()> {
    let node = state.node.clone();
    let path = if has_ticket_extension(&path) {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(format!(".{SHARE_FILE_EXTENSION}"));
        PathBuf::from(name)
    };
    node.save_share_file(&id, &path).await.map_err(fail)
}

/// Protect a share with a password, change it, or remove it (`None`).
#[tauri::command]
async fn set_share_password(
    state: State<'_, AppState>,
    id: String,
    password: Option<String>,
) -> CmdResult<ShareInfo> {
    let node = state.node.clone();
    node.set_share_password(&id, password).await.map_err(fail)
}

/// The share link as an SVG QR code, built from the current addresses.
#[tauri::command]
fn share_qr(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let share = state.node.share(&id).ok_or("unknown share")?;
    // Medium error correction keeps the code small and still survives a glare or a smudge.
    let code =
        qrcode::QrCode::with_error_correction_level(share.link.as_bytes(), qrcode::EcLevel::M)
            .map_err(fail)?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(false)
        .dark_color(qrcode::render::svg::Color("#2A0D20"))
        .light_color(qrcode::render::svg::Color("#FFFFFF"))
        .build())
}

/// Read a `.ferry` file opened from the desktop, picker, or drop handler, and return its link.
/// On phones the picker hands out `content://` URIs instead of paths.
#[tauri::command]
#[cfg_attr(desktop, allow(unused_variables))]
async fn read_share_file(app: AppHandle, path: PathBuf) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(mobile)]
        if let Some(url) = path
            .to_str()
            .filter(|p| p.starts_with("content://"))
            .and_then(|p| tauri::Url::parse(p).ok())
        {
            use tauri_plugin_fs::{FsExt, OpenOptions};
            let file = app.fs().open(url, OpenOptions::new()).map_err(fail)?;
            let link = ferry_core::read_ticket_from(file).map_err(fail)?;
            return Ok(link.to_url());
        }
        read_share_file_checked(&path)
    })
    .await
    .map_err(fail)?
}

fn read_share_file_checked(path: &Path) -> CmdResult<String> {
    if !has_ticket_extension(path) {
        return Err("That is not a Ferry ticket file (.ferry).".into());
    }
    Ok(ferry_core::read_ticket_file(path).map_err(fail)?.to_url())
}

/// What a link holds. A locked share answers with what the password prompt should say.
#[tauri::command]
async fn preview(
    state: State<'_, AppState>,
    link: String,
    password: Option<String>,
) -> CmdResult<PreviewOutcome> {
    let node = state.node.clone();
    let link = ShareLink::parse(&link).map_err(fail)?;
    match node.preview(&link, password).await {
        Ok(preview) => Ok(PreviewOutcome::Ready { preview }),
        Err(err) => match err.downcast_ref::<AccessError>() {
            Some(AccessError::PasswordRequired) => Ok(PreviewOutcome::PasswordRequired),
            Some(AccessError::WrongPassword) => Ok(PreviewOutcome::WrongPassword),
            Some(AccessError::TooManyGuesses { retry_after_secs }) => {
                Ok(PreviewOutcome::TooManyGuesses {
                    retry_after_secs: *retry_after_secs,
                })
            }
            None => Err(fail(err)),
        },
    }
}

#[tauri::command]
fn list_downloads(state: State<'_, AppState>) -> Vec<DownloadInfo> {
    state.node.list_downloads()
}

#[tauri::command]
async fn start_download(
    state: State<'_, AppState>,
    link: String,
    dest: Option<PathBuf>,
    password: Option<String>,
) -> CmdResult<DownloadInfo> {
    let node = state.node.clone();
    let dest = dest.unwrap_or_else(|| state.settings.lock().unwrap().download_dir.clone());
    let link = ShareLink::parse(&link).map_err(fail)?;
    node.start_download(&link, password, dest)
        .await
        .map_err(fail)
}

#[tauri::command]
fn pause_download(state: State<'_, AppState>, id: String) {
    state.node.pause_download(&id);
}

#[tauri::command]
async fn resume_download(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let node = state.node.clone();
    node.resume_download(&id).await.map_err(fail)
}

#[tauri::command]
async fn remove_download(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let node = state.node.clone();
    node.remove_download(&id).await.map_err(fail)
}

/// Show a finished download in the file manager. Only paths of known downloads are accepted.
#[tauri::command]
fn reveal_download(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let path = state
        .node
        .download(&id)
        .and_then(|d| d.saved_path)
        .ok_or("This download has not been saved yet.")?;
    app.opener().reveal_item_in_dir(path).map_err(fail)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

fn save_settings(path: &Path, settings: &Settings) -> CmdResult<()> {
    let json = serde_json::to_vec_pretty(settings).map_err(fail)?;
    ferry_core::write_atomic(path, &json).map_err(fail)
}

/// Save settings. Returns `true` when a restart is needed for them to take effect.
#[tauri::command]
fn set_settings(state: State<'_, AppState>, mut settings: Settings) -> CmdResult<bool> {
    validate_settings(&settings)?;
    let mut saved = state.settings.lock().unwrap();
    settings.auto_check_updates = saved.auto_check_updates;
    // Phones download into app storage and then save into the shared Downloads collection.
    if cfg!(mobile) {
        settings.download_dir = saved.download_dir.clone();
    }
    save_settings(&state.settings_path, &settings)?;
    // Short links switch at once: off also takes the existing ones off the link service. Only
    // turning them on in a run started without them needs a restart.
    {
        let _runtime = tauri::async_runtime::handle().inner().enter();
        state.node.set_short_links(settings.short_links);
    }
    let restart = settings.relay != *state.node.relay()
        || settings.short_links != state.node.uses_link_service()
        || settings.hide_ip != state.node.hides_ip();
    *saved = settings;
    Ok(restart)
}

#[tauri::command]
fn open_project_page(app: AppHandle) -> CmdResult<()> {
    app.opener()
        .open_url(PROJECT_PAGE, None::<&str>)
        .map_err(fail)
}

/// Opens the bundled third-party notices, or shows the file if nothing opens Markdown. Phones
/// have no file to show, so they open this version's notices on GitHub.
#[tauri::command]
fn open_third_party_notices(app: AppHandle) -> CmdResult<()> {
    #[cfg(mobile)]
    {
        let url = format!(
            "{PROJECT_PAGE}/blob/v{}/THIRD_PARTY_NOTICES.md",
            app.package_info().version
        );
        return app.opener().open_url(url, None::<&str>).map_err(fail);
    }
    #[cfg_attr(mobile, allow(unreachable_code))]
    let path = app
        .path()
        .resolve("THIRD_PARTY_NOTICES.md", BaseDirectory::Resource)
        .map_err(fail)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .or_else(|_| app.opener().reveal_item_in_dir(&path))
        .map_err(fail)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    /// `windows`, `macos`, `linux`, `android` or `ios`.
    platform: &'static str,
    mobile: bool,
    /// How this build is distributed and updated: `desktop`, `play`, `direct` or `appstore`.
    distribution: &'static str,
}

/// What kind of Ferry this is, so the UI can adapt without probing the OS.
#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        platform: std::env::consts::OS,
        mobile: cfg!(mobile),
        distribution: env!("FERRY_DISTRIBUTION"),
    }
}

/// Links that arrived from outside (deep link, opened file, second launch) since last asked.
#[tauri::command]
fn take_pending_links(state: State<'_, AppState>) -> Vec<IncomingShare> {
    std::mem::take(&mut *state.pending_links.lock().unwrap())
}

// ------------------------------------------------------------- integration

fn has_ticket_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(SHARE_FILE_EXTENSION))
}

/// Recognize incoming links and paths without opening files on the event thread.
fn link_from_arg(arg: &str) -> Option<IncomingShare> {
    if ferry_core::is_share_url(arg) {
        return Some(IncomingShare::Link {
            link: arg.trim().to_string(),
        });
    }
    // "Open with Ferry" on a phone hands over a content URI; reading it checks the format.
    #[cfg(mobile)]
    if arg.trim().starts_with("content://") {
        return Some(IncomingShare::File {
            path: PathBuf::from(arg.trim()),
        });
    }
    let path = Path::new(arg);
    has_ticket_extension(path).then(|| IncomingShare::File {
        path: path.to_owned(),
    })
}

fn accept_links(app: &AppHandle, args: impl IntoIterator<Item = String>) {
    let links: Vec<_> = args.into_iter().filter_map(|a| link_from_arg(&a)).collect();
    if links.is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        state.pending_links.lock().unwrap().extend(links);
    }
    app.emit(OPEN_LINK, ()).ok();
    // A link that launched Ferry waits for the window to appear with its page (show_at_startup);
    // later ones bring the window forward.
    #[cfg(desktop)]
    if STARTUP_SHOWN.load(std::sync::atomic::Ordering::SeqCst) {
        show_main(app);
    }
}

// Mobile apps have a single window the OS brings forward.
#[cfg(desktop)]
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Once the window has been shown (here by the tray or a second launch too), the startup
        // reveal must not show it again: it may since have been closed to the tray.
        STARTUP_SHOWN.store(true, std::sync::atomic::Ordering::SeqCst);
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
    }
}

/// Set once the main window has been shown, at startup or by an earlier explicit open.
#[cfg(desktop)]
static STARTUP_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The main window starts hidden (tauri.conf.json) so it never shows a blank frame; it appears
/// once its page has loaded, which means the splash is already on screen. Only the first time:
/// a later reload must not bring back a window hidden to the tray.
#[cfg(desktop)]
fn show_at_startup(app: &AppHandle) {
    if !STARTUP_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        show_main(app);
    }
}

fn forward_events(app: AppHandle, node: &Node) {
    let mut rx = node.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    #[cfg(target_os = "android")]
                    android::on_event(&app, &event);
                    if let Event::DownloadUpdated { download } = &event
                        && download.status == DownloadStatus::Completed
                    {
                        app.notification()
                            .builder()
                            .title("Download complete")
                            .body(format!("{} is ready", download.name))
                            .show()
                            .ok();
                    }
                    app.emit(EVENT, &event).ok();
                }
                // Some events were missed: have the UI reload its lists.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // The phone side catches up too: the notification and pending saves.
                    #[cfg(target_os = "android")]
                    android::resume(&app, false);
                    app.emit(RESYNC, ()).ok();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(desktop)]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Ferry", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Ferry", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Ferry: sharing while open")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn init_logging(dir: &Path) {
    let path = dir.join("ferry.log");
    // Keep one previous log; start a new one when the current one gets large.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG) {
        std::fs::rename(&path, dir.join("ferry.log.1")).ok();
    }
    let writer = std::fs::create_dir_all(dir)
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        })
        .ok();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "warn,ferry_core=info,ferry_desktop_lib=info".into());
    match writer {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// Where the node keeps its identity, store and partial downloads: the local (not roaming)
/// app data folder, since partial downloads can be large. Data from versions that used the
/// roaming folder is moved over once.
fn node_dir(app: &tauri::App) -> tauri::Result<PathBuf> {
    let local = app.path().app_local_data_dir()?.join("node");
    let roaming = app.path().app_data_dir()?.join("node");
    if roaming != local && roaming.exists() && !local.exists() {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Err(e) = std::fs::rename(&roaming, &local) {
            tracing::warn!("could not move Ferry data to {}: {e}", local.display());
            return Ok(roaming);
        }
    }
    Ok(local)
}

/// The identifier Ferry 0.2 and earlier used. It names the app's folders.
#[cfg(desktop)]
const LEGACY_IDENTIFIER: &str = "dev.ferry.desktop";

/// What the legacy folders hold that Ferry can't run without: its identity, shares and
/// downloads, and its settings. If one of these stays behind, starting would create a fresh
/// one in the new folder, and the old one would never be moved after that.
#[cfg(desktop)]
const ESSENTIAL_ENTRIES: [&str; 2] = ["node", "settings.json"];

/// The outcome of [`migrate_legacy_dirs`], reported once logging is up.
#[cfg(desktop)]
#[derive(Default)]
struct Migration {
    /// Entries that could not be moved but that Ferry can do without (old logs, caches).
    warnings: Vec<String>,
    /// Essential entries that could not be moved: Ferry must not start.
    errors: Vec<String>,
}

/// Move what the folders named after [`LEGACY_IDENTIFIER`] hold into the current ones. Tauri
/// has already created the new folders (for webview data) by the time `setup` runs, so this
/// moves entries one by one and leaves any whose name is already taken.
#[cfg(desktop)]
fn migrate_legacy_dirs(app: &tauri::App) -> Migration {
    let identifier = app.config().identifier.as_str();
    let path = app.path();
    let mut moved = Vec::new();
    let mut migration = Migration::default();
    for new in [
        path.app_data_dir(),
        path.app_local_data_dir(),
        path.app_config_dir(),
        path.app_cache_dir(),
        path.app_log_dir(),
    ]
    .into_iter()
    .flatten()
    {
        let Some(old) = legacy_dir(&new, identifier) else {
            continue;
        };
        if moved.contains(&old) || !old.is_dir() {
            continue;
        }
        for failure in move_entries(&old, &new) {
            let message = format!(
                "could not move {} to {}: {}",
                failure.from.display(),
                new.display(),
                failure.error
            );
            if failure.essential {
                migration.errors.push(message);
            } else {
                migration.warnings.push(message);
            }
        }
        moved.push(old);
    }
    migration
}

/// An entry [`move_entries`] left behind although its name was free.
#[cfg(desktop)]
#[derive(Debug)]
struct MoveFailure {
    from: PathBuf,
    error: std::io::Error,
    /// One of [`ESSENTIAL_ENTRIES`], or the folder itself could not be read.
    essential: bool,
}

/// `dir` with the path component equal to `identifier` replaced by the legacy one.
#[cfg(desktop)]
fn legacy_dir(dir: &Path, identifier: &str) -> Option<PathBuf> {
    if identifier == LEGACY_IDENTIFIER {
        return None;
    }
    let mut found = false;
    let old = dir
        .components()
        .map(|part| {
            if part.as_os_str() == identifier {
                found = true;
                std::ffi::OsStr::new(LEGACY_IDENTIFIER)
            } else {
                part.as_os_str()
            }
        })
        .collect();
    found.then_some(old)
}

/// Move each entry of `old` whose name is free in `new`, carrying on past failures so one
/// stuck file doesn't keep the rest behind. Returns what could not be moved.
#[cfg(desktop)]
fn move_entries(old: &Path, new: &Path) -> Vec<MoveFailure> {
    // Without a listing nobody can tell whether essential data is stranded.
    let whole = |error| {
        vec![MoveFailure {
            from: old.to_owned(),
            error,
            essential: true,
        }]
    };
    if let Err(e) = std::fs::create_dir_all(new) {
        return whole(e);
    }
    let entries = match std::fs::read_dir(old) {
        Ok(entries) => entries,
        Err(e) => return whole(e),
    };
    let mut failures = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return whole(e),
        };
        let name = entry.file_name();
        let target = new.join(&name);
        if target.exists() {
            continue;
        }
        if let Err(error) = std::fs::rename(entry.path(), target) {
            failures.push(MoveFailure {
                from: entry.path(),
                error,
                essential: ESSENTIAL_ENTRIES.iter().any(|e| name == *e),
            });
        }
    }
    // Only succeeds once nothing is left behind.
    std::fs::remove_dir(old).ok();
    failures
}

/// Tell the user Ferry can't start (there may be no console to see a panic), then quit.
fn fail_startup(app: &tauri::App, problem: &str) {
    let handle = app.handle().clone();
    app.dialog()
        .message(format!(
            "Ferry could not start.\n\n{problem}\n\nIf this keeps happening, check the log in {}.",
            app.path()
                .app_log_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ))
        .title("Ferry")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(desktop)]
    let migration = migrate_legacy_dirs(app);
    init_logging(&app.path().app_log_dir()?);
    #[cfg(desktop)]
    {
        for warning in &migration.warnings {
            tracing::warn!("{warning}");
        }
        if !migration.errors.is_empty() {
            // Starting now would create a new identity and empty lists next to the stranded
            // ones, and later starts would never move those.
            for error in &migration.errors {
                tracing::error!("{error}");
            }
            fail_startup(
                app,
                &format!(
                    "Ferry could not move its data from the folder older versions used.\n\n{}\n\n\
                     Close any other copy of Ferry that is running, then open Ferry again.",
                    migration.errors.join("\n")
                ),
            );
            return Ok(());
        }
    }
    let config_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    let settings_path = config_dir.join("settings.json");
    #[cfg(desktop)]
    let settings = {
        let default_downloads = app
            .path()
            .download_dir()
            .unwrap_or_else(|_| config_dir.join("Downloads"));
        load_settings(&settings_path, default_downloads)
    };
    // Phones download into app storage, then hand finished files to the shared Downloads.
    #[cfg(mobile)]
    let settings = {
        let incoming = app.path().app_local_data_dir()?.join("incoming");
        let mut settings = load_settings(&settings_path, incoming.clone());
        settings.download_dir = incoming;
        settings
    };

    let mut config = Config::new(node_dir(app)?);
    config.relay = settings.relay.clone();
    if !settings.short_links {
        config.links_service = None;
    }
    // Settings saved by hand could ask for both; relays win, so Ferry still starts.
    config.hide_ip = settings.hide_ip && settings.relay != RelayChoice::Disabled;
    let node = match tauri::async_runtime::block_on(Node::start(config)) {
        Ok(node) => node,
        Err(e) => {
            tracing::error!("could not start Ferry: {e:#}");
            fail_startup(app, &format!("{e:#}"));
            return Ok(());
        }
    };
    forward_events(app.handle().clone(), &node);
    // Links shown before the relay connection is up lack the relay address; refresh the UI
    // once the node is reachable from anywhere.
    {
        let node = node.clone();
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            if node.wait_online(std::time::Duration::from_secs(20)).await {
                handle.emit(RESYNC, ()).ok();
            }
        });
    }
    app.manage(AppState {
        node,
        settings: Mutex::new(settings),
        settings_path,
        pending_links: Mutex::new(Vec::new()),
    });
    #[cfg(target_os = "android")]
    {
        app.manage(android::Phone::load(config_dir.join("saved.json")));
        android::start(app.handle());
        android_send::start(app.handle(), &app.path().app_local_data_dir()?);
    }

    #[cfg(desktop)]
    build_tray(app)?;
    // In case the page never reports that it loaded, the window still appears.
    #[cfg(desktop)]
    {
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            show_at_startup(&handle);
        });
    }

    // Register ferry:// for this executable (installers do this too; needed for dev builds
    // and AppImages).
    #[cfg(any(windows, target_os = "linux"))]
    if let Err(e) = app.deep_link().register_all() {
        tracing::warn!("could not register ferry:// links: {e}");
    }
    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        accept_links(&handle, event.urls().into_iter().map(|u| u.to_string()));
    });
    // A link that launched the app before the handler above existed.
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        accept_links(app.handle(), urls.into_iter().map(|u| u.to_string()));
    }
    // Launched by opening a .ferry file or a web link (Windows/Linux pass it as an argument).
    // ferry:// links came through the deep-link plugin above.
    accept_links(
        app.handle(),
        std::env::args()
            .skip(1)
            .filter(|a| !ferry_core::is_app_url(a)),
    );
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg_attr(mobile, allow(unused_mut))]
    let mut builder = tauri::Builder::default();
    // Must be the first plugin: a second launch hands its arguments to the running instance
    // and exits. ferry:// links are forwarded to the deep-link plugin (on_open_url), so only
    // files and web links are handled here.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            accept_links(
                app,
                argv.into_iter()
                    .skip(1)
                    .filter(|a| !ferry_core::is_app_url(a)),
            );
            show_main(app);
        }));
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .manage(updates::PendingUpdate::default());
    }
    let app = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    #[cfg(mobile)]
    let app = app
        .plugin(tauri_plugin_barcode_scanner::init())
        .plugin(tauri_plugin_fs::init());
    #[cfg(target_os = "android")]
    let app = app
        .plugin(tauri_plugin_ferry_mobile::init())
        .plugin(android_updates::init());
    #[cfg(desktop)]
    let app = app.on_page_load(|webview, payload| {
        // Not when startup failed: its error dialog is all there is to show.
        if payload.event() == tauri::webview::PageLoadEvent::Finished
            && webview.label() == "main"
            && webview.try_state::<AppState>().is_some()
        {
            show_at_startup(webview.app_handle());
        }
    });
    let app = app
        .setup(setup)
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = window
                    .try_state::<AppState>()
                    .is_some_and(|s| s.settings.lock().unwrap().close_to_tray);
                if to_tray {
                    api.prevent_close();
                    window.hide().ok();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            node_status,
            list_shares,
            get_share,
            create_share,
            remove_share,
            set_share_paused,
            set_share_password,
            save_share_file,
            share_qr,
            read_share_file,
            preview,
            list_downloads,
            start_download,
            pause_download,
            resume_download,
            remove_download,
            reveal_download,
            get_settings,
            set_settings,
            #[cfg(desktop)]
            updates::update_info,
            #[cfg(desktop)]
            updates::set_auto_check_updates,
            #[cfg(desktop)]
            updates::check_for_updates,
            #[cfg(desktop)]
            updates::install_update,
            #[cfg(target_os = "android")]
            android_updates::update_info,
            #[cfg(target_os = "android")]
            android_updates::set_auto_check_updates,
            #[cfg(target_os = "android")]
            android_updates::check_for_updates,
            #[cfg(target_os = "android")]
            android_updates::install_update,
            take_pending_links,
            app_info,
            #[cfg(target_os = "android")]
            android::saved_downloads,
            #[cfg(target_os = "android")]
            android::open_download,
            #[cfg(target_os = "android")]
            android::share_download,
            #[cfg(target_os = "android")]
            android::retry_save,
            #[cfg(target_os = "android")]
            android_send::pick_items,
            #[cfg(target_os = "android")]
            android_send::take_incoming,
            #[cfg(target_os = "android")]
            android_send::share_items,
            #[cfg(target_os = "android")]
            android_send::share_link,
            open_project_page,
            open_third_party_notices,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Ferry");

    app.run(|app, event| match event {
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                let node = state.node.clone();
                tauri::async_runtime::block_on(async move { node.shutdown().await.ok() });
            }
        }
        // macOS delivers opened files and links as events, not arguments.
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => accept_links(
            app,
            urls.into_iter().map(|u| {
                u.to_file_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| u.to_string())
            }),
        ),
        // Clicking the Dock icon brings back a window hidden to the tray.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_links_and_share_files() {
        for link in [
            "FERRY://s#abc",
            "ferry://s/K7X2QP9A4MZT",
            "https://ferry.xenvaultinteractive.com/s#abc",
            "https://ferry.xenvaultinteractive.com/s/K7X2QP9A4MZT",
        ] {
            assert_eq!(
                link_from_arg(link),
                Some(IncomingShare::Link { link: link.into() })
            );
        }
        assert!(link_from_arg("https://example.com/s#abc").is_none());
        assert!(link_from_arg("https://ferry.xenvaultinteractive.com/get#abc").is_none());
        assert!(link_from_arg("--flag").is_none());
        let dir = tempfile_dir();
        let file = dir.join("x.ferry");
        std::fs::write(&file, "{\"ticket\":\"t\"}").unwrap();
        assert_eq!(
            link_from_arg(file.to_str().unwrap()),
            Some(IncomingShare::File { path: file })
        );
        let other = dir.join("x.txt");
        std::fs::write(&other, "secret").unwrap();
        assert!(link_from_arg(other.to_str().unwrap()).is_none());
        assert!(read_share_file_checked(&other).is_err());
    }

    #[test]
    fn settings_are_validated() {
        let ok = Settings {
            download_dir: std::env::temp_dir(),
            relay: RelayChoice::Custom("https://relay.example.com".into()),
            close_to_tray: true,
            auto_check_updates: true,
            show_artwork: true,
            short_links: true,
            hide_ip: true,
        };
        assert!(validate_settings(&ok).is_ok());
        let hidden_without_relays = Settings {
            relay: RelayChoice::Disabled,
            ..ok.clone()
        };
        assert!(validate_settings(&hidden_without_relays).is_err());
        let bad_relay = Settings {
            relay: RelayChoice::Custom("https://".into()),
            ..ok.clone()
        };
        assert!(validate_settings(&bad_relay).is_err());
        let relative = Settings {
            download_dir: PathBuf::from("downloads"),
            ..ok
        };
        assert!(validate_settings(&relative).is_err());
    }

    #[test]
    fn existing_settings_default_to_automatic_update_checks() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "downloadDir": std::env::temp_dir(),
            "relay": { "mode": "disabled" },
            "closeToTray": false,
        }))
        .unwrap();
        assert!(settings.auto_check_updates);
        assert!(settings.short_links, "short links are on unless turned off");
        assert!(!settings.hide_ip, "hiding the IP address is opt-in");
        assert!(!settings.close_to_tray);
        assert_eq!(settings.relay, RelayChoice::Disabled);
    }

    #[test]
    fn settings_saved_before_artwork_existed_still_load() {
        let dir = tempfile_dir();
        let path = dir.join("old-settings.json");
        let saved = dir.join("Saved here");
        let json = serde_json::json!({
            "downloadDir": saved,
            "relay": { "mode": "disabled" },
            "closeToTray": false,
        });
        std::fs::write(&path, json.to_string()).unwrap();
        let settings = load_settings(&path, std::env::temp_dir());
        assert_eq!(settings.download_dir, saved);
        assert!(!settings.close_to_tray);
        assert!(settings.show_artwork);
        assert!(settings.auto_check_updates);
    }

    #[test]
    fn legacy_dirs_swap_only_the_identifier() {
        let id = "com.xenvaultinteractive.ferry";
        let new = Path::new("data").join(id).join("logs");
        assert_eq!(
            legacy_dir(&new, id),
            Some(Path::new("data").join(LEGACY_IDENTIFIER).join("logs"))
        );
        assert_eq!(legacy_dir(Path::new("data/other"), id), None);
        assert_eq!(
            legacy_dir(&Path::new("x").join(LEGACY_IDENTIFIER), LEGACY_IDENTIFIER),
            None
        );
    }

    #[test]
    fn legacy_entries_move_without_overwriting() {
        let dir = tempfile_dir().join("migrate");
        let (old, new) = (dir.join("old"), dir.join("new"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(old.join("node")).unwrap();
        std::fs::write(old.join("settings.json"), "old").unwrap();
        std::fs::write(old.join("node").join("key"), "k").unwrap();
        std::fs::create_dir_all(new.join("EBWebView")).unwrap();
        std::fs::create_dir_all(old.join("EBWebView")).unwrap();

        assert!(move_entries(&old, &new).is_empty());
        assert_eq!(
            std::fs::read_to_string(new.join("settings.json")).unwrap(),
            "old"
        );
        assert!(new.join("node").join("key").is_file());
        // A name already taken stays behind, so the old folder does too.
        assert!(old.join("EBWebView").is_dir());

        std::fs::remove_dir(old.join("EBWebView")).unwrap();
        assert!(move_entries(&old, &new).is_empty());
        assert!(!old.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unreadable_legacy_folder_counts_as_essential() {
        let dir = tempfile_dir().join("migrate-fail");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("old").join("node")).unwrap();
        // The new folder can't be created where a file sits.
        std::fs::write(dir.join("new"), "").unwrap();
        let failures = move_entries(&dir.join("old"), &dir.join("new"));
        assert_eq!(failures.len(), 1);
        assert!(failures[0].essential);
        assert!(dir.join("old").join("node").is_dir());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferry-desktop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
