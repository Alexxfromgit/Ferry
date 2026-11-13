//! Ferry desktop shell: a thin Tauri layer over `ferry-core`.
//!
//! Commands map 1:1 to `Node` methods; core events are forwarded to the webview as
//! `ferry-event`. Incoming links (`ferry://` deep links, `.ferry` files opened from the file
//! manager, or a second app launch) are queued and announced with `ferry-open-link`. When the
//! UI may have missed events, `ferry-resync` tells it to reload its lists.

#[cfg(desktop)]
mod updates;

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use ferry_core::{
    Config, DownloadInfo, DownloadStatus, Event, Node, NodeStatus, Preview, RelayChoice,
    SHARE_FILE_EXTENSION, ShareInfo, ShareLink, TicketFile, TicketPassword,
};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent,
    menu::{Menu, MenuItem},
    path::BaseDirectory,
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

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum OpenedTicket {
    Ready { link: String },
    PasswordRequired,
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
async fn remove_share(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let node = state.node.clone();
    node.remove_share(&id).await.map_err(fail)
}

/// Write a `.ferry` file. The extension is always `.ferry`, whatever the path says.
#[tauri::command]
async fn save_share_file(
    state: State<'_, AppState>,
    id: String,
    path: PathBuf,
    password: Option<String>,
) -> CmdResult<()> {
    let file = state.node.share_file(&id).map_err(fail)?;
    let path = if has_ticket_extension(&path) {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(format!(".{SHARE_FILE_EXTENSION}"));
        PathBuf::from(name)
    };
    let password = password.map(|value| TicketPassword::from(value.into_boxed_str()));
    tauri::async_runtime::spawn_blocking(move || file.save(&path, password))
        .await
        .map_err(fail)?
        .map_err(fail)
}

/// The share link as an SVG QR code, built from the current addresses.
#[tauri::command]
fn share_qr(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let share = state.node.share(&id).ok_or("unknown share")?;
    let code = qrcode::QrCode::new(share.link.as_bytes()).map_err(fail)?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(false)
        .dark_color(qrcode::render::svg::Color("#2A0D20"))
        .light_color(qrcode::render::svg::Color("#FFFFFF"))
        .build())
}

/// Read a `.ferry` file opened from the desktop, picker, or drop handler.
#[tauri::command]
async fn read_share_file(path: PathBuf, password: Option<String>) -> CmdResult<OpenedTicket> {
    let password = password.map(|value| TicketPassword::from(value.into_boxed_str()));
    tauri::async_runtime::spawn_blocking(move || read_share_file_checked(&path, password))
        .await
        .map_err(fail)?
}

fn read_share_file_checked(
    path: &Path,
    password: Option<TicketPassword>,
) -> CmdResult<OpenedTicket> {
    if !has_ticket_extension(path) {
        return Err("That is not a Ferry ticket file (.ferry).".into());
    }
    let link = match (TicketFile::read(path).map_err(fail)?, password) {
        (TicketFile::Ready(link), None) => link,
        (TicketFile::Protected(_), None) => return Ok(OpenedTicket::PasswordRequired),
        (TicketFile::Protected(ticket), Some(password)) => ticket.unlock(password).map_err(fail)?,
        (TicketFile::Ready(_), Some(_)) => {
            return Err("The ticket file changed. Open it again.".into());
        }
    };
    Ok(OpenedTicket::Ready {
        link: link.to_url(),
    })
}

#[tauri::command]
async fn preview(state: State<'_, AppState>, link: String) -> CmdResult<Preview> {
    let node = state.node.clone();
    let link = ShareLink::parse(&link).map_err(fail)?;
    node.preview(&link).await.map_err(fail)
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
) -> CmdResult<DownloadInfo> {
    let node = state.node.clone();
    let dest = dest.unwrap_or_else(|| state.settings.lock().unwrap().download_dir.clone());
    let link = ShareLink::parse(&link).map_err(fail)?;
    node.start_download(&link, dest).await.map_err(fail)
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
    save_settings(&state.settings_path, &settings)?;
    let restart = settings.relay != *state.node.relay();
    *saved = settings;
    Ok(restart)
}

