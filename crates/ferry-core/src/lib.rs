//! Ferry engine: share files and folders peer-to-peer.
//!
//! Built on [iroh](https://iroh.computer) (connections dialed by public key, with hole punching
//! and relay fallback) and `iroh-blobs` (BLAKE3-verified, resumable transfers).
//!
//! The crate is UI-agnostic: the CLI and the Tauri desktop app are thin layers over [`Node`].

mod access;
mod config;
mod events;
mod identity;
mod link;
mod node;
pub mod protocol;
mod provider;
mod registry;
mod speed;
mod ticket_file;
mod util;

pub use access::{MIN_PASSWORD_CHARS, check_password};
pub use config::{Config, RelayChoice, validate_relay_url};
pub use events::{ConnectionKind, Event, ReceiverProgress};
pub use link::{
    CODE_LEN, FileEntry, LINK_SCHEME, SHARE_FILE_EXTENSION, ShareLink, ShareToken, WEB_LINK_HOST,
    format_code, is_app_url, is_share_url, share_code,
};
pub use node::{AccessError, Node, NodeStatus, PREVIEW_FILES, Preview};
pub use registry::{DownloadInfo, DownloadStatus, ShareInfo, SkipReason, SkippedItem};
pub use ticket_file::{read_ticket_file, read_ticket_from};
pub use util::write_atomic;
