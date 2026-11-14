//! Ferry's phone integration, called from the app's Rust code (never from the webview):
//! saving received files into the shared Downloads collection, opening and sharing them,
//! picking files to send and copying them into Ferry's storage, receiving files from other
//! apps' Share menus, and a foreground service that keeps transfers and shares alive with the
//! screen off. Android reports back through the channel given to [`FerryMobile::watch`].
//!
//! Android only for now; iOS gets a Swift counterpart later.

#![cfg(target_os = "android")]

use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, IgnoredAny as Ignored},
};
use tauri::{
    Manager, Runtime,
    ipc::{Channel, InvokeResponseBody},
    plugin::{Builder, PluginHandle, TauriPlugin},
};

const PLUGIN_IDENTIFIER: &str = "com.xenvaultinteractive.ferry.mobile";

/// A file saved into Downloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    /// `content://` URI of the saved file.
    pub uri: String,
    /// The name Android saved it under (it adds ` (1)` when the name is taken).
    pub name: String,
}

/// What the transfer notification shows. `progress` is a percentage; `None` is an
/// indeterminate bar, or no bar while `sharing`. With `sharing`, the notification also has a
/// "Stop sharing" button, which arrives as [`MobileEvent::StopSharing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferStatus {
    pub title: String,
    pub text: String,
    pub progress: Option<u8>,
    pub sharing: bool,
}

/// What Android tells Ferry, through the channel given to [`FerryMobile::watch`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MobileEvent {
    /// Files shared to Ferry from another app's Share menu.
    Shared { uris: Vec<String> },
    /// Something was shared to Ferry, but no files (for example, only text).
    SharedNothing,
    /// The notification's "Stop sharing" button.
    StopSharing,
    /// Android ended the foreground service after its daily limit.
    Timeout,
    /// Progress of [`FerryMobile::stage`].
    StageProgress { done_bytes: u64, total_bytes: u64 },
}

/// The result of copying picked items into Ferry's storage.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    /// One path per item that had files: a file, or a folder.
    pub paths: Vec<String>,
    /// Picked folders that had no files.
    pub skipped: usize,
}

#[derive(Deserialize)]
struct Picked {
    uris: Vec<String>,
}

#[derive(Serialize)]
struct WatchArgs {
    channel: Channel<serde_json::Value>,
}

#[derive(Serialize)]
struct StageArgs<'a> {
    uris: &'a [String],
    dest: &'a str,
}

#[derive(Serialize)]
struct TextArgs<'a> {
    text: &'a str,
    title: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveArgs<'a> {
    path: &'a str,
    relative_dir: &'a str,
}

#[derive(Serialize)]
struct UriArgs<'a> {
    uri: &'a str,
}

#[derive(Serialize)]
struct UrisArgs<'a> {
    uris: &'a [String],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferArgs<'a> {
    active: bool,
    #[serde(flatten)]
    status: Option<&'a TransferStatus>,
}

/// Handle to the Android side. Calls block until Android answers, so make them from a
/// blocking thread.
pub struct FerryMobile<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> FerryMobile<R> {
    fn call<T: DeserializeOwned>(&self, command: &str, args: impl Serialize) -> Result<T, String> {
        self.0
            .run_mobile_plugin(command, args)
            .map_err(|e| e.to_string())
    }

    /// Copy `path` into `relative_dir` (which starts with `Download/`) of shared storage.
    pub fn save_to_downloads(&self, path: &str, relative_dir: &str) -> Result<SavedFile, String> {
        self.call("saveToDownloads", SaveArgs { path, relative_dir })
    }

    /// Open a saved file in whatever app handles its type.
    pub fn open(&self, uri: &str) -> Result<(), String> {
        self.call::<Ignored>("open", UriArgs { uri }).map(drop)
    }

    /// Offer saved files to other apps through the share sheet.
    pub fn share(&self, uris: &[String]) -> Result<(), String> {
        self.call::<Ignored>("share", UrisArgs { uris }).map(drop)
    }

    /// Open the system's Downloads view.
    pub fn show_downloads(&self) -> Result<(), String> {
        self.call::<Ignored>("showDownloads", ()).map(drop)
    }

    /// Receive Android's events. Files shared to Ferry before this call arrive right away.
    pub fn watch(
        &self,
        on_event: impl Fn(MobileEvent) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let channel = Channel::new(move |body| {
            let parsed = match body {
                InvokeResponseBody::Json(json) => serde_json::from_str::<MobileEvent>(&json),
                InvokeResponseBody::Raw(bytes) => serde_json::from_slice::<MobileEvent>(&bytes),
            };
            match parsed {
                Ok(event) => on_event(event),
                Err(e) => tracing::warn!("unexpected event from Android: {e}"),
            }
            Ok(())
        });
        self.call::<Ignored>("watch", WatchArgs { channel })
            .map(drop)
    }

    /// Let the user pick files. Empty when they cancel.
    pub fn pick_files(&self) -> Result<Vec<String>, String> {
        self.call::<Picked>("pickFiles", ()).map(|p| p.uris)
    }

    /// Let the user pick a folder (a `content://` tree URI). `None` when they cancel.
    pub fn pick_folder(&self) -> Result<Option<String>, String> {
        self.call::<Picked>("pickFolder", ())
            .map(|p| p.uris.into_iter().next())
    }

    /// Copy picked or shared items (files, or folder trees) into `dest`, an existing empty
    /// folder. Progress arrives as [`MobileEvent::StageProgress`]. Fails before copying when
    /// the phone hasn't enough room.
    pub fn stage(&self, uris: &[String], dest: &str) -> Result<Staged, String> {
        self.call("stage", StageArgs { uris, dest })
    }

    /// Offer a text, such as a share link, through the share sheet.
    pub fn share_text(&self, text: &str, title: Option<&str>) -> Result<(), String> {
        self.call::<Ignored>("shareText", TextArgs { text, title })
            .map(drop)
    }

    /// Start or update the transfer notification (and foreground service), or stop it with
    /// `None`.
    pub fn set_transfer(&self, status: Option<&TransferStatus>) -> Result<(), String> {
        self.call::<Ignored>(
            "setTransfer",
            TransferArgs {
                active: status.is_some(),
                status,
            },
        )
        .map(drop)
    }
}

pub trait FerryMobileExt<R: Runtime> {
    fn ferry_mobile(&self) -> &FerryMobile<R>;
}

impl<R: Runtime, T: Manager<R>> FerryMobileExt<R> for T {
    fn ferry_mobile(&self) -> &FerryMobile<R> {
        self.state::<FerryMobile<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("ferry-mobile")
        .setup(|app, api| {
            let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "FerryMobilePlugin")?;
            app.manage(FerryMobile(handle));
            Ok(())
        })
        .build()
}
