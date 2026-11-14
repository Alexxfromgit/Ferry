//! Ferry's phone integration, called from the app's Rust code (never from the webview):
//! saving received files into the shared Downloads collection, opening and sharing them, and
//! a foreground service that keeps transfers alive with the screen off.
//!
//! Android only for now; iOS gets a Swift counterpart later.

#![cfg(target_os = "android")]

use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, IgnoredAny as Ignored},
};
use tauri::{
    Manager, Runtime,
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

/// What the transfer notification shows. `progress` is a percentage; `None` is indeterminate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferStatus {
    pub title: String,
    pub text: String,
    pub progress: Option<u8>,
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