#[tauri::command]
fn open_project_page(app: AppHandle) -> CmdResult<()> {
    app.opener()
        .open_url(PROJECT_PAGE, None::<&str>)
        .map_err(fail)
}

/// Opens the bundled third-party notices, or shows the file if nothing opens Markdown.
#[tauri::command]
fn open_third_party_notices(app: AppHandle) -> CmdResult<()> {
    let path = app
        .path()
        .resolve("THIRD_PARTY_NOTICES.md", BaseDirectory::Resource)
        .map_err(fail)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .or_else(|_| app.opener().reveal_item_in_dir(&path))
        .map_err(fail)
}

/// Links that arrived from outside (deep link, opened file, second launch) since last asked.
#[tauri::command]
fn take_pending_links(state: State<'_, AppState>) -> Vec<IncomingShare> {
    std::mem::take(&mut *state.pending_links.lock().unwrap())
}

// ------------------------------------------------------------- integration

fn is_ferry_url(arg: &str) -> bool {
    arg.trim()
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("ferry://"))
}

fn has_ticket_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(SHARE_FILE_EXTENSION))
}

/// Recognize incoming links and paths without opening files on the event thread.
fn link_from_arg(arg: &str) -> Option<IncomingShare> {
    if is_ferry_url(arg) {
        return Some(IncomingShare::Link {
            link: arg.trim().to_string(),
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
    show_main(app);
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
    }
}

fn forward_events(app: AppHandle, node: &Node) {
    let mut rx = node.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
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
                    app.emit(RESYNC, ()).ok();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

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

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    init_logging(&app.path().app_log_dir()?);
    let config_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    let settings_path = config_dir.join("settings.json");
    let default_downloads = app
        .path()
        .download_dir()
        .unwrap_or_else(|_| config_dir.join("Downloads"));
    let settings = load_settings(&settings_path, default_downloads);

    let mut config = Config::new(node_dir(app)?);
    config.relay = settings.relay.clone();
    let node = match tauri::async_runtime::block_on(Node::start(config)) {
        Ok(node) => node,
        Err(e) => {
            // Without a console nobody would see a panic; tell the user, then quit.
            tracing::error!("could not start Ferry: {e:#}");
            let handle = app.handle().clone();
            app.dialog()
                .message(format!(
                    "Ferry could not start.\n\n{e:#}\n\nIf this keeps happening, check the log in {}.",
                    app.path()
                        .app_log_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ))
                .title("Ferry")
                .kind(MessageDialogKind::Error)
                .show(move |_| handle.exit(1));
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

    build_tray(app)?;

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
    // Launched by opening a .ferry file (Windows/Linux pass it as an argument).
    accept_links(
        app.handle(),
        std::env::args().skip(1).filter(|a| !is_ferry_url(a)),
    );
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    // Must be the first plugin: a second launch hands its arguments to the running instance
    // and exits. ferry:// links are forwarded to the deep-link plugin (on_open_url), so only
    // file arguments are handled here.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            accept_links(app, argv.into_iter().skip(1).filter(|a| !is_ferry_url(a)));
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
        .plugin(tauri_plugin_clipboard_manager::init())
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
            take_pending_links,
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
        assert_eq!(
            link_from_arg("FERRY://get?t=abc"),
            Some(IncomingShare::Link {
                link: "FERRY://get?t=abc".into()
            })
        );
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
        assert!(read_share_file_checked(&other, None).is_err());
    }

    #[test]
    fn settings_are_validated() {
        let ok = Settings {
            download_dir: std::env::temp_dir(),
            relay: RelayChoice::Custom("https://relay.example.com".into()),
            close_to_tray: true,
            auto_check_updates: true,
            show_artwork: true,
        };
        assert!(validate_settings(&ok).is_ok());
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

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferry-desktop-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
